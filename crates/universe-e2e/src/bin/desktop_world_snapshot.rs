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
//! honest `unknown` visual. Positions come from the graph-native layout kernel
//! (`universe_assets::layout`): the Space tree (PART_OF) with scale-per-descent,
//! per-predicate `physical_profile` forces, and hitbox packing — never a physics
//! residency (which stays `not_measured` for layout).

use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs,
    path::PathBuf,
};
use universe_assets::layout::{self, LayoutParams, ProfileInput, RelationInput};
use universe_assets::visual::{derive, VisualCatalog, VisualPolicy};
use universe_core::EntityKey;
use universe_store::UniverseStore;

const PROTOCOL_VERSION: u16 = 0;
const MICRO: f64 = 1_000_000.0;
/// The canonical containment predicate: `partie → ensemble`. It is the only
/// predicate that builds the Space tree (there is no dedicated spatial `inside`
/// relation in the ontology; PART_OF carries containment).
const CONTAINMENT_PREDICATE: &str = "PART_OF";

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

    // --- Structural layout: replace the structure-blind Fibonacci sphere with
    // the graph-native layout kernel. Positions derive from the Space tree
    // (PART_OF), per-predicate physical_profile forces, and hitbox packing. ---
    let positions = compute_layout(&store, &snapshot)?;

    let mut frames: Vec<Value> = Vec::new();
    let mut sequence: u64 = 1;
    let mut resolved_embodiments = 0usize;

    frames.push(json!({
        "protocol_version": PROTOCOL_VERSION,
        "sequence": sequence,
        "payload": { "message_type": "snapshot", "revision": snapshot.revision.0 },
    }));

    for entity in snapshot.entities.iter() {
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

        let [x, y, z] = positions
            .placements
            .get(&entity.key)
            .copied()
            .unwrap_or([0.0, 0.0, 0.0]);
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
        // A `thing` whose content declares an audio pointer loops in the
        // renderer. The decision is graph-owned: emitted verbatim from content,
        // never invented here.
        if let Some(audio) = resolve_audio(content.as_ref()) {
            entity_value["audio"] = audio;
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
        "layout": {
            "kernel": "graph_native_space_tree_force_directed",
            "containment_predicate": CONTAINMENT_PREDICATE,
            "containment_links": positions.containment_links,
            "force_links": positions.force_links,
            "profiles_read": positions.profiles_read,
            "max_space_depth": positions.max_depth,
            "residual_hitbox_overlaps": positions.residual_overlaps,
            "similarity_measured": false,
        },
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

/// Surfaces an entity's audio facet when its graph content declares one. The
/// authoring convention places an `audio` object (with a string `src`) only on
/// `thing` content; the projection emits it as-is so the renderer can loop it.
/// `loop` defaults to true (the feature: audio things loop) and `gain` to full.
/// Nothing is emitted when no `audio.src` is declared — a silent entity stays
/// honestly silent rather than acquiring an invented sound.
fn resolve_audio(content: Option<&Value>) -> Option<Value> {
    let audio = content?.get("audio")?;
    let src = audio.get("src").and_then(Value::as_str)?;
    if src.is_empty() {
        return None;
    }
    let looping = audio.get("loop").and_then(Value::as_bool).unwrap_or(true);
    let gain = audio.get("gain").and_then(Value::as_f64).unwrap_or(1.0);
    Some(json!({
        "src": src,
        "loop": looping,
        "gain_micro": micro(gain.clamp(0.0, 1.0)),
    }))
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

/// The computed layout plus the evidence the manifest reports.
struct ComputedLayout {
    placements: BTreeMap<EntityKey, [f64; 3]>,
    max_depth: u32,
    residual_overlaps: usize,
    profiles_read: usize,
    containment_links: usize,
    force_links: usize,
}

/// Projects the store into a `LayoutInput` and runs the graph-native layout
/// kernel. Positions come from the Space tree (PART_OF), the per-predicate
/// `physical_profile` forces, and hitbox packing — NOT a structure-blind sphere.
/// Similarity is `not_measured` here (no embedding sampled), so it is honestly 0.
fn compute_layout(
    store: &UniverseStore,
    snapshot: &universe_store::UniverseSnapshot,
) -> Result<ComputedLayout, Box<dyn Error>> {
    let node_keys: Vec<EntityKey> = snapshot.entities.iter().map(|entity| entity.key).collect();

    // Per-predicate force descriptors, keyed by `canonical_id`.
    let mut profiles: BTreeMap<String, ProfileInput> = BTreeMap::new();
    for entity in &snapshot.entities {
        let Some(content) = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok())
        else {
            continue;
        };
        if content.get("kind").and_then(Value::as_str) != Some("physical_profile") {
            continue;
        }
        let (Some(canonical_id), Some(profile)) = (
            content.get("canonical_id").and_then(Value::as_str),
            content.get("profile"),
        ) else {
            continue;
        };
        let hierarchy = profile.get("hierarchy").and_then(Value::as_f64);
        let polarity = profile
            .get("polarity")
            .and_then(Value::as_array)
            .and_then(|a| {
                if a.len() == 2 {
                    Some([a[0].as_f64()?, a[1].as_f64()?])
                } else {
                    None
                }
            });
        // Only record a profile when BOTH scalars are present — never a partial,
        // invented descriptor.
        if let (Some(hierarchy), Some(polarity)) = (hierarchy, polarity) {
            profiles.insert(
                canonical_id.to_owned(),
                ProfileInput {
                    hierarchy,
                    polarity,
                },
            );
        }
    }

    // Relations carried by predicate name.
    let relations: Vec<RelationInput> = snapshot
        .relations
        .iter()
        .filter_map(|relation| {
            let predicate = snapshot.symbols.get(relation.predicate as usize)?.clone();
            Some(RelationInput {
                source: relation.source,
                target: relation.target,
                predicate,
            })
        })
        .collect();

    let containment: BTreeSet<String> = BTreeSet::from([CONTAINMENT_PREDICATE.to_owned()]);
    let input = layout::project(
        &node_keys,
        &relations,
        &profiles,
        &containment,
        &|_, _| 0.0, // similarity: not_measured — no embedding sampled here.
        layout::DEFAULT_RADIUS,
        LayoutParams::default(),
    );
    let containment_links = input.links.iter().filter(|link| link.inside).count();
    let force_links = input.links.len() - containment_links;

    let result = layout::compute(&input).map_err(|error| error.to_string())?;
    let placements = result
        .placements
        .iter()
        .map(|placed| (placed.key, placed.position))
        .collect();

    Ok(ComputedLayout {
        placements,
        max_depth: result.max_depth,
        residual_overlaps: result.residual_overlaps,
        profiles_read: profiles.len(),
        containment_links,
        force_links,
    })
}
