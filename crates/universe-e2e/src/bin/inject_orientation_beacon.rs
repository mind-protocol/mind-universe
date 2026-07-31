//! Injects the Lumina Prime Orientation Beacon loop (a portable graph
//! projection) into the LIVE canonical store as ONE additive, atomic
//! transaction. It authors NO coordinate: positions are inferred by the layout
//! solver, never stored, so the beacon carries no `built_position` pose.
//!
//! This is the same LOWER write layer as the other injectors: a hand-built
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
//!   * No `HAS_POSITION` / `built_position` is written — a node's place is
//!     emergent (solver output), not an authored coordinate.
//!
//! Usage: `inject_orientation_beacon [fixture.json] [store-dir]`
//!   fixture.json defaults to fixtures/ontology/lumina-prime-orientation-beacon-v0.json
//!   store-dir    defaults to artifacts/ontology-registry/current/store

use std::{collections::BTreeMap, env, error::Error, path::PathBuf};

use universe_core::{EntityKey, RelationKey, Tick};
use universe_e2e::canonical::{canonical_predicate, entity_symbol};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

// The authored -> canonical predicate remap and the subtype-promotion rule are
// shared with the other injectors in `universe_e2e::canonical`, the single
// source of truth. An authored predicate absent from that table is a hard error
// here — the injector never silently mints a non-canonical predicate into the
// canonical store.

// Disjoint key blocks (live store currently tops out at 0x230f).
const ENTITY_BASE: u128 = 0xB000; // beacon space + members, by ordered index
const REL_BASE: u128 = 0xBB00; // included intra-graph relations, by index

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

    // 4. (Removed) The beacon no longer authors a Built pose. Positions are not
    // stored — a node's place is inferred by the layout solver, never written.
    // No `built_position` / `HAS_POSITION` / construction / justification here.

    // 5. Plan symbol interning: node_type symbols + predicates.
    // A node whose subtype is a canonical node-type carries that specific type
    // (subtype-promotion rule shared via `universe_e2e::canonical::entity_symbol`).
    let mut requested: Vec<String> = Vec::new();
    for node in &nodes {
        requested.push(entity_symbol(&node.node_type, &node.subtype));
    }
    for r in &kept {
        requested.push(r.predicate.clone());
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
                symbol: sym(&entity_symbol(&node.node_type, &node.subtype))?,
                content: Some(store.append_content(&content)?),
            },
        });
    }
    // 6b. (Removed) No placement entities — no Built pose is authored.
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
    // 6d. (Removed) No placement relations — no HAS_POSITION / CONSTRUCTED_BY /
    // JUSTIFIED_BY. The beacon carries no authored coordinate.

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

    // 8b. (Removed) No pose to read back — the beacon authors no coordinate.

    println!(
        "\nRESULT: injected the Lumina Prime orientation beacon ({} nodes, {} intra-graph relations) into the LIVE store",
        nodes.len(),
        kept.len()
    );
    println!(
        "        with NO authored Built pose — positions are inferred by the layout solver, never stored."
    );
    println!("        Read the whole subgraph back from a fresh reopen. graph_status: WRITTEN (wiring/runtime still not_wired).");
    Ok(())
}

/// Short tail of a canonical id for compact remap logging.
fn short(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
