//! Injects the Lumina Prime Energy Pen loop (the Stylo énergétique — a portable
//! construct inscription loop) into the LIVE canonical store as ONE
//! additive, atomic transaction, then INDEPENDENTLY reads the whole subgraph
//! back from a fresh reopen.
//!
//! This is the same LOWER write layer as `inject_orientation_beacon`: a
//! hand-built write-set (0 interned symbols +
//! N PutEntity + M PutRelation), committed at a tick boundary. It is NOT the
//! permanent semantic-intent path; it is the bootstrap injector the generic
//! translator generalizes.
//!
//! Difference from the beacon injector: the Pen is a sensemaking INSTRUMENT, not
//! a sited civic monument. It has no measured civic position, so this injector
//! writes NO Built pose — inventing a coordinate would forge a measurement the
//! Pen does not have. The Pen authors Inscriptions that are later placed
//! elsewhere; the Pen itself is placement-free.
//!
//! Epistemic honesty:
//!   * Every member of the portable projection becomes one canonical entity
//!     carrying its original `canonical_id`; no member is silently dropped.
//!   * A relation whose source OR target is absent from the injected id-set is
//!     SKIPPED and reported (the parent-city `PART_OF` edge is dropped, not
//!     dangled — the city is not yet built).
//!   * A clean injection interns ZERO new symbols; a non-canonical symbol is a
//!     hard error, never silently minted into the canonical store.
//!   * Readback re-derives the twelve-support seal contract from the committed
//!     `code` node — proving the seal gate survived the round-trip, not just
//!     that some bytes landed.
//!
//! Usage: `inject_energy_pen [fixture.json] [store-dir]`
//!   fixture.json defaults to fixtures/ontology/lumina-prime-energy-pen-v0.json
//!   store-dir    defaults to artifacts/ontology-registry/current/store

use std::{collections::BTreeMap, env, error::Error, path::PathBuf};

use universe_core::{EntityKey, RelationKey, Tick};
use universe_e2e::canonical::{canonical_predicate, entity_symbol};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

// The authored -> canonical predicate remap and the subtype-promotion rule are
// shared with the other injectors in `universe_e2e::canonical`, the single
// source of truth. An authored predicate absent from that table is a hard error
// here — the injector never mints a non-canonical predicate into the canonical
// store.

/// The exact twelve seal supports the SealGate must require. Readback re-derives
/// this set from the committed `code` node and fails if it drifted.
const SEAL_SUPPORTS: &[&str] = &[
    "gesture_to_seal",
    "claims_to_seal",
    "epistemic_to_seal",
    "sources_to_seal",
    "justifications_to_seal",
    "context_to_seal",
    "authors_to_seal",
    "targets_to_seal",
    "surface_to_seal",
    "broadcast_to_seal",
    "reader_rules_to_seal",
    "signatures_to_seal",
];

// Disjoint key block. Kernel tops out ~0x230f; beacon occupies 0xB000..=0xBBF2.
const ENTITY_BASE: u128 = 0xC000; // Pen Space + members, by ordered index
const REL_BASE: u128 = 0xCC00; // included intra-graph relations, by index

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
        .unwrap_or_else(|| PathBuf::from("fixtures/ontology/lumina-prime-energy-pen-v0.json"));
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

    struct Node {
        id: String,
        node_type: String,
        subtype: String,
        content: serde_json::Value,
    }
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
    let mut nodes: Vec<Node> = Vec::new();
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
    let pen_key = *id_to_key.get(&root_id).expect("root is indexed");
    println!("nodes to inject: {} (pen space = {:#x})", nodes.len(), pen_key.0);

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

    // 4. Plan symbol interning: node_type symbols + remapped predicates only.
    // Subtype-promotion rule shared via `universe_e2e::canonical::entity_symbol`.
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
    // Canonical-conformance guard: a clean injection must intern NOTHING new.
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

    // 5. Build the atomic write-set: Pen Space + members, then intra-graph edges.
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
                symbol: sym(&entity_symbol(&node.node_type, &node.subtype))?,
                content: Some(store.append_content(&content)?),
            },
        });
    }
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

    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "mutation:lumina-energy-pen:v0".to_string(),
        causal_ancestry: vec!["changeset:lumina-energy-pen-v0".to_string()],
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

    // 7a. Every injected node present, by key + canonical_id.
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

    // 7b. Every kept relation present by exact (source, target, predicate).
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

    // 7c. Deep seal-contract check: re-derive the twelve seal supports from the
    // COMMITTED `code` node. This proves the SealGate survived the round-trip.
    let code_key = *id_to_key
        .get("code:l2:lumina-prime:energy-pen-v0")
        .ok_or("code node id absent from fixture")?;
    let code_entity = after
        .entities
        .iter()
        .find(|e| e.key == code_key)
        .ok_or("code node not found on readback")?;
    let code_content = fresh.read_content(code_entity.content.as_ref().ok_or("code node has no content")?)?;
    let seal_gate = code_content
        .pointer("/content/seal_atom_circuit/atoms")
        .and_then(|v| v.as_array())
        .ok_or("code node has no seal_atom_circuit.atoms")?
        .iter()
        .find(|a| a.get("key").and_then(|k| k.as_str()) == Some("seal_gate"))
        .ok_or("seal_gate atom not found in committed code node")?;
    let threshold = seal_gate.get("threshold").and_then(|v| v.as_u64()).unwrap_or(0);
    let supports: Vec<&str> = seal_gate
        .get("required_supports")
        .and_then(|v| v.as_array())
        .ok_or("seal_gate has no required_supports")?
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    if supports.len() != 12 {
        return Err(format!("seal gate requires {} supports on readback, expected exactly 12", supports.len()).into());
    }
    for want in SEAL_SUPPORTS {
        if !supports.contains(want) {
            return Err(format!("seal gate is missing required support bond '{want}' on readback").into());
        }
    }
    if threshold != 1200 {
        return Err(format!("seal gate threshold is {threshold} on readback, expected 1200 (12 x 100 measured supports)").into());
    }
    println!("seal contract verified from store: seal_gate threshold {threshold}, all 12 supports present");

    println!(
        "\nRESULT: injected the Lumina Prime Energy Pen ({} nodes, {} intra-graph relations) into the LIVE store",
        nodes.len(),
        kept.len()
    );
    println!("        as one atomic transaction, interned 0 new symbols, and read the whole subgraph back");
    println!("        from a fresh reopen — including the twelve-support SealGate contract.");
    println!("        No Built pose written: the Pen is a placement-free sensemaking instrument.");
    println!("        graph_status: WRITTEN. wiring/runtime/health remain not_wired / not_running / not_measured.");
    Ok(())
}

/// Short tail of a canonical id for compact remap logging.
fn short(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
