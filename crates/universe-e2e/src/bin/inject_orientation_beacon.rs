//! Injects the Lumina Prime Orientation Beacon loop (a portable graph
//! projection) into the LIVE canonical store as ONE additive, atomic
//! transaction, and places the beacon's `Built` pose at the civic
//! coordinate-frame ORIGIN (0, 0, 0) — Balise Zéro.
//!
//! This is the same LOWER write layer as `place_built_position`: a hand-built
//! write-set (intern symbols + N PutEntity + M PutRelation), committed at a tick
//! boundary and INDEPENDENTLY read back from a fresh reopen. It is NOT the
//! permanent semantic-intent path; it is the bootstrap injector the generic
//! translator generalizes.
//!
//! Epistemic honesty:
//!   * Every member of the portable projection becomes one canonical entity
//!     carrying its original `canonical_id`; no member is silently dropped.
//!   * A relation whose source OR target is absent from the injected id-set is
//!     SKIPPED and reported (this is how the missing parent-city `PART_OF` edge
//!     is dropped instead of dangling — per the operator's decision).
//!   * The beacon pose is `provenance:"built"` and MUST carry a `CONSTRUCTED_BY`
//!     construction Moment; a built pose with no Moment is a forgery and aborts.
//!
//! Usage: `inject_orientation_beacon [fixture.json] [store-dir]`
//!   fixture.json defaults to fixtures/ontology/lumina-prime-orientation-beacon-v0.json
//!   store-dir    defaults to artifacts/ontology-registry/current/store

use std::{collections::BTreeMap, env, error::Error, path::PathBuf};

use universe_core::{EntityKey, RelationKey, Tick};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// Canonical predicate remap. The portable projection uses passive/ad-hoc edge
/// names (`IMPLEMENTED_IN`, `VALIDATED_BY`, ...) that are NOT in the canonical
/// ontology's predicate vocabulary (fixtures/ontology/canonical-ontology.json).
/// Each authored predicate maps to an ACTIVE-VOICE canonical predicate and a
/// `swap` flag (true = reverse source/target so the canonical direction holds,
/// e.g. `space IMPLEMENTED_IN impl` becomes `impl IMPLEMENTS space`).
/// An authored predicate absent from this table is a hard error — the injector
/// never silently mints a non-canonical predicate into the canonical store.
fn canonical_predicate(authored: &str) -> Option<(&'static str, bool)> {
    Some(match authored {
        "PART_OF" => ("PART_OF", false),
        "IMPLEMENTED_IN" => ("IMPLEMENTS", true),
        "DEFINED_BY_CODE" => ("DEFINES", true),
        "IMPLEMENTED_BY" => ("COMPILES_TO", false),
        "JUSTIFIED_BY" => ("GROUNDS", true),
        "VALIDATED_BY" => ("TESTS", true),
        "OBSERVED_BY" => ("OBSERVES", true),
        "PRODUCES" => ("PRODUCES", false),
        "FEEDS" => ("FEEDS", false),
        "SUPPORTS" => ("MOTIVATES", false),
        _ => return None,
    })
}

/// Beacon member subtypes that are ALSO canonical node-type symbols. Such an
/// entity carries the specific canonical type as its symbol instead of the
/// generic `node_type` — strictly more ontology-conformant, and interns nothing
/// new (both symbols already exist in the canonical seed).
const CANONICAL_TYPE_SUBTYPES: &[&str] = &["metric", "validation"];

// Disjoint key blocks (live store currently tops out at 0x230f).
const ENTITY_BASE: u128 = 0xB000; // beacon space + members, by ordered index
const POS_POSITION: u128 = 0xB800;
const POS_CONSTRUCTION: u128 = 0xB801;
const POS_JUSTIFICATION: u128 = 0xB802;
const REL_BASE: u128 = 0xBB00; // included intra-graph relations, by index
const REL_HAS_POSITION: u128 = 0xBBF0;
const REL_CONSTRUCTED_BY: u128 = 0xBBF1;
const REL_JUSTIFIED_BY: u128 = 0xBBF2;

fn main() {
    if let Err(error) = run() {
        eprintln!("INJECTION FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let fixture_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("fixtures/ontology/lumina-prime-orientation-beacon-v0.json"));
    let store_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    println!("fixture  : {}", fixture_path.display());
    println!("store dir: {}", store_dir.display());

    // 1. Parse the portable projection.
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&fixture_path)?)?;
    let root_id = doc
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("fixture has no top-level id")?
        .to_string();

    // Ordered entity list: the beacon Space itself, then every member.
    struct Node {
        id: String,
        node_type: String,
        subtype: String,
        content: serde_json::Value,
    }
    let mut nodes: Vec<Node> = Vec::new();
    let node_from = |v: &serde_json::Value| -> Result<Node, Box<dyn Error>> {
        Ok(Node {
            id: v
                .get("id")
                .and_then(|x| x.as_str())
                .ok_or("node without id")?
                .to_string(),
            node_type: v
                .get("node_type")
                .and_then(|x| x.as_str())
                .unwrap_or("thing")
                .to_string(),
            subtype: v
                .get("subtype")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            content: v.get("content").cloned().unwrap_or(serde_json::Value::Null),
        })
    };
    nodes.push(node_from(&doc)?);
    for member in doc
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or("fixture has no members array")?
    {
        nodes.push(node_from(member)?);
    }

    // id -> EntityKey (ordered, deterministic).
    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let key = EntityKey(ENTITY_BASE + i as u128);
        if id_to_key.insert(node.id.clone(), key).is_some() {
            return Err(format!("duplicate node id {}", node.id).into());
        }
    }
    let beacon_key = *id_to_key.get(&root_id).expect("root is indexed");
    println!("nodes to inject: {} (beacon = {:#x})", nodes.len(), beacon_key.0);

    // 2. Partition relations: keep only those whose BOTH endpoints are injected.
    struct Rel {
        source: EntityKey,
        target: EntityKey,
        predicate: String,
    }
    let mut kept: Vec<Rel> = Vec::new();
    let mut dropped: Vec<(String, String, String)> = Vec::new();
    for r in doc
        .get("relations")
        .and_then(|v| v.as_array())
        .unwrap_or(&Vec::new())
    {
        let source = r.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let target = r.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let authored = r.get("predicate").and_then(|v| v.as_str()).unwrap_or("");
        // Conform to the canonical predicate vocabulary (active-voice + direction).
        let (predicate, swap) = canonical_predicate(authored)
            .ok_or_else(|| format!("authored predicate {authored} has no canonical mapping"))?;
        match (id_to_key.get(source), id_to_key.get(target)) {
            (Some(s), Some(t)) => {
                let (src, tgt) = if swap { (*t, *s) } else { (*s, *t) };
                if authored != predicate {
                    println!(
                        "  REMAP    {authored:<15} -> {predicate:<12} {}  ({} -> {})",
                        if swap { "[swap dir]" } else { "" },
                        short(source),
                        short(target)
                    );
                }
                kept.push(Rel {
                    source: src,
                    target: tgt,
                    predicate: predicate.to_string(),
                });
            }
            _ => dropped.push((source.to_string(), predicate.to_string(), target.to_string())),
        }
    }
    println!("relations kept: {} | dropped (dangling): {}", kept.len(), dropped.len());
    for (s, p, t) in &dropped {
        println!("  DROPPED  {s}  -[{p}]->  {t}   (endpoint not in injected set)");
    }

    // 3. Open the LIVE store and replay to the authoritative snapshot.
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "\nbase revision: {} | entities: {} | relations: {}",
        base_revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );
    // Guard: never overwrite an existing key.
    for node in &nodes {
        let key = id_to_key[&node.id];
        if snapshot.entities.iter().any(|e| e.key == key) {
            return Err(format!("entity key {:#x} ({}) already exists in the store", key.0, node.id).into());
        }
    }

    // 4. Placement content — the beacon's Built pose at the ORIGIN (Balise Zéro).
    let position_content = serde_json::json!({
        "kind": "built_position",
        "canonical_id": "built_position:l2:lumina-prime:orientation-beacon-v0",
        "coordinate_frame_id": "lumina-prime-civic",
        "placed_node": root_id,
        "east_mm": 0, "north_mm": 0, "elevation_mm": 0,
        "x": 0.0, "y": 0.0, "z": 0.0,
        "provenance": "built",
        "note": "Balise Zéro — the civic coordinate-frame origin; the beacon defines (0,0)."
    });
    let construction_content = serde_json::json!({
        "kind": "placement_construction",
        "canonical_id": "construction:l2:lumina-prime:orientation-beacon-v0",
        "authored_by": "a.inchauspe@digitalkin.ai",
        "base_revision": base_revision.0,
        "note": "Placed the orientation beacon at civic origin (0,0,0). Parent-city PART_OF edge dropped (city not yet built)."
    });
    let justification_content = serde_json::json!({
        "kind": "placement_justification",
        "canonical_id": "justification:l2:lumina-prime:orientation-beacon-placement-v0",
        "statement": "Balise Zéro is the origin of the Lumina Prime coordinate frame, so the beacon's Built pose is authored at (0,0,0)."
    });

    // 5. Plan symbol interning: node_type symbols + placement symbols + predicates.
    // A node whose subtype is a canonical node-type carries that specific type.
    let entity_symbol = |node: &Node| -> String {
        if CANONICAL_TYPE_SUBTYPES.contains(&node.subtype.as_str()) {
            node.subtype.clone()
        } else {
            node.node_type.clone()
        }
    };

    let mut requested: Vec<String> = Vec::new();
    for node in &nodes {
        requested.push(entity_symbol(node));
    }
    for s in ["built_position", "placement_construction", "placement_justification"] {
        requested.push(s.to_string());
    }
    for r in &kept {
        requested.push(r.predicate.clone());
    }
    for p in ["HAS_POSITION", "CONSTRUCTED_BY", "JUSTIFIED_BY"] {
        requested.push(p.to_string());
    }
    requested.sort();
    requested.dedup();
    let plan = snapshot.plan_symbol_interning(&requested)?;
    // Canonical-conformance guard: a clean re-injection must intern NOTHING new.
    // Node types, the placement vocabulary, and every remapped predicate already
    // exist (canonical seed + the store's built-placement convention). A non-empty
    // additions set means a non-canonical symbol slipped through — refuse.
    if !plan.additions.is_empty() {
        return Err(format!(
            "conformance violation: injection would intern new symbols {:?} (expected none)",
            plan.additions
        )
        .into());
    }
    println!("symbol conformance: 0 new symbols interned (all canonical / pre-existing)");
    let sym = |name: &str| -> Result<u32, Box<dyn Error>> {
        plan.assignments
            .get(name)
            .copied()
            .ok_or_else(|| format!("symbol {name} was not planned").into())
    };

    // 6. Build the atomic write-set.
    let mut commands = Vec::new();
    if !plan.additions.is_empty() {
        println!("interning {} new symbols: {:?}", plan.additions.len(), plan.additions);
        commands.push(UniverseCommand::InternSymbols {
            symbols: plan.additions.clone(),
        });
    }
    // 6a. Beacon Space + members.
    for node in &nodes {
        let content = serde_json::json!({
            "canonical_id": node.id,
            "node_type": node.node_type,
            "subtype": node.subtype,
            "content": node.content,
        });
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: id_to_key[&node.id],
                generation: 0,
                symbol: sym(&entity_symbol(node))?,
                content: Some(store.append_content(&content)?),
            },
        });
    }
    // 6b. Placement entities.
    commands.push(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key: EntityKey(POS_POSITION),
            generation: 0,
            symbol: sym("built_position")?,
            content: Some(store.append_content(&position_content)?),
        },
    });
    commands.push(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key: EntityKey(POS_CONSTRUCTION),
            generation: 0,
            symbol: sym("placement_construction")?,
            content: Some(store.append_content(&construction_content)?),
        },
    });
    commands.push(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key: EntityKey(POS_JUSTIFICATION),
            generation: 0,
            symbol: sym("placement_justification")?,
            content: Some(store.append_content(&justification_content)?),
        },
    });
    // 6c. Intra-graph relations (endpoints proven present above).
    for (i, r) in kept.iter().enumerate() {
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(REL_BASE + i as u128),
                generation: 0,
                source: r.source,
                target: r.target,
                predicate: sym(&r.predicate)?,
                content: None,
            },
        });
    }
    // 6d. Placement relations: beacon HAS_POSITION pose; pose CONSTRUCTED_BY / JUSTIFIED_BY.
    commands.push(UniverseCommand::PutRelation {
        relation: RelationRecord {
            key: RelationKey(REL_HAS_POSITION),
            generation: 0,
            source: beacon_key,
            target: EntityKey(POS_POSITION),
            predicate: sym("HAS_POSITION")?,
            content: None,
        },
    });
    commands.push(UniverseCommand::PutRelation {
        relation: RelationRecord {
            key: RelationKey(REL_CONSTRUCTED_BY),
            generation: 0,
            source: EntityKey(POS_POSITION),
            target: EntityKey(POS_CONSTRUCTION),
            predicate: sym("CONSTRUCTED_BY")?,
            content: None,
        },
    });
    commands.push(UniverseCommand::PutRelation {
        relation: RelationRecord {
            key: RelationKey(REL_JUSTIFIED_BY),
            generation: 0,
            source: EntityKey(POS_POSITION),
            target: EntityKey(POS_JUSTIFICATION),
            predicate: sym("JUSTIFIED_BY")?,
            content: None,
        },
    });

    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "mutation:lumina-orientation-beacon:v0".to_string(),
        causal_ancestry: vec!["changeset:lumina-orientation-beacon-v0".to_string()],
        commands,
    };

    // 7. Prepare + commit as ONE atomic transaction at a tick boundary.
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\ncommitted {command_count} commands as one atomic set");
    println!("commit receipt: {receipt:?}");

    // 8. INDEPENDENT readback: fresh reopen from disk.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);
    println!(
        "entities: {} | relations: {}",
        after.entities.len(),
        after.relations.len()
    );

    // 8a. Every injected node present, by key + canonical_id.
    for node in &nodes {
        let key = id_to_key[&node.id];
        let entity = after
            .entities
            .iter()
            .find(|e| e.key == key)
            .ok_or_else(|| format!("injected node {} ({:#x}) not found on readback", node.id, key.0))?;
        let content = fresh.read_content(
            entity
                .content
                .as_ref()
                .ok_or_else(|| format!("node {} has no content", node.id))?,
        )?;
        let canonical = content.get("canonical_id").and_then(|v| v.as_str()).unwrap_or("(none)");
        if canonical != node.id {
            return Err(format!("canonical_id mismatch for {:#x}: {} != {}", key.0, canonical, node.id).into());
        }
    }
    println!("all {} injected nodes read back with matching canonical_id", nodes.len());

    // 8b. Beacon pose at origin, with construction Moment (forgery check).
    let pos = after
        .entities
        .iter()
        .find(|e| e.key == EntityKey(POS_POSITION))
        .ok_or("built_position not found on readback")?;
    let pos_content = fresh.read_content(pos.content.as_ref().ok_or("built_position has no content")?)?;
    println!("built_position {:#x}: {}", POS_POSITION, pos_content);

    let has_position = after.relations.iter().any(|r| {
        r.source == beacon_key && r.target == EntityKey(POS_POSITION) && r.key == RelationKey(REL_HAS_POSITION)
    });
    let constructed_by = after.relations.iter().any(|r| {
        r.source == EntityKey(POS_POSITION) && r.target == EntityKey(POS_CONSTRUCTION)
    });
    let justified_by = after.relations.iter().any(|r| {
        r.source == EntityKey(POS_POSITION) && r.target == EntityKey(POS_JUSTIFICATION)
    });
    println!(
        "placement edges — HAS_POSITION: {has_position} | CONSTRUCTED_BY: {constructed_by} | JUSTIFIED_BY: {justified_by}"
    );

    let at_origin = ["x", "y", "z"]
        .iter()
        .all(|axis| pos_content.get(axis).and_then(|v| v.as_f64()) == Some(0.0));
    let provenance = pos_content.get("provenance").and_then(|v| v.as_str()).unwrap_or("(none)");
    if provenance == "built" && !constructed_by {
        return Err("FORGERY: provenance=built with no CONSTRUCTED_BY construction Moment".into());
    }
    if !(has_position && constructed_by && justified_by) {
        return Err("readback is missing one of the placement edges".into());
    }
    if !at_origin {
        return Err("beacon pose is NOT at the civic origin (0,0,0)".into());
    }

    println!(
        "\nRESULT: injected the Lumina Prime orientation beacon ({} nodes, {} intra-graph relations) into the LIVE store,",
        nodes.len(),
        kept.len()
    );
    println!(
        "        placed its Built pose at civic origin (0,0,0) with an authored construction Moment,"
    );
    println!("        and read the whole subgraph back from a fresh reopen. graph_status: WRITTEN (wiring/runtime still not_wired).");
    Ok(())
}

/// Short tail of a canonical id for compact remap logging.
fn short(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
