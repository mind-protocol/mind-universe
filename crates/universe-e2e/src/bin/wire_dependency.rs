//! Wire a canonical relation from one or more source entities to a single target,
//! by their canonical_ids, into the LIVE store. Additive; skips edges that already
//! exist; the predicate must be an already-interned canonical symbol (0 new
//! symbols). Independent readback confirms each edge.
//!
//! Used to generalize a shared contract: e.g. wire every construct's space node
//! to the shared validity toolkit via DEPENDS_ON.
//!
//! Usage: `wire_dependency <PREDICATE> <target-canonical-id> <source-canonical-id...> [--store <dir>]`

use std::{collections::BTreeMap, env, error::Error, path::PathBuf};

use universe_core::{EntityKey, RelationKey, Tick};
use universe_store::{RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// Start of this bin's relation-key block. Keys are allocated AFTER the highest
/// key already used in the block: `PutRelation` upserts, so a fixed `BASE + i`
/// would silently overwrite edges a previous run wired here.
const REL_BASE: u128 = 0x00F0_0000;
const REL_BLOCK_END: u128 = 0x0100_0000;

fn main() {
    if let Err(error) = run() {
        eprintln!("WIRE FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut positional: Vec<String> = Vec::new();
    let mut store_dir = PathBuf::from("artifacts/ontology-registry/current/store");
    let mut content_json: Option<String> = None;
    let mut args = env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--store" => store_dir = PathBuf::from(args.next().ok_or("--store needs a value")?),
            // Edge content. An edge that asserts something (a conformance CLAIM,
            // say) should carry what it asserts and how well it is founded,
            // rather than leaving a reader to infer it from the predicate.
            "--content" => content_json = Some(args.next().ok_or("--content needs a value")?),
            _ => positional.push(a),
        }
    }
    if positional.len() < 3 {
        return Err("usage: wire_dependency <PREDICATE> <target-id> <source-id...> [--store dir]".into());
    }
    let predicate = positional[0].clone();
    let target_id = positional[1].clone();
    let source_ids = &positional[2..];
    println!("predicate: {predicate}\ntarget   : {target_id}\nsources  : {}", source_ids.len());

    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;

    // Resolve canonical_id -> key by scanning entity content.
    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    for entity in &snapshot.entities {
        if let Some(ptr) = entity.content.as_ref() {
            let content = store.read_content(ptr)?;
            if let Some(cid) = content.get("canonical_id").and_then(|v| v.as_str()) {
                id_to_key.insert(cid.to_string(), entity.key);
            }
        }
    }
    let target_key = *id_to_key.get(&target_id).ok_or_else(|| format!("target {target_id} not in store"))?;
    let predicate_symbol = snapshot
        .symbol_id(&predicate)
        .ok_or_else(|| format!("predicate '{predicate}' is not an interned canonical symbol"))?;

    let edge_content = match content_json.as_deref() {
        Some(raw) => {
            let value: serde_json::Value = serde_json::from_str(raw)?;
            println!("edge content: {value}");
            Some(store.append_content(&value)?)
        }
        None => None,
    };

    // Allocate after the highest key already used in this block, never at a
    // fixed offset: a second run must not overwrite the first run's edges.
    let mut next_key = snapshot
        .relations
        .iter()
        .map(|r| r.key.0)
        .filter(|k| (REL_BASE..REL_BLOCK_END).contains(k))
        .max()
        .map(|k| k + 1)
        .unwrap_or(REL_BASE);
    println!("next free relation key in block: {next_key:#x}");

    let mut commands = Vec::new();
    let mut planned: Vec<(EntityKey, String)> = Vec::new();
    for sid in source_ids.iter() {
        let source_key = *id_to_key.get(sid).ok_or_else(|| format!("source {sid} not in store"))?;
        let exists = snapshot.relations.iter().any(|r| {
            r.source == source_key && r.target == target_key && r.predicate == predicate_symbol
        });
        if exists {
            println!("  SKIP  {sid}  (edge already present)");
            continue;
        }
        println!("  WIRE  {sid}  --{predicate}-->  {target_id}   ({next_key:#x})");
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(next_key),
                generation: 0,
                source: source_key,
                target: target_key,
                predicate: predicate_symbol,
                content: edge_content.clone(),
            },
        });
        next_key += 1;
        planned.push((source_key, sid.clone()));
    }

    if commands.is_empty() {
        println!("\nNothing to wire — every edge already present. No event written.");
        return Ok(());
    }
    let count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: format!("wire-dependency:{predicate}:{target_id}:{}", snapshot.tick.0),
        commands,
    };
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\ncommitted {count} edge(s): {receipt:?}");

    // Independent readback.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    for (source_key, sid) in &planned {
        let present = after.relations.iter().any(|r| {
            r.source == *source_key && r.target == target_key && r.predicate == predicate_symbol
        });
        if !present {
            return Err(format!("edge from {sid} missing on readback").into());
        }
    }
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);
    println!("RESULT: wired {count} {predicate} edge(s) to {target_id}; readback OK.");
    Ok(())
}
