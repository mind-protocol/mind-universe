//! Projects a real Universe store into the Mind Desktop wire protocol so the
//! renderer can display the bounded situation ACTUALLY held in the store —
//! Nodes, Assets, and relations — instead of a hand-authored fixture.
//!
//! The desktop protocol already carries a `snapshot` payload, but until now the
//! app's adapter kept only the revision and discarded the entities/relations,
//! and raw `EntityRecord`s (symbol id + content pointer) are not renderable
//! anyway. This bin resolves each entity's symbol name + content kind by replay
//! and emits app-ready `entity_materialized` / `relation_materialized` frames.
//!
//! Visuals are resolved from GRAPH AUTHORITY when a visual-mapping catalog +
//! policy are supplied: an entity that DECLARES its `semantic_type` gets its
//! `embodiment` (the materialized `visual-embodiment/1` mapping) plus an
//! epistemic-modulated material, exactly as the app validates it. An entity that
//! declares no semantic type — or when no authority is supplied — stays the
//! honest `unknown` visual; positions are always a deterministic Fibonacci-sphere
//! LAYOUT, never a physics residency (which is `not_measured` for layout).

use serde_json::{json, Value};
use std::{env, error::Error, fs, path::PathBuf};
use universe_assets::visual::{derive, VisualCatalog, VisualPolicy};
use universe_store::UniverseStore;

const PROTOCOL_VERSION: u16 = 0;
const LAYOUT_RADIUS: f64 = 6.0;
const MICRO: f64 = 1_000_000.0;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let store_root = args.next().map(PathBuf::from).ok_or(USAGE)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or(USAGE)?;
    // Optional graph-authority visual resolution.
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
    let snapshot = store.replay(store.load_snapshot()?)?;

    let entity_count = snapshot.entities.len();
    let mut frames: Vec<Value> = Vec::new();
    let mut sequence: u64 = 1;
    let mut resolved_embodiments = 0usize;

    frames.push(json!({
        "protocol_version": PROTOCOL_VERSION,
        "sequence": sequence,
        "payload": { "message_type": "snapshot", "revision": snapshot.revision.0 },
    }));

    for (index, entity) in snapshot.entities.iter().enumerate() {
        sequence += 1;
        let symbol = snapshot
            .symbols
            .get(entity.symbol as usize)
            .cloned()
            .unwrap_or_else(|| "<unknown-symbol>".to_owned());
        let content = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok());
        let kind = content
            .as_ref()
            .and_then(|c| c.get("kind").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "<no-content>".to_owned());
        let residency = content
            .as_ref()
            .and_then(|c| {
                c.get("residency")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "not_measured".to_owned());

        let (material, embodiment) =
            resolve_visual(content.as_ref(), authority.as_ref()).map_err(|e| e.to_string())?;
        if embodiment.is_some() {
            resolved_embodiments += 1;
        }

        let [x, y, z] = fibonacci_layout(index, entity_count);
        let mut entity_value = json!({
            "id": entity.key.to_string(),
            "generation": entity.generation,
            "symbol": symbol,
            "content_kind": kind,
            "residency": residency,
            "position_micro": [micro(x), micro(y), micro(z)],
            "visual": { "primitive": "unknown", "motion": "still", "material": material },
        });
        if let Some(embodiment) = embodiment {
            entity_value["embodiment"] = embodiment;
        }
        frames.push(json!({
            "protocol_version": PROTOCOL_VERSION,
            "sequence": sequence,
            "payload": { "message_type": "entity_materialized", "entity": entity_value }
        }));
    }

    for relation in &snapshot.relations {
        sequence += 1;
        let predicate = snapshot
            .symbols
            .get(relation.predicate as usize)
            .cloned()
            .unwrap_or_else(|| "<unknown-predicate>".to_owned());
        frames.push(json!({
            "protocol_version": PROTOCOL_VERSION,
            "sequence": sequence,
            "payload": {
                "message_type": "relation_materialized",
                "relation": {
                    "id": relation.key.to_string(),
                    "source": relation.source.to_string(),
                    "target": relation.target.to_string(),
                    "predicate": predicate,
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
    }

    let relation_count = snapshot.relations.len();
    let frames_path = artifact_dir.join("world-snapshot-frames.json");
    fs::write(&frames_path, serde_json::to_vec_pretty(&frames)?)?;
    let readback: Vec<Value> = serde_json::from_slice(&fs::read(&frames_path)?)?;
    let readback_ok = readback.len() == frames.len();

    let manifest = json!({
        "kind": "desktop_world_snapshot_manifest",
        "store_root": store_root.to_string_lossy(),
        "universe": snapshot.universe.to_string(),
        "revision": snapshot.revision.0,
        "entity_frames": entity_count,
        "relation_frames": relation_count,
        "total_frames": frames.len(),
        "readback_ok": readback_ok,
        "visual_authority_resolved": authority.is_some(),
        "resolved_embodiments": resolved_embodiments,
        "residency_measured": false,
        "information_status": "measured",
    });
    fs::write(
        artifact_dir.join("world-snapshot-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    println!(
        "world-snapshot universe={} revision={} entities={} relations={} frames={} embodiments={} authority={} readback_ok={}",
        snapshot.universe,
        snapshot.revision.0,
        entity_count,
        relation_count,
        frames.len(),
        resolved_embodiments,
        authority.is_some(),
        readback_ok
    );
    Ok(())
}

const USAGE: &str = "usage: desktop_world_snapshot <store-dir> <artifact-dir> [visual-catalog.json visual-policy.json]";

/// Resolves an entity's material (microunit-encoded) and optional embodiment.
/// When the entity DECLARES a `semantic_type` and a visual authority is present,
/// the material + embodiment come from the graph-materialized visual mapping,
/// with epistemic modulation. Otherwise the entity stays a neutral `unknown`.
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
        // Provenance for the renderer: the form + epistemic confidence the
        // authority resolved this entity to.
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

/// Deterministic Fibonacci-sphere layout — a spatial arrangement for legibility,
/// NOT a claim about the entity. Same index + count always yields the same point.
fn fibonacci_layout(index: usize, count: usize) -> [f64; 3] {
    if count <= 1 {
        return [0.0, 0.0, 0.0];
    }
    let n = count as f64;
    let i = index as f64 + 0.5;
    let phi = (1.0 - 2.0 * i / n).clamp(-1.0, 1.0).acos();
    let golden = std::f64::consts::PI * (1.0 + 5.0_f64.sqrt());
    let theta = golden * i;
    [
        LAYOUT_RADIUS * phi.sin() * theta.cos(),
        LAYOUT_RADIUS * phi.sin() * theta.sin(),
        LAYOUT_RADIUS * phi.cos(),
    ]
}
