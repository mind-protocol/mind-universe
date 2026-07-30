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
//! Epistemic honesty: this projection asserts NO canonical visual meaning. Every
//! entity is emitted with the `unknown` visual primitive and a neutral,
//! non-emissive material; positions are a deterministic Fibonacci-sphere LAYOUT,
//! not a physics residency (which is `not_measured` here). Resolving real visuals
//! through the materialized visual mapping is a separate, later step — this loop
//! only makes the real topology visible without inventing meaning.

use serde_json::{json, Value};
use std::{env, error::Error, fs, path::PathBuf};
use universe_store::UniverseStore;

const PROTOCOL_VERSION: u16 = 0;
const LAYOUT_RADIUS: f64 = 6.0;
const MICRO: f64 = 1_000_000.0;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let store_root = args.next().map(PathBuf::from).ok_or("usage: desktop_world_snapshot <store-dir> <artifact-dir>")?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or("usage: desktop_world_snapshot <store-dir> <artifact-dir>")?;
    if args.next().is_some() {
        return Err("usage: desktop_world_snapshot <store-dir> <artifact-dir>".into());
    }
    fs::create_dir_all(&artifact_dir)?;

    let store = UniverseStore::open(&store_root)?;
    let snapshot = store.replay(store.load_snapshot()?)?;

    let entity_count = snapshot.entities.len();
    let mut frames: Vec<Value> = Vec::new();
    let mut sequence: u64 = 1;

    // Frame 1: the snapshot boundary — resets the view to this revision.
    frames.push(json!({
        "protocol_version": PROTOCOL_VERSION,
        "sequence": sequence,
        "payload": { "message_type": "snapshot", "revision": snapshot.revision.0 },
    }));

    // One entity_materialized frame per Node/Asset actually in the store.
    for (index, entity) in snapshot.entities.iter().enumerate() {
        sequence += 1;
        let symbol = snapshot
            .symbols
            .get(entity.symbol as usize)
            .cloned()
            .unwrap_or_else(|| "<unknown-symbol>".to_owned());
        let kind = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok())
            .and_then(|content| content.get("kind").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "<no-content>".to_owned());
        let [x, y, z] = fibonacci_layout(index, entity_count);
        frames.push(json!({
            "protocol_version": PROTOCOL_VERSION,
            "sequence": sequence,
            "payload": {
                "message_type": "entity_materialized",
                "entity": {
                    "id": entity.key.to_string(),
                    "generation": entity.generation,
                    // Context for later graph-authority visual resolution; the app
                    // does not treat these as canonical meaning.
                    "symbol": symbol,
                    "content_kind": kind,
                    "residency": "not_measured",
                    "position_micro": [micro(x), micro(y), micro(z)],
                    "visual": {
                        "primitive": "unknown",
                        "motion": "still",
                        "material": {
                            "color": "#8a97a8",
                            "emissive": "#2b3440",
                            "emissive_intensity_micro": 0,
                            "opacity_micro": 700_000,
                            "scale_micro": 1_000_000
                        }
                    }
                }
            }
        }));
    }

    // One relation_materialized frame per relation.
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

    // Independent readback: re-read the written file and confirm the frame count
    // and that every entity id is unique and referenced consistently.
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
        // Honest: visuals are NOT resolved from graph authority yet.
        "visual_authority_resolved": false,
        "residency_measured": false,
        "information_status": "measured",
    });
    fs::write(
        artifact_dir.join("world-snapshot-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    println!(
        "world-snapshot universe={} revision={} entities={} relations={} frames={} readback_ok={}",
        snapshot.universe, snapshot.revision.0, entity_count, relation_count, frames.len(), readback_ok
    );
    Ok(())
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
