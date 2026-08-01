//! Emits a PROGRESSIVE desktop delta: when a new authoritative batch commits,
//! only the batch's own write-set is streamed as `entity_materialized` /
//! `relation_materialized` frames — never the whole Universe. This is how newly
//! committed Nodes/Assets become visible without a whole-Universe scan.
//!
//! Honest boundary: the delta STREAM is bounded to the batch write-set (this bin
//! knows exactly what it committed, so it enumerates nothing else). The final
//! independent readback DOES replay the store — that is verification, not delta
//! production, and verification legitimately reads the store back.
//!
//! Demo batch 2 over the citizen world: adds one actor (Vega) observing Ledger.

use serde_json::{json, Value};
use std::{env, error::Error, fs, path::PathBuf};
use universe_assets::visual::{derive, VisualCatalog, VisualPolicy};
use universe_core::{EntityKey, RelationKey, Tick};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{
    CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet,
};

const PROTOCOL_VERSION: u16 = 0;
const MICRO: f64 = 1_000_000.0;
const VEGA_ATOM: EntityKey = EntityKey(0xB010);
const VEGA_RELATION: RelationKey = RelationKey(0xB210);
const LEDGER_ATOM: EntityKey = EntityKey(0xB003);
const CHANGE_ID: &str = "citizen-world-batch-2";

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let store_root = args.next().map(PathBuf::from).ok_or(USAGE)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or(USAGE)?;
    let from_sequence: u64 = args
        .next()
        .ok_or(USAGE)?
        .to_str()
        .and_then(|s| s.parse().ok())
        .ok_or("from-sequence must be a non-negative integer")?;
    let catalog_path = args.next().map(PathBuf::from);
    let policy_path = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(USAGE.into());
    }
    let authority = match (catalog_path, policy_path) {
        (Some(catalog), Some(policy)) => Some((
            VisualCatalog::load(&catalog).map_err(|e| e.to_string())?,
            VisualPolicy::load(&policy).map_err(|e| e.to_string())?,
        )),
        (None, None) => None,
        _ => return Err("catalog and policy must be supplied together".into()),
    };
    fs::create_dir_all(&artifact_dir)?;

    let store = UniverseStore::open(&store_root)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let from_revision = snapshot.revision.0;
    let entities_before = snapshot.entities.len();

    // The batch-2 write-set: one new actor observing an existing entity.
    let vega_content = json!({
        "kind": "citizen",
        "semantic_type": "actor",
        "name": "Vega",
        "residency": "hot",
        "epistemic_state": "measured"
    });

    let already = snapshot.event_keys.contains(CHANGE_ID);
    if !already {
        let actor_symbol = snapshot
            .symbol_id("actor")
            .ok_or("citizen store is missing the `actor` symbol")?;
        let observes_symbol = snapshot
            .symbol_id("OBSERVES")
            .ok_or("citizen store is missing the `OBSERVES` symbol")?;
        let content_ref = store.append_content(&vega_content)?;
        let commands = vec![
            UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: VEGA_ATOM,
                    generation: 0,
                    symbol: actor_symbol,
                    content: Some(content_ref),
                },
            },
            UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: VEGA_RELATION,
                    generation: 0,
                    source: VEGA_ATOM,
                    target: LEDGER_ATOM,
                    predicate: observes_symbol,
                    content: None,
                },
            },
        ];
        let transaction = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: snapshot.revision,
                idempotency_key: CHANGE_ID.to_owned(),
                commands,
            },
        )?;
        let tick = Tick(snapshot.tick.0 + 1);
        if matches!(
            transaction.commit(&store, &mut snapshot, tick)?,
            CommitReceipt::AlreadyCommitted { .. }
        ) {
            return Err("batch-2 change key already present mid-run".into());
        }
    }

    // DELTA STREAM — only the batch write-set, nothing else. Independent of the
    // Universe size: exactly one entity upsert and one relation upsert.
    let mut sequence = from_sequence;
    let mut frames: Vec<Value> = Vec::new();

    sequence += 1;
    let (material, embodiment) =
        resolve_visual(Some(&vega_content), authority.as_ref()).map_err(|e| e.to_string())?;
    let mut vega = json!({
        "id": VEGA_ATOM.to_string(),
        "generation": 0,
        "symbol": "actor",
        "content_kind": "citizen",
        "residency": "hot",
        "position_micro": [0, 2_000_000, 0],
        "visual": { "primitive": "unknown", "motion": "still", "material": material },
    });
    if let Some(embodiment) = embodiment {
        vega["embodiment"] = embodiment;
    }
    // Provenance so the renderer resolves Vega's appearance through her producing
    // toolkit's visual binding (an actor → the citizen-energy archetype), never a
    // universal default. Emitted verbatim from her graph content's semantic_type.
    if let Some(semantic_type) = vega_content.get("semantic_type").and_then(Value::as_str) {
        vega["provenance"] = json!({
            "role_axis": semantic_type,
            "semantic_type": semantic_type,
        });
    }
    frames.push(json!({
        "protocol_version": PROTOCOL_VERSION,
        "sequence": sequence,
        "payload": { "message_type": "entity_materialized", "entity": vega }
    }));

    sequence += 1;
    frames.push(json!({
        "protocol_version": PROTOCOL_VERSION,
        "sequence": sequence,
        "payload": {
            "message_type": "relation_materialized",
            "relation": {
                "id": VEGA_RELATION.to_string(),
                "source": VEGA_ATOM.to_string(),
                "target": LEDGER_ATOM.to_string(),
                "predicate": "OBSERVES",
                "visual": {
                    "primitive": "unknown",
                    "color": "#5a6675",
                    "emissive": "#1f2731",
                    "emissive_intensity_micro": 0,
                    "opacity_micro": 500_000,
                    "width_micro": 20_000,
                    "lane_separation_micro": 0
                }
            }
        }
    }));

    fs::write(
        artifact_dir.join("delta-batch2-frames.json"),
        serde_json::to_vec_pretty(&frames)?,
    )?;

    // Independent readback (verification only): the batch is durable and bounded.
    let readback_store = UniverseStore::open(&store_root)?;
    let readback = readback_store.replay(readback_store.load_snapshot()?)?;
    let vega_present = readback.entities.iter().any(|e| e.key == VEGA_ATOM);
    let entities_after = readback.entities.len();

    let manifest = json!({
        "kind": "desktop_world_delta_manifest",
        "batch_id": CHANGE_ID,
        "from_revision": from_revision,
        "to_revision": readback.revision.0,
        "delta_entity_upserts": 1,
        "delta_relation_upserts": 1,
        "delta_frames": frames.len(),
        "whole_universe_in_delta": false,
        "entities_before": entities_before,
        "entities_after": entities_after,
        "vega_present": vega_present,
        "newly_committed": !already,
        "information_status": "measured",
    });
    fs::write(
        artifact_dir.join("delta-batch2-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    println!(
        "world-delta batch={} from_rev={} to_rev={} delta_frames={} entities {}->{} vega_present={} newly_committed={}",
        CHANGE_ID,
        from_revision,
        readback.revision.0,
        frames.len(),
        entities_before,
        entities_after,
        vega_present,
        !already
    );
    Ok(())
}

const USAGE: &str =
    "usage: desktop_world_delta <store-dir> <artifact-dir> <from-sequence> [visual-catalog.json visual-policy.json]";

fn resolve_visual(
    content: Option<&Value>,
    authority: Option<&(VisualCatalog, VisualPolicy)>,
) -> Result<(Value, Option<Value>), universe_core::UniverseError> {
    let (Some(content), Some((catalog, policy))) = (content, authority) else {
        return Ok((neutral_material(), None));
    };
    let Some(semantic_type) = content.get("semantic_type").and_then(Value::as_str) else {
        return Ok((neutral_material(), None));
    };
    let residency = content
        .get("residency")
        .and_then(Value::as_str)
        .unwrap_or("dormant");
    let epistemic = content
        .get("epistemic_state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let Some(resolved) = derive(policy, catalog, semantic_type, residency, epistemic)? else {
        return Ok((neutral_material(), None));
    };
    let material = json!({
        "color": resolved.material.get("color").cloned().unwrap_or(json!("#8a97a8")),
        "emissive": resolved.material.get("emissive").cloned().unwrap_or(json!("#2b3440")),
        "emissive_intensity_micro": micro(resolved.material.get("emissiveIntensity").and_then(Value::as_f64).unwrap_or(0.0)),
        "opacity_micro": micro(resolved.material.get("opacity").and_then(Value::as_f64).unwrap_or(0.7)),
        "scale_micro": 1_000_000,
    });
    let embodiment = json!({
        "source_mapping_id": catalog.authority_id,
        "mapping": catalog.mapping,
        "motion_profile": catalog.motion_profile,
        "residency": residency,
        "sampled_at_ms": 0,
        "resolved_form": resolved.form_name,
        "confident": resolved.confident,
    });
    Ok((material, Some(embodiment)))
}

fn neutral_material() -> Value {
    json!({
        "color": "#8a97a8",
        "emissive": "#2b3440",
        "emissive_intensity_micro": 0,
        "opacity_micro": 700_000,
        "scale_micro": 1_000_000
    })
}

fn micro(value: f64) -> i64 {
    (value * MICRO).round() as i64
}
