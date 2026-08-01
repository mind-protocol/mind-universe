//! Sever the ontology-registry root from the LIVE store — kernel-honestly.
//!
//! The graph wraps every atom inside ONE registry root: the `ontology_manifest`
//! entity, with every other atom joined to it by a `PART_OF` membership edge, so
//! the whole world reads as "encapsulated" inside a single space. The operator
//! asked to remove that root.
//!
//! The kernel has NO entity-delete verb — matter is append-only (see
//! `purge_positions`). So the honest removal severs the EDGES: tombstone every
//! relation incident to the manifest (as source OR target). The manifest atom
//! then survives as an ISOLATED, unreachable orphan — no atom is `PART_OF` it any
//! more, no perception reaches it — the append-only analogue of deletion.
//!
//! In THIS store every `PART_OF` edge targets the manifest, so this removes the
//! entire containment backbone; no sub-nesting is disturbed (there is none).
//!
//! Committed as ONE atomic transaction at a tick boundary, then INDEPENDENTLY
//! read back from a fresh reopen. Idempotent: a second run finds nothing incident
//! and commits nothing.
//!
//! Usage: `sever_registry_root [store-dir]`
//!   store-dir defaults to artifacts/ontology-registry/current/store

use std::collections::BTreeMap;
use std::{env, error::Error, path::PathBuf};

use universe_core::Tick;
use universe_store::UniverseStore;
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const ROOT_KIND: &str = "ontology_manifest";

fn main() {
    if let Err(error) = run() {
        eprintln!("SEVER FAILED: {error}");
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

    // 2. Find the registry root: the entity whose content.kind == ontology_manifest.
    //    Refuse to guess if more than one is present; do nothing if none.
    let mut root = None;
    for entity in &snapshot.entities {
        let Some(pointer) = entity.content.as_ref() else {
            continue;
        };
        let content = store.read_content(pointer)?;
        let kind = content.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if kind == ROOT_KIND {
            let label = content
                .get("canonical_id")
                .and_then(|v| v.as_str())
                .or_else(|| content.get("ontology_id").and_then(|v| v.as_str()))
                .unwrap_or("(none)");
            if let Some(prev) = root {
                return Err(format!(
                    "more than one {ROOT_KIND} present ({:#x} and {:#x}) — refusing to guess which is the root",
                    prev, entity.key.0
                )
                .into());
            }
            println!("registry root: {:#x}  ({label})", entity.key.0);
            root = Some(entity.key.0);
        }
    }
    let Some(root_raw) = root else {
        println!("no `{ROOT_KIND}` entity present — nothing to sever.");
        return Ok(());
    };
    // The EntityKey to compare relation endpoints against.
    let root_key = snapshot
        .entities
        .iter()
        .find(|e| e.key.0 == root_raw)
        .map(|e| e.key)
        .expect("root was just found");

    // 3. Collect every relation incident to the root (source OR target).
    let incident: Vec<_> = snapshot
        .relations
        .iter()
        .filter(|r| r.source == root_key || r.target == root_key)
        .map(|r| (r.key, r.generation, r.predicate))
        .collect();
    if incident.is_empty() {
        println!(
            "root {:#x} has no incident relations — already isolated. Nothing to do.",
            root_raw
        );
        return Ok(());
    }
    // Predicate breakdown, for an attributable record of what is being cut.
    let predname =
        |p: u32| snapshot.symbols.get(p as usize).cloned().unwrap_or_else(|| p.to_string());
    let mut by_pred: BTreeMap<String, usize> = BTreeMap::new();
    for (_, _, p) in &incident {
        *by_pred.entry(predname(*p)).or_default() += 1;
    }
    println!("relations incident to the root: {}", incident.len());
    for (pred, n) in &by_pred {
        println!("  {n:>4} x {pred}");
    }

    // 4. One atomic write-set of TombstoneRelation commands.
    let commands: Vec<UniverseCommand> = incident
        .iter()
        .map(|(key, generation, _)| UniverseCommand::TombstoneRelation {
            relation: *key,
            generation: *generation,
        })
        .collect();
    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "mutation:sever-registry-root:v0".to_string(),
        commands,
    };

    // 5. Prepare + commit as ONE atomic transaction at a tick boundary.
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\ncommitted {command_count} tombstones as one atomic set");
    println!("commit receipt: {receipt:?}");

    // 6. INDEPENDENT readback: fresh reopen. Assert 0 incident remain, and report
    //    that the orphaned root atom still exists (append-only matter — not deleted).
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    let remaining = after
        .relations
        .iter()
        .filter(|r| r.source == root_key || r.target == root_key)
        .count();
    let root_survives = after.entities.iter().any(|e| e.key == root_key);
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);
    println!(
        "relations incident to root {:#x} remaining: {remaining}",
        root_raw
    );
    println!("root atom still present (append-only matter): {root_survives}");
    if remaining != 0 {
        return Err(
            format!("sever incomplete: {remaining} relation(s) still incident to the root").into(),
        );
    }

    println!(
        "\nRESULT: severed {command_count} edge(s) from the registry root {:#x}. No atom is PART_OF",
        root_raw
    );
    println!(
        "        it any more; it is an isolated orphan, unreachable from any perception. The whole"
    );
    println!(
        "        containment backbone is gone (every PART_OF in this store targeted the root)."
    );
    Ok(())
}
