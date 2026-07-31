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
use universe_embeddings::{cosine, QuantizedEmbedding};
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
    // Every entity carries a `dynamics` field now; this counts only those whose
    // graph content DECLARED a measured energy or weight (the honest signals),
    // distinct from the always-present procedural embedding default.
    let mut entities_with_declared_signals = 0usize;

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
        // Provenance / semantic identity — so the renderer resolves this node's
        // appearance through its PRODUCING TOOLKIT's visual binding (the archetype
        // declared for its role_axis / semantic_type), not a universal default. A
        // node that declares no producing toolkit carries only its role/type and
        // renders as the honest bare-presence fallback (never a defaulted citizen).
        if let Some(provenance) = resolve_provenance(content.as_ref()) {
            entity_value["provenance"] = provenance;
        }
        // Per-node dynamic signals (energy → emit, weight/poids → size, embedding
        // → orientation + micro-variation). The `dynamics` field is MANDATORY and
        // always emitted: energy/weight are honest (present only when declared, so
        // an un-measured node is never lit), while the embedding defaults to a
        // procedural per-node seed so every node is individuated. The renderer
        // gates the energy→glow channel on epistemic confidence.
        let declared_signal = content
            .as_ref()
            .map(|c| c.get("energy").is_some() || c.get("weight").is_some())
            .unwrap_or(false);
        entity_value["dynamics"] = resolve_dynamics(content.as_ref(), entity.key);
        if declared_signal {
            entities_with_declared_signals += 1;
        }
        frames.push(json!({
            "protocol_version": PROTOCOL_VERSION,
            "sequence": sequence,
            "payload": { "message_type": "entity_materialized", "entity": entity_value }
        }));
    }

    let mut relation_profiles_emitted = 0usize;
    for relation in &snapshot.relations {
        sequence += 1;
        let predicate = snapshot
            .symbols
            .get(relation.predicate as usize)
            .cloned()
            .unwrap_or_else(|| "<unknown-predicate>".to_owned());
        // The neutral bond is the honest default: a relation whose predicate
        // declares no `physical_profile` carries no polarity/hierarchy, and the
        // renderer draws it neutral rather than faking a colour or slope.
        let mut visual = json!({
            "primitive": "unknown",
            "color": "#5a6675",
            "emissive": "#1f2731",
            "emissive_intensity_micro": 0,
            "opacity_micro": 500_000,
            "width_micro": 20_000,
            "lane_separation_micro": 0
        });
        // Canonical link channels (ALIGN.md §2/§4): when the predicate DECLARES a
        // physical_profile, carry its polarity `[p_ab, p_ba]` (sign → excitation /
        // inhibition light) and signed `hierarchy` (→ conduit slope). Same values
        // the layout already read for placement — one read, two usages. Absent ⇒
        // neither field is emitted (never a default 0, which would read as a
        // confident "flat, neutral" instead of "not declared").
        if let Some(profile) = positions.profiles.get(&predicate) {
            visual["hierarchy_micro"] = json!(micro(profile.hierarchy));
            visual["polarity_micro"] =
                json!([micro(profile.polarity[0]), micro(profile.polarity[1])]);
            relation_profiles_emitted += 1;
        }
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
                    "visual": visual
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
        "relation_profiles_emitted": relation_profiles_emitted,
        "total_frames": frames.len(),
        "readback_ok": readback_ok,
        "visual_authority_resolved": authority.is_some(),
        "resolved_embodiments": resolved_embodiments,
        "entities_with_dynamics": entity_count,
        "entities_with_declared_signals": entities_with_declared_signals,
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
            "similarity_measured": positions.embeddings_read > 0,
            "embeddings_read": positions.embeddings_read,
            "macro_layer": match positions.clustered {
                Some((graphs, routes, cross, membrane_nodes)) => json!({
                    "active": true,
                    "graphs": graphs,
                    "routes": routes,
                    "cross_graph_overlaps": cross,
                    "membrane_nodes": membrane_nodes,
                }),
                // No entity declared a graph (`clusterId`) at projection granularity.
                None => json!({ "active": false, "reason": "no per-entity clusterId at projected granularity" }),
            },
        },
    });
    fs::write(
        artifact_dir.join("world-snapshot-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    println!(
        "world-snapshot universe={} revision={} entities={} relations={} frames={} embodiments={} declared_signals={} authority={} readback_ok={}",
        snapshot.universe,
        snapshot.revision.0,
        entity_count,
        relation_count,
        frames.len(),
        resolved_embodiments,
        entities_with_declared_signals,
        authority.is_some(),
        readback_ok
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_embeddings::QuantizedEmbedding;

    const KEY: EntityKey = EntityKey(0xb101);

    #[test]
    fn resolve_dynamics_carries_declared_signals_and_micro_encodes_them() {
        let embedding = QuantizedEmbedding {
            model: "m".into(),
            model_revision: "r".into(),
            input_sha256: "x".into(),
            dimensions: 3,
            scale: 1000,
            values: vec![1000, -500, 250],
        };
        let content = json!({
            "kind": "canonical_node",
            "energy": 137,
            "weight": 62.5,
            "embedding": embedding,
        });
        let dynamics = resolve_dynamics(Some(&content), KEY);
        assert_eq!(dynamics["energy"], json!(137));
        // weight 62.5 → micro
        assert_eq!(dynamics["weight_micro"], json!(62_500_000));
        // embedding dequantized (value / scale) then ×1e6: [1.0, -0.5, 0.25].
        assert_eq!(
            dynamics["embedding_micro"],
            json!([1_000_000, -500_000, 250_000])
        );
    }

    #[test]
    fn resolve_dynamics_defaults_to_a_procedural_embedding_when_none_declared() {
        // The field is mandatory: with no declared signals, energy/weight are
        // absent (honest — never invented) but a procedural embedding is present
        // so the node still individuates. Deterministic for a given key.
        let content = json!({ "kind": "canonical_node", "title": "no signals" });
        let dynamics = resolve_dynamics(Some(&content), KEY);
        assert!(dynamics.get("energy").is_none());
        assert!(dynamics.get("weight_micro").is_none());
        let embedding = dynamics["embedding_micro"].as_array().unwrap();
        assert_eq!(embedding.len(), 4);
        assert_eq!(dynamics, resolve_dynamics(Some(&content), KEY));
        assert_eq!(dynamics, resolve_dynamics(None, KEY));
    }

    #[test]
    fn resolve_dynamics_drops_invalid_signals_but_still_individuates() {
        // Negative energy/weight are not honest magnitudes → dropped. A zero-scale
        // embedding is undecodable → falls back to the procedural default.
        let content = json!({
            "energy": -5,
            "weight": -1.0,
            "embedding": { "model": "m", "model_revision": "r", "input_sha256": "x",
                           "dimensions": 1, "scale": 0, "values": [1] },
        });
        let dynamics = resolve_dynamics(Some(&content), KEY);
        assert!(dynamics.get("energy").is_none());
        assert!(dynamics.get("weight_micro").is_none());
        assert_eq!(dynamics["embedding_micro"].as_array().unwrap().len(), 4);
    }
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

/// Surfaces a node's provenance / semantic identity so the renderer can resolve
/// its appearance through the producing toolkit's visual binding rather than a
/// kernel-type-global default. Emitted verbatim from graph content: `role_axis`
/// (defaulting to `semantic_type` so the closed role axis is always present),
/// `semantic_type` (the key the toolkit binding's archetype is declared for), and
/// `producing_toolkit` when declared (either a top-level field or under a
/// `provenance` object). A node that declares none carries no provenance — an
/// honestly unattributed node the renderer draws as the bare-presence fallback.
fn resolve_provenance(content: Option<&Value>) -> Option<Value> {
    let content = content?;
    let semantic_type = content.get("semantic_type").and_then(Value::as_str);
    let role_axis = content.get("role_axis").and_then(Value::as_str);
    let producing_toolkit = content
        .get("producing_toolkit")
        .and_then(Value::as_str)
        .or_else(|| {
            content
                .get("provenance")
                .and_then(|p| p.get("producing_toolkit"))
                .and_then(Value::as_str)
        });
    if semantic_type.is_none() && role_axis.is_none() && producing_toolkit.is_none() {
        return None;
    }
    let mut out = serde_json::Map::new();
    if let Some(value) = role_axis.or(semantic_type) {
        out.insert("role_axis".to_owned(), json!(value));
    }
    if let Some(value) = semantic_type {
        out.insert("semantic_type".to_owned(), json!(value));
    }
    if let Some(value) = producing_toolkit {
        out.insert("producing_toolkit".to_owned(), json!(value));
    }
    Some(Value::Object(out))
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

/// Builds a node's MANDATORY `dynamics` field, micro-encoded for the wire. The
/// field is always present (with a default), so no node is ever left un-modulated:
/// - `energy` (a non-negative number) → `energy` (measured integer; the renderer
///   applies it to emission only for an epistemically confident node) — HONEST:
///   emitted only when declared, never invented;
/// - `weight` / `poids` (a non-negative number) → `weight_micro` (drives scale) —
///   likewise emitted only when declared;
/// - `embedding` → `embedding_micro`: the dequantized declared `QuantizedEmbedding`
///   (`value / scale`, then ×1e6) when present, ELSE a procedural per-node default
///   keyed by the entity id. The embedding channel drives only orientation +
///   micro-variation (aesthetic individuation), so a procedural default is honest
///   there — it individuates every node without claiming a measured energy or size.
///
/// `key` seeds the procedural default. Always returns a value (the field is required).
fn resolve_dynamics(content: Option<&Value>, key: EntityKey) -> Value {
    let mut dynamics = serde_json::Map::new();

    if let Some(energy) = content.and_then(|c| c.get("energy")).and_then(nonneg_integer) {
        dynamics.insert("energy".to_owned(), json!(energy));
    }
    if let Some(weight) = content
        .and_then(|c| c.get("weight"))
        .and_then(Value::as_f64)
        .filter(|weight| weight.is_finite() && *weight >= 0.0)
    {
        dynamics.insert("weight_micro".to_owned(), json!(micro(weight)));
    }
    let declared_embedding = content
        .and_then(|c| c.get("embedding"))
        .and_then(|value| serde_json::from_value::<QuantizedEmbedding>(value.clone()).ok())
        .filter(|embedding| embedding.scale != 0 && !embedding.values.is_empty());
    let embedding_micro: Vec<i64> = match declared_embedding {
        Some(embedding) => {
            let scale = embedding.scale as f64;
            embedding
                .values
                .iter()
                .map(|&component| ((component as f64 / scale) * MICRO).round() as i64)
                .collect()
        }
        None => procedural_embedding_micro(key),
    };
    dynamics.insert("embedding_micro".to_owned(), json!(embedding_micro));

    Value::Object(dynamics)
}

/// A deterministic pseudo-embedding from a node's id — the DEFAULT for the
/// embedding channel when the graph declares none. It is a visual-individuation
/// seed (drives only orientation + micro-variation), NOT a measured embedding, so
/// every node still renders as a distinct being. Four components in [-1, 1],
/// micro-encoded. Mirrors the renderer's `proceduralEmbedding` intent (FNV-based).
fn procedural_embedding_micro(key: EntityKey) -> Vec<i64> {
    let id = key.to_string();
    (0..4u32)
        .map(|axis| {
            let mut hash: u64 =
                14695981039346656037 ^ (axis as u64).wrapping_mul(1099511628211);
            for byte in id.bytes() {
                hash = (hash ^ byte as u64).wrapping_mul(1099511628211);
            }
            let unit = (hash as f64 / u64::MAX as f64) * 2.0 - 1.0; // [-1, 1]
            (unit * MICRO).round() as i64
        })
        .collect()
}

/// A JSON number read as a non-negative integer: accepts an unsigned integer or a
/// finite non-negative float (rounded). Anything else (negative, NaN, non-number)
/// yields `None` — the signal is dropped rather than coerced.
fn nonneg_integer(value: &Value) -> Option<i64> {
    if let Some(unsigned) = value.as_u64() {
        return i64::try_from(unsigned).ok();
    }
    value
        .as_f64()
        .filter(|number| number.is_finite() && *number >= 0.0)
        .map(|number| number.round() as i64)
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
    /// Entities that declared an embedding (drives measured similarity).
    embeddings_read: usize,
    /// Present when the macro layer ran: (graphs, routes, cross_graph_overlaps, membrane_nodes).
    clustered: Option<(usize, usize, usize, usize)>,
    /// Per-predicate `physical_profile` forces, keyed by `canonical_id` (which is
    /// the predicate symbol). Already read for placement; reused to paint each
    /// relation's polarity (→ light colour) and hierarchy (→ slope) channels on
    /// the wire — one read, two usages (ALIGN.md §2/§4).
    profiles: BTreeMap<String, ProfileInput>,
}

/// Derives a graph's Mind Protocol layer from its `clusterId` prefix (`l4-…` ⇒
/// 4). Higher layers sit more central (L4 at the core). No recognised prefix ⇒
/// layer 0 (outermost) — an honest "unlayered", not an invented placement.
fn layer_from_cluster(cluster: &str) -> u8 {
    let bytes = cluster.as_bytes();
    if bytes.len() >= 2 && (bytes[0] == b'l' || bytes[0] == b'L') && bytes[1].is_ascii_digit() {
        let digit = bytes[1] - b'0';
        if (1..=4).contains(&digit) {
            return digit;
        }
    }
    0
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

    // Per-predicate force descriptors, keyed by `canonical_id`; and each entity's
    // graph membership (`clusterId`) when its content declares one at top level.
    let mut profiles: BTreeMap<String, ProfileInput> = BTreeMap::new();
    let mut clusters: BTreeMap<EntityKey, String> = BTreeMap::new();
    let mut embeddings: BTreeMap<EntityKey, QuantizedEmbedding> = BTreeMap::new();
    for entity in &snapshot.entities {
        let Some(content) = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok())
        else {
            continue;
        };
        if let Some(cluster) = content
            .get("clusterId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            clusters.insert(entity.key, cluster.to_owned());
        }
        // An entity's embedding, when it declares one (shape = QuantizedEmbedding).
        if let Some(embedding) = content
            .get("embedding")
            .and_then(|value| serde_json::from_value::<QuantizedEmbedding>(value.clone()).ok())
        {
            embeddings.insert(entity.key, embedding);
        }
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
    // Similarity = embedding cosine in [0,1] when BOTH endpoints declare an
    // embedding; otherwise honestly 0 (`not_measured`), never an invented signal.
    // A negative cosine (dissimilar) also maps to 0 — no attraction, not repulsion.
    let similarity = |source: EntityKey, target: EntityKey| -> f64 {
        match (embeddings.get(&source), embeddings.get(&target)) {
            (Some(left), Some(right)) => cosine(left, right)
                .map(|score| (score as f64 / universe_embeddings::SCORE_SCALE as f64).clamp(0.0, 1.0))
                .unwrap_or(0.0),
            _ => 0.0,
        }
    };
    let input = layout::project(
        &node_keys,
        &relations,
        &profiles,
        &containment,
        &similarity,
        layout::DEFAULT_RADIUS,
        LayoutParams::default(),
    );
    let containment_links = input.links.iter().filter(|link| link.inside).count();
    let force_links = input.links.len() - containment_links;

    // Macro layer: when entities declare their graph (`clusterId`), separate the
    // graphs and arrange them by Mind Protocol layer (L4 central). Otherwise fall
    // back to a single graph — an honest no-op, never a faked multi-graph split.
    let (placements, max_depth, residual_overlaps, clustered) = if clusters.is_empty() {
        let result = layout::compute(&input).map_err(|error| error.to_string())?;
        let placements = result
            .placements
            .iter()
            .map(|placed| (placed.key, placed.position))
            .collect();
        (placements, result.max_depth, result.residual_overlaps, None)
    } else {
        let layers: BTreeMap<String, u8> = clusters
            .values()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|cluster| {
                let layer = layer_from_cluster(&cluster);
                (cluster, layer)
            })
            .collect();
        // The special membrane graph is identified by convention: a cluster named
        // `membrane` (or `membrane-…`). It is unbounded and its links are routes.
        let membrane = clusters
            .values()
            .find(|c| c.as_str() == "membrane" || c.starts_with("membrane-"))
            .cloned();
        let result = layout::compute_clustered(&input, &clusters, &layers, membrane.as_deref())
            .map_err(|error| error.to_string())?;
        let placements = result
            .placements
            .iter()
            .map(|placed| (placed.key, placed.position))
            .collect();
        (
            placements,
            result.max_depth,
            result.residual_overlaps,
            Some((
                result.graphs.len(),
                result.routes,
                result.cross_graph_overlaps,
                result.membrane_nodes,
            )),
        )
    };

    Ok(ComputedLayout {
        placements,
        max_depth,
        residual_overlaps,
        profiles_read: profiles.len(),
        containment_links,
        force_links,
        embeddings_read: embeddings.len(),
        clustered,
        profiles,
    })
}
