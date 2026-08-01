//! Generic additive injector for a `members[]`-envelope construct into a LIVE,
//! already-seeded `UniverseStore`, with independent readback.
//!
//! This generalizes the one-off `inject_energy_pen` / `inject_orientation_beacon`
//! bins into ONE reusable command. It is the live-store counterpart of
//! `activate_authority`: `activate_authority` stands a store up from the
//! canonical seed (it refuses a non-empty store), whereas this bin appends ONE
//! atomic transaction (0 interned symbols + N PutEntity + M PutRelation) to a
//! store that is already populated. It is bootstrap tooling, NOT the permanent
//! semantic-intent write path.
//!
//! Contract & honesty:
//!   * The fixture is a construct: a `space` root + `members[]` + `relations[]`,
//!     carrying `content.contractKind == "construct"` and the 12 role subtypes.
//!   * Each member becomes one canonical entity carrying its `canonical_id`.
//!   * A relation whose source OR target is absent from the injected id-set is
//!     SKIPPED and reported (never dangled).
//!   * A clean injection interns ZERO new symbols; a non-canonical authored
//!     predicate or symbol is a hard error, never minted into the canonical store.
//!   * Entity keys are allocated in a fixture-specific disjoint block derived
//!     from the root id; an existing key is a hard error (no overwrite).
//!   * The whole subgraph is read back from a fresh reopen and the construct
//!     contract is re-derived from the committed space node.
//!
//! Usage: `inject_construct <fixture.json> [store-dir]`
//!   store-dir defaults to artifacts/ontology-registry/current/store

use std::{
    collections::{hash_map::DefaultHasher, BTreeMap},
    env,
    error::Error,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use universe_core::{EntityKey, RelationKey, Tick};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// Canonical predicate remap (active-voice canonical predicates + swap flag).
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
        // Guard edge: an underground mechanism points at the sealed hatch that
        // guards its mutation. GUARDED_BY has no canonical symbol; it remaps to
        // the nearest canonical predicate SAFEGUARDS ("protège", canonical
        // direction garde-fou -> élément protégé) with the direction SWAPPED, so
        // the stored edge reads hatch SAFEGUARDS mechanism. Interns 0 new symbols.
        "GUARDED_BY" => ("SAFEGUARDS", true),
        // Appearance edge: a construct points at the physicalization_binding that
        // carries its form (the sky / appearance / district fixtures all author it
        // as PROJECTS_AS). PROJECTS_AS has no canonical symbol; the binding IS the
        // construct's compiled projection, so it remaps to COMPILES_TO in the same
        // direction (construct -> binding), exactly as code -> implementation.
        // Interns 0 new symbols.
        "PROJECTS_AS" => ("COMPILES_TO", false),
        _ => return None,
    })
}

/// Member subtypes that are ALSO canonical node-type symbols.
const CANONICAL_TYPE_SUBTYPES: &[&str] = &["metric", "validation"];

/// The 12 role subtypes a construct must carry.
const REQUIRED_ROLES: &[&str] = &[
    "objective",
    "pattern",
    "vocabulary",
    "behavior",
    "algorithm",
    "code",
    "implementation",
    "justification",
    "validation",
    "observability_algorithm",
    "metric",
    "health",
];

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
        .ok_or("usage: inject_construct <fixture.json> [store-dir]")?;
    let store_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    println!("fixture  : {}", fixture_path.display());
    println!("store dir: {}", store_dir.display());

    // 1. Parse the construct projection.
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&fixture_path)?)?;
    let root_id = doc
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("fixture has no top-level id")?
        .to_string();

    // Fixture-specific disjoint key block, derived from the root id so distinct
    // constructs never collide. High 16 MiB-aligned windows keep clear of the
    // kernel (~0x2300) and prior one-off blocks (beacon 0xB000, pen 0xC000).
    let mut hasher = DefaultHasher::new();
    root_id.hash(&mut hasher);
    let entity_base: u128 = 0x0001_0000 + ((hasher.finish() as u128 & 0x0FFF) << 16);
    let rel_base: u128 = entity_base + 0x8000;

    struct Node {
        id: String,
        node_type: String,
        subtype: String,
        content: serde_json::Value,
    }
    let node_from = |v: &serde_json::Value| -> Result<Node, Box<dyn Error>> {
        Ok(Node {
            id: v.get("id").and_then(|x| x.as_str()).ok_or("node without id")?.to_string(),
            node_type: v.get("node_type").and_then(|x| x.as_str()).unwrap_or("thing").to_string(),
            subtype: v.get("subtype").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            content: v.get("content").cloned().unwrap_or(serde_json::Value::Null),
        })
    };
    let mut nodes: Vec<Node> = Vec::new();
    nodes.push(node_from(&doc)?);
    for member in doc
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or("fixture has no members array")?
    {
        nodes.push(node_from(member)?);
    }

    // Contract check before writing: contractKind + all 12 roles present.
    let contract_kind = doc
        .pointer("/content/contractKind")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    if contract_kind != "construct" {
        return Err(format!(
            "fixture root contractKind is '{contract_kind}', expected 'construct'"
        )
        .into());
    }
    for role in REQUIRED_ROLES {
        if !nodes.iter().any(|n| n.subtype == *role) {
            return Err(format!("not a valid construct: missing role subtype '{role}'").into());
        }
    }

    // id -> EntityKey (ordered, deterministic).
    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let key = EntityKey(entity_base + i as u128);
        if id_to_key.insert(node.id.clone(), key).is_some() {
            return Err(format!("duplicate node id {}", node.id).into());
        }
    }
    let space_key = *id_to_key.get(&root_id).expect("root is indexed");
    println!(
        "construct: {root_id}\nnodes to inject: {} (space = {:#x}, key block {:#x}..)",
        nodes.len(),
        space_key.0,
        entity_base
    );

    // 2. Partition relations: keep only those whose BOTH endpoints are injected.
    struct Rel {
        source: EntityKey,
        target: EntityKey,
        predicate: String,
    }
    let mut kept: Vec<Rel> = Vec::new();
    let mut dropped: Vec<(String, String, String)> = Vec::new();
    for r in doc.get("relations").and_then(|v| v.as_array()).unwrap_or(&Vec::new()) {
        let source = r.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let target = r.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let authored = r.get("predicate").and_then(|v| v.as_str()).unwrap_or("");
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
                kept.push(Rel { source: src, target: tgt, predicate: predicate.to_string() });
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
    for node in &nodes {
        let key = id_to_key[&node.id];
        if snapshot.entities.iter().any(|e| e.key == key) {
            return Err(format!("entity key {:#x} ({}) already exists in the store", key.0, node.id).into());
        }
    }

    // 4. Plan symbol interning: node_type symbols + remapped predicates only.
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
    for r in &kept {
        requested.push(r.predicate.clone());
    }
    requested.sort();
    requested.dedup();
    let plan = snapshot.plan_symbol_interning(&requested)?;
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

    // 5. Build the atomic write-set: space + members, then intra-graph edges.
    let mut commands = Vec::new();
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
    for (i, r) in kept.iter().enumerate() {
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(rel_base + i as u128),
                generation: 0,
                source: r.source,
                target: r.target,
                predicate: sym(&r.predicate)?,
                content: None,
            },
        });
    }

    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: format!("mutation:inject-construct:{root_id}"),
        commands,
    };

    // 6. Prepare + commit as ONE atomic transaction at a tick boundary.
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\ncommitted {command_count} commands as one atomic set");
    println!("commit receipt: {receipt:?}");

    // 7. INDEPENDENT readback: fresh reopen from disk.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);
    println!("entities: {} | relations: {}", after.entities.len(), after.relations.len());

    for node in &nodes {
        let key = id_to_key[&node.id];
        let entity = after
            .entities
            .iter()
            .find(|e| e.key == key)
            .ok_or_else(|| format!("injected node {} ({:#x}) not found on readback", node.id, key.0))?;
        let content = fresh.read_content(
            entity.content.as_ref().ok_or_else(|| format!("node {} has no content", node.id))?,
        )?;
        let canonical = content.get("canonical_id").and_then(|v| v.as_str()).unwrap_or("(none)");
        if canonical != node.id {
            return Err(format!("canonical_id mismatch for {:#x}: {} != {}", key.0, canonical, node.id).into());
        }
    }
    println!("all {} injected nodes read back with matching canonical_id", nodes.len());

    for r in &kept {
        let sym_id = sym(&r.predicate)?;
        let present = after
            .relations
            .iter()
            .any(|x| x.source == r.source && x.target == r.target && x.predicate == sym_id);
        if !present {
            return Err(format!(
                "kept relation {:#x} -[{}]-> {:#x} missing on readback",
                r.source.0, r.predicate, r.target.0
            )
            .into());
        }
    }
    println!("all {} intra-graph relations read back", kept.len());

    // 7c. Construct contract from the COMMITTED space node.
    let space_entity = after
        .entities
        .iter()
        .find(|e| e.key == space_key)
        .ok_or("space node not found on readback")?;
    let space_content = fresh.read_content(space_entity.content.as_ref().ok_or("space has no content")?)?;
    let readback_kind = space_content
        .pointer("/content/contractKind")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    if readback_kind != "construct" {
        return Err(format!("space contractKind is '{readback_kind}' on readback, expected 'construct'").into());
    }
    println!("construct contract verified from store: contractKind={readback_kind}, all 12 roles present");

    println!(
        "\nRESULT: injected construct {root_id} ({} nodes, {} intra-graph relations) into the LIVE store",
        nodes.len(),
        kept.len()
    );
    println!("        as one atomic transaction, interned 0 new symbols, and read the whole construct back.");
    println!("        graph_status: WRITTEN. wiring/runtime/health remain not_wired / not_running / not_measured.");
    Ok(())
}

fn short(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
