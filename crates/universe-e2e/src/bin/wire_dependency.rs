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

const REL_BASE: u128 = 0x00F0_0000;

fn main() {
    if let Err(error) = run() {
        eprintln!("WIRE FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut positional: Vec<String> = Vec::new();
    let mut store_dir = PathBuf::from("artifacts/ontology-registry/current/store");
    let mut args = env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        if a == "--store" {
            store_dir = PathBuf::from(args.next().ok_or("--store needs a value")?);
        } else {
            positional.push(a);
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

    let mut commands = Vec::new();
    let mut planned: Vec<(EntityKey, String)> = Vec::new();
    for (i, sid) in source_ids.iter().enumerate() {
        let source_key = *id_to_key.get(sid).ok_or_else(|| format!("source {sid} not in store"))?;
        let exists = snapshot.relations.iter().any(|r| {
            r.source == source_key && r.target == target_key && r.predicate == predicate_symbol
        });
        if exists {
            println!("  SKIP  {sid}  (edge already present)");
            continue;
        }
        println!("  WIRE  {sid}  --{predicate}-->  {target_id}");
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(REL_BASE + i as u128),
                generation: 0,
                source: source_key,
                target: target_key,
                predicate: predicate_symbol,
                content: None,
            },
        });
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
        causal_ancestry: vec![format!("changeset:wire:{predicate}:{target_id}")],
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
