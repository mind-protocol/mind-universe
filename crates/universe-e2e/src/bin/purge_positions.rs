//! Purge authored coordinates from the LIVE store — kernel-honestly.
//!
//! There are no stored coordinates any more (the injectors no longer write a
//! Built pose), but earlier runs left `built_position` poses behind, each hung
//! off a node by a `HAS_POSITION` relation. The kernel has no entity-delete
//! verb — matter is append-only — so a true purge severs the EDGE: tombstone
//! every `HAS_POSITION` relation. A node then no longer *has* a position, every
//! reader (materializer, layout, `sense`) that keys off `HAS_POSITION` sees
//! nothing, and the orphaned `built_position` entities become isolated,
//! unreachable from any perception.
//!
//! It touches ONLY `HAS_POSITION`. Legitimate `CONSTRUCTED_BY` / `JUSTIFIED_BY`
//! edges (a proposal's construction Moment) are left intact.
//!
//! Committed as ONE atomic transaction at a tick boundary, then INDEPENDENTLY
//! read back from a fresh reopen. Idempotent: a second run finds nothing to
//! tombstone and commits nothing.
//!
//! Usage: `purge_positions [store-dir]`
//!   store-dir defaults to artifacts/ontology-registry/current/store

use std::{env, error::Error, path::PathBuf};

use universe_core::Tick;
use universe_store::UniverseStore;
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const PREDICATE: &str = "HAS_POSITION";

fn main() {
    if let Err(error) = run() {
        eprintln!("PURGE FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    println!("store dir: {}", store_dir.display());

    // 1. Open the LIVE store and replay to the authoritative snapshot.
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "base revision: {} | entities: {} | relations: {}",
        base_revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );

    // 2. Resolve the HAS_POSITION predicate symbol. Absent => nothing was ever
    //    placed; there is nothing to purge.
    let Some(predicate) = snapshot.symbols.iter().position(|s| s == PREDICATE) else {
        println!("symbol `{PREDICATE}` is not interned — no authored positions exist. Nothing to purge.");
        return Ok(());
    };
    let predicate = predicate as u32;

    // 3. Collect every HAS_POSITION relation (its key + generation), for tombstone.
    let targets: Vec<_> = snapshot
        .relations
        .iter()
        .filter(|r| r.predicate == predicate)
        .map(|r| (r.key, r.generation, r.source, r.target))
        .collect();
    if targets.is_empty() {
        println!("no HAS_POSITION relations present — already purged. Nothing to do.");
        return Ok(());
    }
    println!("found {} HAS_POSITION relation(s) to tombstone:", targets.len());
    for (key, generation, source, target) in &targets {
        println!(
            "  {:#x} (gen {generation}): {:#x} -[HAS_POSITION]-> {:#x}",
            key.0, source.0, target.0
        );
    }

    // 4. One atomic write-set of TombstoneRelation commands.
    let commands: Vec<UniverseCommand> = targets
        .iter()
        .map(|(key, generation, _, _)| UniverseCommand::TombstoneRelation {
            relation: *key,
            generation: *generation,
        })
        .collect();
    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "mutation:purge-has-position:v0".to_string(),
        commands,
    };

    // 5. Prepare + commit as ONE atomic transaction at a tick boundary.
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\ncommitted {command_count} tombstones as one atomic set");
    println!("commit receipt: {receipt:?}");

    // 6. INDEPENDENT readback: fresh reopen from disk; assert 0 remain.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    let remaining = after
        .relations
        .iter()
        .filter(|r| r.predicate == predicate)
        .count();
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);
    println!("HAS_POSITION relations remaining: {remaining}");
    if remaining != 0 {
        return Err(format!("purge incomplete: {remaining} HAS_POSITION relation(s) still present").into());
    }
    println!(
        "\nRESULT: severed {command_count} HAS_POSITION edge(s). No node in the store carries an authored"
    );
    println!("        coordinate any more; the orphaned built_position poses are unreachable from perception.");
    Ok(())
}
