//! First real bounded import (G2 phase 2): one PostgreSQL Space → inert Atoms.
//!
//! Reads one `graph_id` from the live source (DSN in `MIND_POSTGRES_DSN`) in a
//! read-only transaction and materializes each node as an **inert identity Atom**
//! into a Universe store — preserving the global source id, a canonical row
//! hash, source vocabulary as data, and provenance. It activates no ontology,
//! no predicate, no code; it imports no payload beyond hashes.
//!
//! The import is bounded (fixed batch size), resumable (watermark = the greatest
//! imported source id), idempotent (deterministic per-node keys + per-batch
//! idempotency keys), and independently read back after every batch.
//!
//! Run: `cargo run -p universe-postgres-import --features live-postgres \
//!   --bin postgres_import -- <graph_id|auto> <artifact-dir> [batch] [max_batches]`

use postgres::{Client, NoTls};
use serde_json::{json, Value};
use std::path::PathBuf;
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseId};
use universe_store::{canonical_hash, EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const IMPORT_UNIVERSE: UniverseId = UniverseId(0x5001);
const SYM_SOURCE: &str = "postgres_import_source";
const SYM_IDENTITY: &str = "postgres_source_identity";
const SYM_RECEIPT: &str = "import_receipt";
const SYM_IMPORTS_FROM: &str = "IMPORTS_FROM";
const SYM_HAS_RECEIPT: &str = "HAS_RECEIPT";

type Err = Box<dyn std::error::Error>;

/// Deterministic 128-bit key from a stable string tuple, so re-importing the
/// same node maps to the same Atom (idempotent identity), never a new one.
fn key_u128(parts: &[&str]) -> Result<u128, Err> {
    let hex = canonical_hash(&json!(parts))?;
    Ok(u128::from_str_radix(&hex[..32], 16)?)
}

fn main() -> Result<(), Err> {
    let mut args = std::env::args_os().skip(1);
    let graph_arg = args
        .next()
        .map(|a| a.to_string_lossy().into_owned())
        .ok_or("usage: postgres_import <graph_id|auto> <artifact-dir> [batch] [max_batches]")?;
    let artifact_dir = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: postgres_import <graph_id|auto> <artifact-dir> [batch] [max_batches]")?;
    let batch_limit: i64 = args
        .next()
        .and_then(|a| a.to_string_lossy().parse().ok())
        .unwrap_or(64);
    let max_batches: usize = args
        .next()
        .and_then(|a| a.to_string_lossy().parse().ok())
        .unwrap_or(64);
    std::fs::create_dir_all(&artifact_dir)?;
    let store_root = artifact_dir.join("store");

    let dsn = std::env::var("MIND_POSTGRES_DSN").map_err(|_| "MIND_POSTGRES_DSN is not set")?;
    let authority = format!("postgres:{}", dsn.rsplit('@').next().unwrap_or("unknown"));
    let mut client = Client::connect(&dsn, NoTls)?;
    client.batch_execute("SET default_transaction_read_only = on")?;

    // Resolve the target graph (smallest if "auto") in a read-only transaction.
    let graph_id: String = {
        let mut tx = client.build_transaction().read_only(true).start()?;
        let g = if graph_arg == "auto" {
            tx.query_one(
                "SELECT graph_id FROM mind_nodes GROUP BY graph_id ORDER BY count(*) ASC LIMIT 1",
                &[],
            )?
            .get(0)
        } else {
            graph_arg.clone()
        };
        tx.commit()?;
        g
    };

    let source_atom = EntityKey(key_u128(&["source", &authority, &graph_id])?);

    // Resume: the watermark is the greatest source id already imported.
    let mut cursor: Option<String> = None;
    let mut already_imported = 0usize;
    if store_root.join("snapshot.json").exists() {
        let store = UniverseStore::open(&store_root)?;
        let snapshot = store.replay(store.load_snapshot()?)?;
        let identity_symbol = snapshot.symbol_id(SYM_IDENTITY);
        for entity in &snapshot.entities {
            if Some(entity.symbol) != identity_symbol {
                continue;
            }
            if let Some(content) = entity.content.as_ref() {
                let value = store.read_content(content)?;
                // Watermark is per graph_id: a shared multi-Space store must
                // resume each graph from its own greatest imported id.
                if value.get("graph_id").and_then(Value::as_str) != Some(graph_id.as_str()) {
                    continue;
                }
                if let Some(id) = value.get("source_id").and_then(Value::as_str) {
                    already_imported += 1;
                    if cursor.as_deref().is_none_or(|c| id > c) {
                        cursor = Some(id.to_owned());
                    }
                }
            }
        }
    }

    let mut total_imported = 0usize;
    let mut readback_ok = 0usize;
    let mut batches = 0usize;
    let mut batch_reports = Vec::new();

    for _ in 0..max_batches {
        // Bounded, ordered, resumable read of the next batch.
        let mut tx = client.build_transaction().read_only(true).start()?;
        let rows = tx.query(
            "SELECT id, node_type, subtype, name, status, content, properties, revision, \
                    graph_id, space_id, created_at::text, updated_at::text \
             FROM mind_nodes \
             WHERE graph_id = $1 AND ($2::text IS NULL OR id > $2) \
             ORDER BY id ASC LIMIT $3",
            &[&graph_id, &cursor, &batch_limit],
        )?;
        tx.commit()?;
        if rows.is_empty() {
            break;
        }

        let input_cursor = cursor.clone();
        let mut node_ids = Vec::new();
        let mut identity_atoms: Vec<(EntityKey, String, Value)> = Vec::new();
        for row in &rows {
            let id: String = row.get(0);
            let node_type: String = row.get(1);
            let subtype: Option<String> = row.get(2);
            let name: Option<String> = row.get(3);
            let status: Option<String> = row.get(4);
            let content: Option<String> = row.get(5);
            let properties: Option<Value> = row.get(6);
            let revision: i64 = row.get(7);
            let row_graph: String = row.get(8);
            let space_id: Option<String> = row.get(9);
            let created_at: Option<String> = row.get(10);
            let updated_at: Option<String> = row.get(11);
            let properties = properties.unwrap_or(Value::Null);

            // Faithful canonical hash over the whole row.
            let row_value = json!({
                "id": id, "node_type": node_type, "subtype": subtype, "name": name,
                "status": status, "content": content, "properties": properties,
                "revision": revision, "graph_id": row_graph, "space_id": space_id,
                "created_at": created_at, "updated_at": updated_at,
            });
            let row_sha256 = canonical_hash(&row_value)?;
            let properties_sha256 = canonical_hash(&properties)?;
            let key = EntityKey(key_u128(&["identity", &authority, &graph_id, &id])?);
            // Inert identity: source vocabulary is preserved as DATA, never
            // activated; no ontology/predicate/code binding, nothing executable.
            let content_value = json!({
                "kind": "postgres_source_identity",
                "source_authority": authority,
                "graph_id": row_graph,
                "source_id": id,
                "node_type_source": node_type,
                "subtype_source": subtype,
                "status_source": status,
                "source_revision": revision,
                "row_sha256": row_sha256,
                "properties_sha256": properties_sha256,
                "content_present": content.is_some(),
                "imported_inert": true,
                "ontology_activated": false,
                "executable": false,
                "status_is_source_data": true,
            });
            node_ids.push(id.clone());
            identity_atoms.push((key, id, content_value));
        }
        let last_id = node_ids.last().cloned().expect("non-empty batch");

        let receipt_atom =
            EntityKey(key_u128(&["receipt", &graph_id, input_cursor.as_deref().unwrap_or("start")])?);
        let idempotency_key = format!(
            "live-import:{graph_id}:after:{}",
            input_cursor.as_deref().unwrap_or("start")
        );

        // Commit the batch (idempotent: a re-run whose key is present skips).
        let store = UniverseStore::open(&store_root)?;
        let mut snapshot = if store_root.join("snapshot.json").exists() {
            store.replay(store.load_snapshot()?)?
        } else {
            store.install_seed(&universe_store::GraphSeed {
                universe: IMPORT_UNIVERSE,
                symbols: vec![],
                entities: vec![],
                relations: vec![],
            })?
        };

        if !snapshot.event_keys.contains(&idempotency_key) {
            let plan = snapshot.plan_symbol_interning(
                &[SYM_SOURCE, SYM_IDENTITY, SYM_RECEIPT, SYM_IMPORTS_FROM, SYM_HAS_RECEIPT]
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
            )?;
            let sym = |name: &str| -> Result<u32, Err> {
                plan.assignments
                    .get(name)
                    .copied()
                    .ok_or_else(|| format!("symbol {name} not planned").into())
            };

            let mut commands = Vec::new();
            if !plan.additions.is_empty() {
                commands.push(UniverseCommand::InternSymbols {
                    symbols: plan.additions.clone(),
                });
            }
            // Provenance root, created once.
            if !snapshot.entities.iter().any(|e| e.key == source_atom) {
                let source_ref = store.append_content(&json!({
                    "kind": "postgres_import_source",
                    "authority_id": authority,
                    "graph_id": graph_id,
                    "read_only": true,
                    "credentials_stored": false,
                    "transport": "read_only_transaction_env_dsn",
                }))?;
                commands.push(UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: source_atom,
                        generation: 0,
                        symbol: sym(SYM_SOURCE)?,
                        content: Some(source_ref),
                    },
                });
            }
            for (key, id, content_value) in &identity_atoms {
                let content_ref = store.append_content(content_value)?;
                commands.push(UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: *key,
                        generation: 0,
                        symbol: sym(SYM_IDENTITY)?,
                        content: Some(content_ref),
                    },
                });
                commands.push(UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: RelationKey(key_u128(&["imports-from", &authority, &graph_id, id])?),
                        generation: 0,
                        source: *key,
                        target: source_atom,
                        predicate: sym(SYM_IMPORTS_FROM)?,
                        content: None,
                    },
                });
            }
            let receipt_ref = store.append_content(&json!({
                "kind": "import_receipt",
                "graph_id": graph_id,
                "input_cursor": input_cursor,
                "next_cursor": last_id,
                "imported": identity_atoms.len(),
                "information_status": "measured",
                "ontology_activated": false,
                "executable_nodes": 0,
            }))?;
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: receipt_atom,
                    generation: 0,
                    symbol: sym(SYM_RECEIPT)?,
                    content: Some(receipt_ref),
                },
            });
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(key_u128(&["has-receipt", &graph_id, &last_id])?),
                    generation: 0,
                    source: source_atom,
                    target: receipt_atom,
                    predicate: sym(SYM_HAS_RECEIPT)?,
                    content: None,
                },
            });

            let transaction = UniverseTransaction::prepare(
                &snapshot,
                UniverseWriteSet {
                    base_revision: snapshot.revision,
                    idempotency_key,
                    causal_ancestry: vec![format!("live-import:{graph_id}")],
                    commands,
                },
            )?;
            let tick = Tick(snapshot.tick.0 + 1);
            transaction.commit(&store, &mut snapshot, tick)?;
            // Checkpoint after every batch so the next load_snapshot+replay only
            // applies this batch's events, keeping the import linear rather than
            // quadratic across a large Space.
            store.checkpoint(&snapshot)?;
        }

        // Independent readback: reopen the store and verify every node landed
        // with a matching row hash.
        let rb_store = UniverseStore::open(&store_root)?;
        let rb = rb_store.replay(rb_store.load_snapshot()?)?;
        // Index by key once so each atom's readback is O(1), not a linear scan
        // of the whole (growing) store.
        let rb_index: std::collections::HashMap<EntityKey, &universe_store::EntityRecord> =
            rb.entities.iter().map(|e| (e.key, e)).collect();
        let mut batch_ok = 0usize;
        for (key, _id, content_value) in &identity_atoms {
            let ok = rb_index
                .get(key)
                .and_then(|e| e.content.as_ref())
                .map(|c| rb_store.read_content(c))
                .transpose()?
                .and_then(|stored| {
                    stored.get("row_sha256").and_then(Value::as_str).map(|s| s.to_owned())
                })
                == content_value.get("row_sha256").and_then(Value::as_str).map(|s| s.to_owned());
            if ok {
                batch_ok += 1;
            }
        }
        if batch_ok != identity_atoms.len() {
            return Err(format!(
                "batch readback mismatch: {batch_ok}/{} verified",
                identity_atoms.len()
            )
            .into());
        }

        total_imported += identity_atoms.len();
        readback_ok += batch_ok;
        batches += 1;
        batch_reports.push(json!({
            "input_cursor": input_cursor,
            "next_cursor": last_id,
            "imported": identity_atoms.len(),
            "readback_ok": batch_ok,
        }));
        cursor = Some(last_id);
        if rows.len() < batch_limit as usize {
            break;
        }
    }

    let evidence = json!({
        "kind": "postgres_identity_import_evidence",
        "graph_id": graph_id,
        "authority_id": authority,
        "universe": IMPORT_UNIVERSE,
        "already_imported_before_run": already_imported,
        "imported_this_run": total_imported,
        "readback_ok": readback_ok,
        "batches": batches,
        "final_watermark": cursor,
        "batch_limit": batch_limit,
        "ontology_activated": false,
        "executable_nodes": 0,
        "read_only_source": true,
        "credentials_stored": false,
        "batch_reports": batch_reports,
    });
    std::fs::write(
        artifact_dir.join("import-evidence.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!(
        "live import: graph={graph_id} imported_this_run={total_imported} readback_ok={readback_ok} batches={batches} watermark={:?} ontology_activated=false executable=0",
        cursor
    );
    Ok(())
}
