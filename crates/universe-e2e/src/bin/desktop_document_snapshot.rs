//! Projects the LOGICAL graph — the nodes and links held inside `ontology_source`
//! `document` blobs — into the Mind Desktop wire protocol, instead of the store's
//! top-level infrastructure entities.
//!
//! `desktop_world_snapshot` projects the store's top-level entities, but the real
//! clustered graph (nodes carrying `clusterId`/`semanticType`, links carrying a
//! predicate `type`) lives one level down, inside `document` blobs. At that
//! granularity the macro layer ACTIVATES: nodes separate by `clusterId` into
//! graphs, arranged by Mind Protocol layer (L4 central). This bin closes that
//! granularity gap for the clustered layout.
//!
//! Positions come from the graph-native layout kernel (`universe_assets::layout`):
//! document nodes are keyed by a stable hash of their string id, links become
//! containment (`PART_OF`) or force links, and `compute_clustered` separates the
//! graphs by layer. Similarity is `not_measured` (no embedding in these docs).

use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs,
    path::PathBuf,
};
use universe_assets::layout::{self, stable_key, LayoutParams, ProfileInput, RelationInput};
use universe_core::EntityKey;
use universe_store::UniverseStore;

const PROTOCOL_VERSION: u16 = 0;
const MICRO: f64 = 1_000_000.0;
const CONTAINMENT_PREDICATE: &str = "PART_OF";

struct DocNode {
    key: EntityKey,
    id: String,
    cluster: String,
    semantic_type: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let store_root = args.next().map(PathBuf::from).ok_or(USAGE)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or(USAGE)?;
    if args.next().is_some() {
        return Err(USAGE.into());
    }
    fs::create_dir_all(&artifact_dir)?;

    let store = UniverseStore::open(&store_root)?;
    let snapshot = store.replay(store.load_snapshot()?)?;

    // Per-predicate force descriptors (physical_profile), keyed by canonical_id.
    let mut profiles: BTreeMap<String, ProfileInput> = BTreeMap::new();
    // Logical document nodes and links, gathered across every ontology_source.
    let mut nodes: Vec<DocNode> = Vec::new();
    let mut seen: BTreeSet<EntityKey> = BTreeSet::new();
    let mut relations: Vec<RelationInput> = Vec::new();
    let mut known_ids: BTreeSet<String> = BTreeSet::new();

    for entity in &snapshot.entities {
        let Some(content) = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok())
        else {
            continue;
        };
        match content.get("kind").and_then(Value::as_str) {
            Some("physical_profile") => read_profile(&content, &mut profiles),
            Some("ontology_source") => {
                if let Some(document) = content.get("document") {
                    collect_document(document, &mut nodes, &mut seen, &mut known_ids);
                }
            }
            _ => {}
        }
    }

    // Second pass for links now that every node id is known (drop dangling links).
    for entity in &snapshot.entities {
        let Some(content) = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok())
        else {
            continue;
        };
        if content.get("kind").and_then(Value::as_str) != Some("ontology_source") {
            continue;
        }
        let Some(links) = content
            .get("document")
            .and_then(|document| document.get("links"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for link in links {
            let (Some(source), Some(target), predicate) = (
                link.get("source").and_then(Value::as_str),
                link.get("target").and_then(Value::as_str),
                link.get("type").and_then(Value::as_str).unwrap_or("LINKS"),
            ) else {
                continue;
            };
            if known_ids.contains(source) && known_ids.contains(target) {
                relations.push(RelationInput {
                    source: stable_key(source),
                    target: stable_key(target),
                    predicate: predicate.to_owned(),
                });
            }
        }
    }

    let node_keys: Vec<EntityKey> = nodes.iter().map(|node| node.key).collect();
    let clusters: BTreeMap<EntityKey, String> = nodes
        .iter()
        .map(|node| (node.key, node.cluster.clone()))
        .collect();

    let containment: BTreeSet<String> = BTreeSet::from([CONTAINMENT_PREDICATE.to_owned()]);
    let input = layout::project(
        &node_keys,
        &relations,
        &profiles,
        &containment,
        &|_, _| 0.0, // similarity: not_measured — no embedding in these documents.
        layout::DEFAULT_RADIUS,
        LayoutParams::default(),
    );

    // Layer per cluster from its id prefix; membrane by naming convention.
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
    let membrane = clusters
        .values()
        .find(|c| c.as_str() == "membrane" || c.starts_with("membrane-"))
        .cloned();

    // Continuous "city" layout: one global field, gapless, shaped by the link
    // topology (no round blobs, no space between graphs), L4 kept central.
    let result = layout::compute_city(&input, &clusters, &layers, membrane.as_deref())
        .map_err(|error| format!("{error:?}"))?;
    // key → (position, footprint radius, built height)
    let built: BTreeMap<EntityKey, ([f64; 3], f64, f64)> = result
        .placements
        .iter()
        .map(|placed| (placed.key, (placed.position, placed.radius, placed.height)))
        .collect();

    // Emit frames.
    let mut frames: Vec<Value> = Vec::new();
    let mut sequence: u64 = 1;
    frames.push(json!({
        "protocol_version": PROTOCOL_VERSION,
        "sequence": sequence,
        "payload": { "message_type": "snapshot", "revision": snapshot.revision.0 },
    }));
    for node in &nodes {
        sequence += 1;
        let ([x, y, z], footprint, height) = built
            .get(&node.key)
            .copied()
            .unwrap_or(([0.0, 0.0, 0.0], layout::DEFAULT_RADIUS, 1.0));
        frames.push(json!({
            "protocol_version": PROTOCOL_VERSION,
            "sequence": sequence,
            "payload": { "message_type": "entity_materialized", "entity": {
                "id": node.key.to_string(),
                "logical_id": node.id,
                "cluster": node.cluster,
                "semantic_type": node.semantic_type,
                "position_micro": [micro(x), micro(y), micro(z)],
                // Weight→volume: a wide footprint, or a tall tower where dense.
                "footprint_micro": micro(footprint),
                "height_micro": micro(height),
            }}
        }));
    }
    for relation in &relations {
        sequence += 1;
        frames.push(json!({
            "protocol_version": PROTOCOL_VERSION,
            "sequence": sequence,
            "payload": { "message_type": "relation_materialized", "relation": {
                "source": relation.source.to_string(),
                "target": relation.target.to_string(),
                "predicate": relation.predicate,
            }}
        }));
    }

    let frames_path = artifact_dir.join("document-snapshot-frames.json");
    fs::write(&frames_path, serde_json::to_vec_pretty(&frames)?)?;
    let readback: Vec<Value> = serde_json::from_slice(&fs::read(&frames_path)?)?;
    let readback_ok = readback.len() == frames.len();

    let manifest = json!({
        "kind": "desktop_document_snapshot_manifest",
        "store_root": store_root.to_string_lossy(),
        "universe": snapshot.universe.to_string(),
        "revision": snapshot.revision.0,
        "logical_nodes": nodes.len(),
        "logical_links": relations.len(),
        "profiles_read": profiles.len(),
        "readback_ok": readback_ok,
        "information_status": "measured",
        "layout": {
            "kernel": "graph_native_continuous_city",
            "city": {
                "graphs": result.graphs.len(),
                "extent_micro": micro(result.extent),
                // Fraction of the city disc filled by hitboxes (continuity).
                "occupancy_micro": result.occupancy_micro,
                // Tallest tower — a heavy node squeezed by a dense locality.
                "max_height_micro": micro(result.max_height),
                "districts": result.graphs.iter().map(|g| json!({
                    "cluster": g.cluster, "layer": g.layer, "nodes": g.node_count,
                    "centroid_micro": [micro(g.anchor[0]), micro(g.anchor[1]), micro(g.anchor[2])],
                    "extent_micro": micro(g.bounding),
                })).collect::<Vec<_>>(),
            },
            "residual_hitbox_overlaps": result.residual_overlaps,
            "similarity_measured": false,
        },
    });
    fs::write(
        artifact_dir.join("document-snapshot-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    println!(
        "document-snapshot universe={} nodes={} links={} graphs={} overlaps={} readback_ok={}",
        snapshot.universe,
        nodes.len(),
        relations.len(),
        result.graphs.len(),
        result.residual_overlaps,
        readback_ok
    );
    Ok(())
}

const USAGE: &str = "usage: desktop_document_snapshot <store-dir> <artifact-dir>";

fn collect_document(
    document: &Value,
    nodes: &mut Vec<DocNode>,
    seen: &mut BTreeSet<EntityKey>,
    known_ids: &mut BTreeSet<String>,
) {
    let Some(list) = document.get("nodes").and_then(Value::as_array) else {
        return;
    };
    for node in list {
        let Some(id) = node.get("id").and_then(Value::as_str) else {
            continue;
        };
        let key = stable_key(id);
        // Distinct ids never collide (see stable_key test); dedup defensively.
        if !seen.insert(key) {
            continue;
        }
        known_ids.insert(id.to_owned());
        nodes.push(DocNode {
            key,
            id: id.to_owned(),
            cluster: node
                .get("clusterId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            semantic_type: node
                .get("semanticType")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
        });
    }
}

fn read_profile(content: &Value, profiles: &mut BTreeMap<String, ProfileInput>) {
    let (Some(canonical_id), Some(profile)) = (
        content.get("canonical_id").and_then(Value::as_str),
        content.get("profile"),
    ) else {
        return;
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
    if let (Some(hierarchy), Some(polarity)) = (hierarchy, polarity) {
        profiles.insert(canonical_id.to_owned(), ProfileInput { hierarchy, polarity });
    }
}

/// Layer from a `clusterId` prefix (`l4-…` ⇒ 4; higher = more central). No
/// recognised prefix ⇒ layer 0 (outermost).
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

fn micro(value: f64) -> i64 {
    (value * MICRO).round() as i64
}
