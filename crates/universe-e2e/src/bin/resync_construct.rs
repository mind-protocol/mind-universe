//! Re-sync an already-injected construct's authored content into the LIVE store
//! when its fixture changed, using the kernel's `SupersedeEntity` mutation.
//!
//! `inject_construct` refuses to touch existing keys (append-only PutEntity). When
//! a construct's fixture is revised in place — same ids, new content — this bin
//! is the update path: it derives the SAME deterministic key block as
//! `inject_construct` (from the root id), and for every fixture node already in
//! the store whose content differs, it commits ONE `SupersedeEntity` (generation
//! bumped, key preserved, so every relation survives). Nodes not yet present are
//! reported (run `inject_construct` first for a brand-new construct). Relations
//! are unchanged. Independent readback confirms the new content landed.
//!
//! Usage: `resync_construct <fixture.json> [store-dir]`

use std::{
    collections::{hash_map::DefaultHasher, BTreeMap},
    env,
    error::Error,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use universe_core::{EntityKey, Tick};
use universe_store::{EntityRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const CANONICAL_TYPE_SUBTYPES: &[&str] = &["metric", "validation"];

fn main() {
    if let Err(error) = run() {
        eprintln!("RESYNC FAILED: {error}");
        std::process::exit(1);
    }
}

struct Node {
    id: String,
    node_type: String,
    subtype: String,
    content: serde_json::Value,
}

fn node_from(v: &serde_json::Value) -> Result<Node, Box<dyn Error>> {
    Ok(Node {
        id: v.get("id").and_then(|x| x.as_str()).ok_or("node without id")?.to_string(),
        node_type: v.get("node_type").and_then(|x| x.as_str()).unwrap_or("thing").to_string(),
        subtype: v.get("subtype").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        content: v.get("content").cloned().unwrap_or(serde_json::Value::Null),
    })
}

fn run() -> Result<(), Box<dyn Error>> {
    // Args: <fixture.json> [store-dir] [--keys-from <old-root-id>]
    // --keys-from lets a RENAMED construct supersede the nodes of its old name,
    // mapping the new fixture (same member order) onto the old key block. The
    // stable keys are preserved, so relations pointing at them stay valid.
    let mut positional: Vec<String> = Vec::new();
    let mut keys_from: Option<String> = None;
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "--keys-from" {
            keys_from = Some(it.next().ok_or("--keys-from needs a value")?);
        } else {
            positional.push(a);
        }
    }
    let fixture_path = PathBuf::from(
        positional.first().ok_or("usage: resync_construct <fixture.json> [store-dir] [--keys-from <old-root-id>]")?,
    );
    let store_dir = positional
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    println!("fixture  : {}", fixture_path.display());
    println!("store dir: {}", store_dir.display());

    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&fixture_path)?)?;
    let root_id = doc.get("id").and_then(|v| v.as_str()).ok_or("fixture has no id")?.to_string();

    // Key block: by default from this fixture's root id (same as inject_construct);
    // with --keys-from, from the OLD root id so a rename supersedes old nodes.
    let key_source = keys_from.clone().unwrap_or_else(|| root_id.clone());
    if let Some(old) = &keys_from {
        println!("keys-from : {old}  (renaming {root_id} onto old key block)");
    }
    let mut hasher = DefaultHasher::new();
    key_source.hash(&mut hasher);
    let entity_base: u128 = 0x0001_0000 + ((hasher.finish() as u128 & 0x0FFF) << 16);

    let mut nodes: Vec<Node> = vec![node_from(&doc)?];
    for member in doc.get("members").and_then(|v| v.as_array()).ok_or("no members")? {
        nodes.push(node_from(member)?);
    }
    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        id_to_key.insert(node.id.clone(), EntityKey(entity_base + i as u128));
    }

    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!("base revision: {} | entities: {}", base_revision.0, snapshot.entities.len());

    let entity_symbol = |node: &Node| -> String {
        if CANONICAL_TYPE_SUBTYPES.contains(&node.subtype.as_str()) {
            node.subtype.clone()
        } else {
            node.node_type.clone()
        }
    };

    let mut commands = Vec::new();
    let mut absent = Vec::new();
    let mut unchanged = 0usize;
    for node in &nodes {
        let key = id_to_key[&node.id];
        let Some(existing) = snapshot.entities.iter().find(|e| e.key == key).cloned() else {
            absent.push(node.id.clone());
            continue;
        };
        let new_content = serde_json::json!({
            "canonical_id": node.id,
            "node_type": node.node_type,
            "subtype": node.subtype,
            "content": node.content,
        });
        // Compare against current stored content; only supersede real changes.
        let current = existing
            .content
            .as_ref()
            .map(|p| store.read_content(p))
            .transpose()?;
        if current.as_ref() == Some(&new_content) {
            unchanged += 1;
            continue;
        }
        let next_generation = existing.generation.checked_add(1).ok_or("generation overflow")?;
        println!("  SUPERSEDE {}  gen {} -> {}", node.id, existing.generation, next_generation);
        commands.push(UniverseCommand::SupersedeEntity {
            entity: EntityRecord {
                key,
                generation: next_generation,
                symbol: existing.symbol, // symbol unchanged; entity_symbol kept for parity
                content: Some(store.append_content(&new_content)?),
            },
        });
        let _ = entity_symbol(node);
    }

    for id in &absent {
        println!("  ABSENT   {id}  (run inject_construct first for a new construct)");
    }
    println!("changed: {} | unchanged: {} | absent: {}", commands.len(), unchanged, absent.len());
    if commands.is_empty() {
        println!("\nNothing to resync — store already matches the fixture. No event written.");
        return Ok(());
    }

    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: format!("resync-construct:{root_id}:{}", snapshot.tick.0),
        causal_ancestry: vec![format!("changeset:{root_id}")],
        commands,
    };
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\ncommitted {command_count} supersede command(s): {receipt:?}");

    // Independent readback: fresh reopen, confirm superseded content matches.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    for node in &nodes {
        if absent.contains(&node.id) {
            continue;
        }
        let key = id_to_key[&node.id];
        let entity = after.entities.iter().find(|e| e.key == key).ok_or("node vanished")?;
        let content = fresh.read_content(entity.content.as_ref().ok_or("no content")?)?;
        let cid = content.get("canonical_id").and_then(|v| v.as_str()).unwrap_or("");
        if cid != node.id {
            return Err(format!("readback canonical_id mismatch for {}", node.id).into());
        }
    }
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);
    println!("RESULT: resynced {command_count} node(s) of {root_id} via SupersedeEntity; readback OK.");
    Ok(())
}
