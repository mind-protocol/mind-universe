//! Injects the L1 Attention inner tool (a citizen's cognitive faculty that turns
//! a bounded local field into a readable, salience-ordered, budgeted manual
//! frame) into a canonical store as ONE additive, atomic transaction, then
//! INDEPENDENTLY reads the whole subgraph back from a fresh reopen.
//!
//! This is the same LOWER write layer as `inject_energy_pen` /
//! `inject_orientation_beacon`: a hand-built write-set (0 interned symbols +
//! N PutEntity + M PutRelation), committed at a tick boundary. It is NOT the
//! permanent semantic-intent path; it is the bootstrap injector the generic
//! translator generalizes.
//!
//! Difference from the L2 injectors: Attention is an INNER (L1) tool of the
//! citizen, not a sited civic construct. It authors NO Built pose (a coordinate
//! it does not have would be a forged measurement) and NO PART_OF edge (its
//! parent brain Space is not built) — it is a standalone portable inner tool.
//!
//! Epistemic honesty:
//!   * Every member of the portable projection becomes one canonical entity
//!     carrying its original `canonical_id`; no member is silently dropped.
//!   * A relation whose source OR target is absent from the injected id-set is
//!     SKIPPED and reported (none are expected — the graph is self-contained).
//!   * A clean injection interns ZERO new symbols; a non-canonical symbol is a
//!     hard error, never silently minted into the canonical store.
//!   * Readback re-derives the five-precondition attention gate from the
//!     committed `code` node — proving the attention AND-gate survived the
//!     round-trip, not just that some bytes landed.
//!
//! Store selection (honors the operator env, then positional args, then default):
//!   UNIVERSE_STORE   store dir            (arg 2; default artifacts/ontology-registry/current/store)
//!   UNIVERSE_GENESIS genesis snapshot     (arg 3; only used to BOOTSTRAP an empty store)
//!
//! Usage: `inject_attention [fixture.json] [store-dir] [genesis.json]`
//!   fixture.json defaults to fixtures/ontology/attention-l1-v0.json

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

/// The exact five attention preconditions the attention gate must require.
/// Readback re-derives this set from the committed `code` node and fails if it
/// drifted — the load-bearing proof that the gate contract survived injection.
const GATE_SUPPORTS: &[&str] = &[
    "field_to_gate",
    "name_to_gate",
    "faces_to_gate",
    "salience_to_gate",
    "budget_to_gate",
];

// Disjoint key block. Kernel tops out ~0x230f; beacon 0xB000, pen 0xC000. The
// L1 Attention tool takes 0xD000 to avoid every existing block.
const ENTITY_BASE: u128 = 0xD000; // Attention Space + members, by ordered index
const REL_BASE: u128 = 0xDD00; // included intra-graph relations, by index

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
        .unwrap_or_else(|| PathBuf::from("fixtures/ontology/attention-l1-v0.json"));
    // Store dir: UNIVERSE_STORE env wins, then positional arg 2, then default.
    let store_dir = env::var_os("UNIVERSE_STORE")
        .map(PathBuf::from)
        .or_else(|| args.next().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    // Genesis: UNIVERSE_GENESIS env wins, then positional arg 3. Only used to
    // bootstrap an EMPTY store (a store with a committed snapshot ignores it).
    let genesis_path = env::var_os("UNIVERSE_GENESIS")
        .map(PathBuf::from)
        .or_else(|| args.next().map(PathBuf::from));
    println!("fixture  : {}", fixture_path.display());
    println!("store dir: {}", store_dir.display());
    if let Some(genesis) = &genesis_path {
        println!("genesis  : {} (bootstrap fallback for an empty store)", genesis.display());
    }

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
    let attention_key = *id_to_key.get(&root_id).expect("root is indexed");
    println!(
        "nodes to inject: {} (attention space = {:#x})",
        nodes.len(),
        attention_key.0
    );

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
    println!(
        "relations kept: {} | dropped (dangling): {}",
        kept.len(),
        dropped.len()
    );
    for (s, p, t) in &dropped {
        println!("  DROPPED  {s}  -[{p}]->  {t}   (endpoint not in injected set)");
    }

    // 3. Open the store and replay to the authoritative snapshot. If the store is
    // empty AND a genesis was supplied, bootstrap it first (honors UNIVERSE_GENESIS).
    let store = UniverseStore::open(&store_dir)?;
    let base_snapshot = match store.load_snapshot() {
        Ok(snapshot) => snapshot,
        Err(load_error) => {
            let genesis = genesis_path.as_ref().ok_or_else(|| {
                format!(
                    "store has no snapshot ({load_error}) and no UNIVERSE_GENESIS was provided to bootstrap it"
                )
            })?;
            println!("store empty; bootstrapping from genesis {}", genesis.display());
            let seeded = universe_store::load_genesis(genesis)?;
            store.checkpoint(&seeded)?;
            store.load_snapshot()?
        }
    };
    let mut snapshot = store.replay(base_snapshot)?;
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
            return Err(format!(
                "entity key {:#x} ({}) already exists in the store",
                key.0, node.id
            )
            .into());
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

    // 5. Build the atomic write-set: Attention Space + members, then intra edges.
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
        idempotency_key: "mutation:attention-l1:v0".to_string(),
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
    println!(
        "entities: {} | relations: {}",
        after.entities.len(),
        after.relations.len()
    );

    // 7a. Every injected node present, by key + canonical_id, with its authored name.
    println!("\n-- injected node manifest (read back from a fresh reopen) --");
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
        let canonical = content
            .get("canonical_id")
            .and_then(|v| v.as_str())
            .unwrap_or("(none)");
        if canonical != node.id {
            return Err(format!(
                "canonical_id mismatch for {:#x}: {} != {}",
                key.0, canonical, node.id
            )
            .into());
        }
        // The authored name (Space carries a top-level `name`; other faces are
        // named by their subtype). Printed so the manifest is human-legible.
        let name = content
            .pointer("/content/name")
            .and_then(|v| v.as_str())
            .unwrap_or(&node.subtype);
        println!("  {:#06x}  {:<24}  {}", key.0, node.subtype, name);
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

    // 7c. Deep gate-contract check: re-derive the five attention preconditions
    // from the COMMITTED `code` node. This proves the attention AND-gate (the
    // load-bearing mechanism of the construct) survived the round-trip.
    let code_key = *id_to_key
        .get("code:l1:mind-universe:attention-l1-v0")
        .ok_or("code node id absent from fixture")?;
    let code_entity = after
        .entities
        .iter()
        .find(|e| e.key == code_key)
        .ok_or("code node not found on readback")?;
    let code_content =
        fresh.read_content(code_entity.content.as_ref().ok_or("code node has no content")?)?;
    let gate = code_content
        .pointer("/content/attention_atom_circuit/atoms")
        .and_then(|v| v.as_array())
        .ok_or("code node has no attention_atom_circuit.atoms")?
        .iter()
        .find(|a| a.get("key").and_then(|k| k.as_str()) == Some("attention_gate"))
        .ok_or("attention_gate atom not found in committed code node")?;
    let threshold = gate.get("threshold").and_then(|v| v.as_u64()).unwrap_or(0);
    let supports: Vec<&str> = gate
        .get("required_supports")
        .and_then(|v| v.as_array())
        .ok_or("attention_gate has no required_supports")?
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    if supports.len() != 5 {
        return Err(format!(
            "attention gate requires {} supports on readback, expected exactly 5",
            supports.len()
        )
        .into());
    }
    for want in GATE_SUPPORTS {
        if !supports.contains(want) {
            return Err(
                format!("attention gate is missing required precondition '{want}' on readback").into(),
            );
        }
    }
    if threshold != 500 {
        return Err(format!(
            "attention gate threshold is {threshold} on readback, expected 500 (5 x 100 measured preconditions)"
        )
        .into());
    }
    println!(
        "\ngate contract verified from store: attention_gate threshold {threshold}, all 5 preconditions present"
    );

    println!(
        "\nRESULT: injected the L1 Attention inner tool ({} nodes, {} intra-graph relations) into the store",
        nodes.len(),
        kept.len()
    );
    println!("        as one atomic transaction, interned 0 new symbols, and read the whole subgraph back");
    println!("        from a fresh reopen — including the five-precondition attention gate contract.");
    println!("        No Built pose and no PART_OF written: Attention is a standalone L1 inner tool.");
    println!("        graph_status: WRITTEN. This brick AUTHORS the construct only — it is NOT yet wired");
    println!("        to render the citizen's sense (WRITTEN, not RUNNING).");
    Ok(())
}

/// Short tail of a canonical id for compact remap logging.
fn short(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
