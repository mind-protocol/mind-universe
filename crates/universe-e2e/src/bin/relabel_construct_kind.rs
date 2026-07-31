//! Migration: re-label the two constructs already in the LIVE canonical store
//! from the historical `contractKind: "self_verifying_loop"` to the current
//! `contractKind: "construct"`, using the kernel's new `SupersedeEntity`
//! mutation — the ONLY canonical path that revises an entity's content in place.
//!
//! Entity records are append-only by key: `PutEntity` rejects an existing key.
//! `SupersedeEntity` preserves the stable key, requires a strictly greater
//! generation, and therefore leaves every relation that references the entity
//! intact. This migration does NOT rewrite content-0.jsonl by hand and does NOT
//! re-seed; it appends one atomic event to the authoritative log and reads the
//! result back from a fresh reopen.
//!
//! Idempotent: a construct already carrying `contractKind: "construct"` is
//! skipped, and the whole run is a no-op if nothing needs relabeling.
//!
//! Usage: `relabel_construct_kind [store-dir]`
//!   store-dir defaults to artifacts/ontology-registry/current/store

use std::{env, error::Error, path::PathBuf};

use universe_store::{EntityRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};
use universe_core::Tick;

const TARGETS: &[&str] = &[
    "space:l2:lumina-prime:orientation-beacon-v0",
    "space:l2:lumina-prime:energy-pen-v0",
];
const OLD_LABEL: &str = "self_verifying_loop";
const NEW_LABEL: &str = "construct";

fn main() {
    if let Err(error) = run() {
        eprintln!("MIGRATION FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    println!("store dir: {}", store_dir.display());

    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "base revision: {} | entities: {} | relations: {}",
        base_revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );

    // Resolve each target entity by its canonical_id (stored inside content),
    // and build a SupersedeEntity command with the contractKind rewritten.
    let mut commands = Vec::new();
    for cid in TARGETS {
        // Find the entity whose content.canonical_id matches.
        let mut found: Option<&EntityRecord> = None;
        for entity in &snapshot.entities {
            let Some(ptr) = entity.content.as_ref() else {
                continue;
            };
            let content = store.read_content(ptr)?;
            if content.get("canonical_id").and_then(|v| v.as_str()) == Some(cid) {
                found = Some(entity);
                break;
            }
        }
        let entity = found.ok_or_else(|| format!("target {cid} not found in store"))?;
        let mut content = store.read_content(entity.content.as_ref().expect("checked above"))?;

        let current = content
            .pointer("/content/contractKind")
            .and_then(|v| v.as_str())
            .unwrap_or("(none)")
            .to_string();
        if current == NEW_LABEL {
            println!("  SKIP  {cid}  (already contractKind=construct)");
            continue;
        }
        if current != OLD_LABEL {
            return Err(format!(
                "refusing to relabel {cid}: current contractKind is '{current}', expected '{OLD_LABEL}'"
            )
            .into());
        }

        // Rewrite the label in the content projection.
        content
            .get_mut("content")
            .and_then(|c| c.as_object_mut())
            .ok_or_else(|| format!("{cid} content has no inner content object"))?
            .insert(
                "contractKind".to_string(),
                serde_json::Value::String(NEW_LABEL.to_string()),
            );

        let new_content_ref = store.append_content(&content)?;
        let next_generation = entity
            .generation
            .checked_add(1)
            .ok_or("entity generation overflow")?;
        println!(
            "  RELABEL {cid}  gen {} -> {}  ({OLD_LABEL} -> {NEW_LABEL})",
            entity.generation, next_generation
        );
        commands.push(UniverseCommand::SupersedeEntity {
            entity: EntityRecord {
                key: entity.key,
                generation: next_generation,
                symbol: entity.symbol,
                content: Some(new_content_ref),
            },
        });
    }

    if commands.is_empty() {
        println!("\nNothing to relabel — every target already reads contractKind=construct. No event written.");
        return Ok(());
    }

    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "migration:relabel-construct-kind:v0".to_string(),
        causal_ancestry: vec!["changeset:construct-rename-v0".to_string()],
        commands,
    };

    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\ncommitted {command_count} supersede command(s) as one atomic set");
    println!("commit receipt: {receipt:?}");

    // Independent readback from a fresh reopen.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);

    for cid in TARGETS {
        let mut ok = false;
        for entity in &after.entities {
            let Some(ptr) = entity.content.as_ref() else {
                continue;
            };
            let content = fresh.read_content(ptr)?;
            if content.get("canonical_id").and_then(|v| v.as_str()) == Some(cid) {
                let label = content
                    .pointer("/content/contractKind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(none)");
                if label != NEW_LABEL {
                    return Err(format!(
                        "readback: {cid} still reads contractKind='{label}', expected '{NEW_LABEL}'"
                    )
                    .into());
                }
                println!(
                    "  OK  {cid}  contractKind={label}  generation={}",
                    entity.generation
                );
                ok = true;
                break;
            }
        }
        if !ok {
            return Err(format!("readback: target {cid} vanished after migration").into());
        }
    }

    println!(
        "\nRESULT: relabeled {command_count} construct(s) in the LIVE store to contractKind=construct"
    );
    println!("        via SupersedeEntity (append-only event, generation bumped, keys and relations preserved),");
    println!("        and verified the new label from a fresh reopen. Old content remains in the content log.");
    Ok(())
}
