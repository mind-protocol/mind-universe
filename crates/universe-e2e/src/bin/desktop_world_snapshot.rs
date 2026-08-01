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
//! honest `unknown` visual.
//!
//! Position is a CONSTRUCTION, not a projection. A node the store says a citizen
//! BUILT somewhere (a `built_position` record naming it) is emitted at exactly
//! that place, and the solver never moves it. Every other node is SCAFFOLDED by
//! the graph-native layout kernel (`universe_assets::layout`): the Space tree
//! (PART_OF) with scale-per-descent, per-predicate `physical_profile` forces, and
//! hitbox packing — a proposed starting spot, never an authored one. Each entity
//! carries which of the two it is (`placement.provenance`), so a scaffold is never
//! read as a construction. Physical residency stays `not_measured` for layout.

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

    // The appearance each toolkit holds for what it produces, read FROM THE GRAPH:
    // `visual_binding:…` nodes carried AS members of their construct. The policy
    // says which construct registers which member; this index makes that member
    // reachable. Nothing here decides how anything looks — it only stops the
    // toolkit's own authored binding from being unreachable, which is why the one
    // real binding in the store dressed nothing.
    let toolkit_bindings = collect_toolkit_bindings(&store, &snapshot);

    let mut frames: Vec<Value> = Vec::new();
    let mut sequence: u64 = 1;
    let mut resolved_embodiments = 0usize;
    // Every entity carries a `dynamics` field now; this counts only those whose
    // graph content DECLARED a measured energy or weight (the honest signals),
    // distinct from the always-present procedural embedding default.
    let mut entities_with_declared_signals = 0usize;
    // How much of the world can say what it IS and who built it. These are the
    // measured inputs to provenance-based appearance resolution: without them no
    // toolkit binding can be reached, and every node is honestly unbound.
    let mut entities_with_provenance = 0usize;
    let mut entities_with_producing_construct = 0usize;

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
        // The node's kind, and an HONEST distinction the projection used to lose:
        // `<no-content>` means the entity holds no content record at all, while
        // `<unkinded>` means it holds one that names no kind. Collapsing the two
        // reported a node the store describes in full as if the store said nothing
        // about it — `unknown` dressed as `known_absent`. The dominant content
        // shape in the store (`canonical_id` + `node_type` + `subtype` + `content`)
        // carries no `kind` key; its kind is its `subtype` (objective, validation,
        // justification, …), read here rather than discarded.
        let kind = match content.as_ref() {
            None => "<no-content>".to_owned(),
            Some(c) => c
                .get("kind")
                .and_then(Value::as_str)
                .or_else(|| c.get("subtype").and_then(Value::as_str))
                .map(str::to_owned)
                .unwrap_or_else(|| "<unkinded>".to_owned()),
        };
        let residency = content
            .as_ref()
            .and_then(|c| {
                c.get("residency")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "not_measured".to_owned());

        // Provenance first: it is what a toolkit binding is resolved THROUGH.
        let provenance = resolve_provenance(content.as_ref());
        let (material, catalog_embodiment) =
            resolve_visual(content.as_ref(), authority.as_ref()).map_err(|e| e.to_string())?;
        // The producing toolkit's own binding wins over the standalone catalog:
        // the v1 policy demotes that catalog to one toolkit binding among others,
        // so a node is dressed by whoever made it, never by a global default.
        let embodiment = resolve_toolkit_embodiment(
            provenance.as_ref(),
            &residency,
            content
                .as_ref()
                .and_then(|c| c.get("epistemic_state").and_then(Value::as_str))
                .unwrap_or("unknown"),
            authority.as_ref().map(|(_, policy)| policy),
            &toolkit_bindings,
        )
        .or(catalog_embodiment);
        if embodiment.is_some() {
            resolved_embodiments += 1;
        }

        let [x, y, z] = positions
            .placements
            .get(&entity.key)
            .copied()
            .unwrap_or([0.0, 0.0, 0.0]);
        // Where this node is, and on whose authority. `built` = a citizen placed it
        // (a `built_position` record in the store names it); `scaffold` = the layout
        // kernel proposed a starting spot because nothing has ever placed it. The
        // renderer must be able to tell a construction from a proposal.
        let placement_provenance = if positions.built.contains_key(&entity.key) {
            "built"
        } else {
            "scaffold"
        };
        let mut entity_value = json!({
            "id": entity.key.to_string(),
            "generation": entity.generation,
            "symbol": symbol.clone(),
            "content_kind": kind.clone(),
            "residency": residency.clone(),
            "position_micro": [micro(x), micro(y), micro(z)],
            "placement": { "provenance": placement_provenance },
            "visual": { "primitive": "unknown", "motion": "still", "material": material },
            // Presentation facet (hover): the label is the entity's GRAPH SYMBOL and
            // the detail its content kind — read from the store, never invented. A
            // node that declares no `epistemic_state` stays honestly `unknown`.
            "label": symbol,
            "detail": kind,
            "state": residency,
            "epistemic": content
                .as_ref()
                .and_then(|c| c.get("epistemic_state").and_then(Value::as_str))
                .unwrap_or("unknown"),
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
        if let Some(provenance) = provenance {
            entities_with_provenance += 1;
            if provenance.get("producing_toolkit").is_some() {
                entities_with_producing_construct += 1;
            }
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
        "entities_with_provenance": entities_with_provenance,
        "entities_with_producing_construct": entities_with_producing_construct,
        "residency_measured": false,
        "information_status": "measured",
        "placement": {
            // What the store AUTHORED, and what this projection could act on.
            "built_positions_declared": positions.built_declared,
            "built_positions_applied": positions.built.len(),
            "built_positions_unresolved": positions.built_unresolved,
            // Everything else is a layout proposal, not a construction.
            "scaffolded": entity_count.saturating_sub(positions.built.len()),
            "authority": "built_wins_scaffold_proposes",
        },
        "layout": {
            "kernel": "graph_native_space_tree_force_directed",
            // The kernel SCAFFOLDS: it proposes a spot for a node nothing has placed.
            // It is not the placement authority — see `placement` above.
            "role": "scaffold",
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
        "world-snapshot universe={} revision={} entities={} relations={} frames={} embodiments={} provenance={} produced_by={} declared_signals={} built={}/{} unresolved={} authority={} readback_ok={}",
        snapshot.universe,
        snapshot.revision.0,
        entity_count,
        relation_count,
        frames.len(),
        resolved_embodiments,
        entities_with_provenance,
        entities_with_producing_construct,
        entities_with_declared_signals,
        positions.built.len(),
        positions.built_declared,
        positions.built_unresolved,
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

    fn canonical_index() -> BTreeMap<String, EntityKey> {
        BTreeMap::from([(
            "space:l2:lumina-prime:orientation-beacon-v0".to_owned(),
            EntityKey(0xb000),
        )])
    }

    #[test]
    fn placed_node_resolves_both_authored_address_forms() {
        let index = canonical_index();
        // The short hex handle the placement bins write.
        assert_eq!(resolve_placed_node("0x1000", &index), Some(EntityKey(0x1000)));
        // The full 32-digit id the wire uses.
        assert_eq!(
            resolve_placed_node("00000000000000000000000000001100", &index),
            Some(EntityKey(0x1100))
        );
        // A canonical_id addresses the node that holds it.
        assert_eq!(
            resolve_placed_node("space:l2:lumina-prime:orientation-beacon-v0", &index),
            Some(EntityKey(0xb000))
        );
        // An address naming nothing resolves to nothing — never a guessed key.
        assert_eq!(resolve_placed_node("space:l2:nowhere", &index), None);
    }

    #[test]
    fn provenance_is_read_in_the_vocabulary_the_store_actually_writes() {
        // The dominant content shape in the live store: no `kind`, no `role_axis`,
        // no `semantic_type`, no `producing_toolkit` — but an authored name, a
        // `node_type` on the closed role axis, and a `subtype`. Asking only for the
        // first vocabulary returned nothing for every such node.
        let content = json!({
            "canonical_id": "justification:l2:mind-universe:underground-toolkit-v0",
            "node_type": "narrative",
            "subtype": "justification",
            "content": { "statement": "…" }
        });
        let provenance = resolve_provenance(Some(&content)).expect("provenance");
        assert_eq!(
            provenance["canonical_id"],
            json!("justification:l2:mind-universe:underground-toolkit-v0")
        );
        assert_eq!(provenance["role_axis"], json!("narrative"));
        assert_eq!(provenance["semantic_type"], json!("justification"));
        assert_eq!(
            provenance["producing_toolkit"],
            json!("space:l2:mind-universe:underground-toolkit-v0")
        );
    }

    #[test]
    fn a_node_that_declares_its_own_provenance_overrides_what_its_id_implies() {
        let content = json!({
            "canonical_id": "objective:l2:mind-universe:underground-toolkit-v0",
            "node_type": "narrative",
            "role_axis": "thing",
            "producing_toolkit": "space:l2:mind-universe:sky-toolkit-v0"
        });
        let provenance = resolve_provenance(Some(&content)).expect("provenance");
        assert_eq!(provenance["role_axis"], json!("thing"));
        assert_eq!(
            provenance["producing_toolkit"],
            json!("space:l2:mind-universe:sky-toolkit-v0")
        );
    }

    #[test]
    fn a_producing_construct_is_read_from_an_authored_id_or_not_claimed_at_all() {
        assert_eq!(
            producing_construct("visual_binding:l2:mind-universe:underground-toolkit-v0"),
            Some("space:l2:mind-universe:underground-toolkit-v0".to_owned())
        );
        assert_eq!(
            producing_construct("code:l1:mind-universe:ollama-inference-probe-v0"),
            Some("space:l1:mind-universe:ollama-inference-probe-v0".to_owned())
        );
        // A change-ground moment carries more segments: its construct is NOT the
        // last one. Silence beats a plausible wrong attribution.
        assert_eq!(
            producing_construct("moment:l2:mind-universe:underground:change-ground:002e274edc9ea853"),
            None
        );
        // Not a level segment, and too few segments: no claim either way.
        assert_eq!(producing_construct("thing:zz:mind-universe:x"), None);
        assert_eq!(producing_construct("subentity"), None);
    }

    #[test]
    fn a_node_is_dressed_by_the_toolkit_that_produced_it_or_not_at_all() {
        let policy = VisualPolicy::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/visual-resolution-policy-v1.json"),
        )
        .unwrap();
        let mapping = json!({ "schema_version": "visual-embodiment/1-role-keyed" });
        let bindings = BTreeMap::from([(
            "visual_binding:l2:mind-universe:underground-toolkit-v0".to_owned(),
            mapping.clone(),
        )]);
        let provenance = json!({
            "producing_toolkit": "space:l2:mind-universe:underground-toolkit-v0",
            "role_axis": "narrative"
        });
        let embodiment = resolve_toolkit_embodiment(
            Some(&provenance),
            "dormant",
            "measured",
            Some(&policy),
            &bindings,
        )
        .expect("the toolkit's own binding dresses its member");
        assert_eq!(embodiment["mapping"], mapping);
        assert_eq!(embodiment["confident"], json!(true));
        // The form is the renderer's to resolve from the role-keyed archetypes.
        assert_eq!(embodiment["resolved_form"], Value::Null);

        // A node whose construct registered no binding stays unbound — it is never
        // handed the underground dress just because that binding happens to load.
        let other = json!({ "producing_toolkit": "space:l2:lumina-prime:energy-pen-v0" });
        assert_eq!(
            resolve_toolkit_embodiment(Some(&other), "dormant", "measured", Some(&policy), &bindings),
            None
        );
        // Registered, but the graph does not hold the binding: still unbound.
        assert_eq!(
            resolve_toolkit_embodiment(
                Some(&provenance),
                "dormant",
                "measured",
                Some(&policy),
                &BTreeMap::new()
            ),
            None
        );
        // No provenance at all: nothing to resolve through.
        assert_eq!(
            resolve_toolkit_embodiment(None, "dormant", "measured", Some(&policy), &bindings),
            None
        );
    }

    #[test]
    fn a_non_confident_state_never_arrives_claiming_to_be_a_confident_presence() {
        let policy = VisualPolicy::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/visual-resolution-policy-v1.json"),
        )
        .unwrap();
        let bindings = BTreeMap::from([(
            "visual_binding:l2:mind-universe:underground-toolkit-v0".to_owned(),
            json!({ "schema_version": "visual-embodiment/1-role-keyed" }),
        )]);
        let provenance =
            json!({ "producing_toolkit": "space:l2:mind-universe:underground-toolkit-v0" });
        for state in ["unknown", "not_measured", "known_absent", "measurement_failed"] {
            let embodiment =
                resolve_toolkit_embodiment(Some(&provenance), "hot", state, Some(&policy), &bindings)
                    .expect("bound");
            assert_eq!(embodiment["confident"], json!(false), "state {state}");
        }
    }

    #[test]
    fn a_node_the_store_says_nothing_about_is_not_confused_with_an_unkinded_one() {
        // No content record at all — the store holds nothing here.
        assert_eq!(resolve_provenance(None), None);
        // A content record that names no kind is NOT the same fact, and a node
        // carrying only a `content` blob still yields no invented identity.
        let unkinded = json!({ "content": { "statement": "…" } });
        assert_eq!(resolve_provenance(Some(&unkinded)), None);
    }

    #[test]
    fn built_coordinates_read_metres_and_the_civic_millimetre_frame() {
        assert_eq!(
            built_coordinates(&json!({ "x": 12.0, "y": 0.0, "z": -4.0 })),
            Some([12.0, 0.0, -4.0])
        );
        assert_eq!(
            built_coordinates(&json!({
                "east_mm": 2_500.0, "elevation_mm": 0.0, "north_mm": -1_000.0
            })),
            Some([2.5, 0.0, -1.0])
        );
    }

    #[test]
    fn a_partial_or_unnamed_placement_is_not_completed_with_a_zero() {
        // Two axes declared, one missing: NOT a placement (a zero here would be an
        // invented coordinate presented as a construction).
        assert_eq!(built_coordinates(&json!({ "x": 1.0, "y": 2.0 })), None);
        // The record a runtime gesture wrote without naming its target.
        let record = json!({ "kind": "built_position", "provenance": "built" });
        assert!(record.get("placed_node").is_none());
        assert_eq!(built_coordinates(&record), None);
    }

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
    // `role_axis` / `semantic_type` / `producing_toolkit` are the names this
    // projection asks for; `node_type` / `subtype` / `canonical_id` are the names
    // the store actually writes. Reading only the former asked the world a
    // question in a vocabulary it does not speak, and every node came back
    // unattributed. Both are read now, the explicit field first — so a node that
    // one day declares its own provenance overrides what its id implies.
    let role_axis = content
        .get("role_axis")
        .and_then(Value::as_str)
        .or_else(|| content.get("node_type").and_then(Value::as_str));
    let semantic_type = content
        .get("semantic_type")
        .and_then(Value::as_str)
        .or_else(|| content.get("subtype").and_then(Value::as_str));
    let canonical_id = content.get("canonical_id").and_then(Value::as_str);
    let producing_toolkit = content
        .get("producing_toolkit")
        .and_then(Value::as_str)
        .or_else(|| {
            content
                .get("provenance")
                .and_then(|p| p.get("producing_toolkit"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
        .or_else(|| canonical_id.and_then(producing_construct));
    if semantic_type.is_none()
        && role_axis.is_none()
        && producing_toolkit.is_none()
        && canonical_id.is_none()
    {
        return None;
    }
    let mut out = serde_json::Map::new();
    // The node's authored name, carried so a reader can say WHICH node it saw.
    // Without it the wire identified a node only by its symbol-table name, which
    // several nodes share, and no downstream resolution could key on identity.
    if let Some(value) = canonical_id {
        out.insert("canonical_id".to_owned(), json!(value));
    }
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

/// Indexes the visual bindings the world itself holds: nodes named
/// `visual_binding:…`, carried as members of the construct whose appearance they
/// declare. The mapping is the member's own `content` block, forwarded verbatim
/// so what the renderer consumes is byte-identical to what a citizen authored.
fn collect_toolkit_bindings(
    store: &UniverseStore,
    snapshot: &universe_store::UniverseSnapshot,
) -> BTreeMap<String, Value> {
    let mut bindings = BTreeMap::new();
    for entity in &snapshot.entities {
        let Some(content) = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok())
        else {
            continue;
        };
        let Some(canonical_id) = content.get("canonical_id").and_then(Value::as_str) else {
            continue;
        };
        if !canonical_id.starts_with("visual_binding:") {
            continue;
        }
        if let Some(mapping) = content.get("content") {
            bindings.insert(canonical_id.to_owned(), mapping.clone());
        }
    }
    bindings
}

/// Resolves a node's appearance through its PRODUCING TOOLKIT: provenance names
/// the construct, the policy names the binding that construct registered, and the
/// graph holds that binding. Every link must be present — a missing one yields
/// `None`, and the node stays honestly unbound rather than borrowing a dress.
///
/// The archetype for the node's `(role_axis, semantic_type)` is chosen by the
/// renderer from the role-keyed mapping carried here; the epistemic modulation is
/// resolved now, so a node in a non-confident state can never arrive claiming to
/// be a confident presence.
fn resolve_toolkit_embodiment(
    provenance: Option<&Value>,
    residency: &str,
    epistemic_state: &str,
    policy: Option<&VisualPolicy>,
    bindings: &BTreeMap<String, Value>,
) -> Option<Value> {
    let toolkit = provenance?.get("producing_toolkit")?.as_str()?;
    let member = policy?.binding_member_for(toolkit)?;
    let mapping = bindings.get(member)?;
    let confident = policy
        .and_then(|policy| policy.epistemic_modulation.get(epistemic_state))
        .is_some_and(|modulation| modulation.confident);
    Some(json!({
        "source_mapping_id": member,
        "mapping": mapping,
        "residency": residency,
        "sampled_at_ms": 0,
        // The form is NOT chosen here: a role-keyed binding declares one archetype
        // per role, and the renderer resolves it from the provenance travelling on
        // the same frame. Naming a form now would be inventing one.
        "resolved_form": Value::Null,
        "confident": confident,
    }))
}

/// The construct that produced a node, read off its AUTHORED `canonical_id`.
///
/// A canonical id is `<kind>:<level>:<world>:<construct>` — the construct segment
/// names the thing a citizen built the node as part of, and the construct's own
/// Space node is `space:<level>:<world>:<construct>`. Recovering it is reading an
/// authored name, not inferring an identity: nothing is invented, and the result
/// is exactly the key a toolkit visual binding is registered under.
///
/// Anything not in that exact shape yields `None` rather than a guess. Notably a
/// `moment:l2:mind-universe:underground:change-ground:<hash>` has more segments,
/// so its construct is NOT claimed here — an honest silence, not a bad match.
fn producing_construct(canonical_id: &str) -> Option<String> {
    let mut parts = canonical_id.split(':');
    let _kind = parts.next()?;
    let level = parts.next()?;
    let world = parts.next()?;
    let construct = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let is_level = level.len() >= 2
        && level.starts_with('l')
        && level[1..].chars().all(|c| c.is_ascii_digit());
    if !is_level || world.is_empty() || construct.is_empty() {
        return None;
    }
    Some(format!("space:{level}:{world}:{construct}"))
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

/// Resolves a `built_position.placed_node` address to an entity key. The store
/// authors it in two forms: a hex handle (`0x1000`, or the full 32-digit id) and a
/// `canonical_id` (`space:l2:lumina-prime:orientation-beacon-v0`). Anything else
/// resolves to `None` — the caller counts it as unresolved rather than guessing.
fn resolve_placed_node(
    address: &str,
    canonical: &BTreeMap<String, EntityKey>,
) -> Option<EntityKey> {
    let hex = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
        .or(Some(address).filter(|value| value.len() == 32));
    if let Some(hex) = hex {
        if let Ok(value) = u128::from_str_radix(hex, 16) {
            return Some(EntityKey(value));
        }
    }
    canonical.get(address).copied()
}

/// The coordinate a `built_position` record carries, in metres. Two authored
/// encodings: direct `x`/`y`/`z`, and the civic millimetre frame
/// (`east_mm`/`elevation_mm`/`north_mm`). ALL THREE axes must be present — a
/// partial record is not a placement, and is never completed with a zero.
fn built_coordinates(record: &Value) -> Option<[f64; 3]> {
    let axis = |direct: &str, millimetres: &str| -> Option<f64> {
        record
            .get(direct)
            .and_then(Value::as_f64)
            .or_else(|| {
                record
                    .get(millimetres)
                    .and_then(Value::as_f64)
                    .map(|value| value / 1_000.0)
            })
            .filter(|value| value.is_finite())
    };
    Some([
        axis("x", "east_mm")?,
        axis("y", "elevation_mm")?,
        axis("z", "north_mm")?,
    ])
}

/// The computed layout plus the evidence the manifest reports.
struct ComputedLayout {
    placements: BTreeMap<EntityKey, [f64; 3]>,
    /// The subset of `placements` a citizen BUILT (a `built_position` record in the
    /// store names the node). These are authoritative: the layout kernel's proposal
    /// for the same node is discarded, never blended.
    built: BTreeMap<EntityKey, [f64; 3]>,
    /// `built_position` records found in the store, and those that named a node this
    /// projection could not resolve (unknown address, or missing a coordinate). An
    /// unresolved placement is counted, never quietly treated as absent.
    built_declared: usize,
    built_unresolved: usize,
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
    // Authored placement: the `built_position` records, plus the `canonical_id` →
    // key index they address their target through.
    let mut canonical_index: BTreeMap<String, EntityKey> = BTreeMap::new();
    let mut built_records: Vec<Value> = Vec::new();
    for entity in &snapshot.entities {
        let Some(content) = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok())
        else {
            continue;
        };
        if let Some(canonical_id) = content
            .get("canonical_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            canonical_index.insert(canonical_id.to_owned(), entity.key);
        }
        if content.get("kind").and_then(Value::as_str) == Some("built_position") {
            built_records.push(content.clone());
        }
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
    let (mut placements, max_depth, residual_overlaps, clustered) = if clusters.is_empty() {
        let result = layout::compute(&input).map_err(|error| error.to_string())?;
        let placements: BTreeMap<EntityKey, [f64; 3]> = result
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

    // A citizen's placement is authoritative and is never silently overwritten by a
    // solver: the scaffold above is a proposal, and every node the store says was
    // BUILT somewhere now takes back its own coordinate. This runs AFTER hitbox
    // packing on purpose — packing may not push a built node off the spot it was
    // built on, so `residual_overlaps` describes the scaffold pass only.
    let node_set: BTreeSet<EntityKey> = node_keys.iter().copied().collect();
    let built_declared = built_records.len();
    let mut built: BTreeMap<EntityKey, [f64; 3]> = BTreeMap::new();
    let mut built_unresolved = 0usize;
    for record in &built_records {
        let placed = record
            .get("placed_node")
            .and_then(Value::as_str)
            .and_then(|address| resolve_placed_node(address, &canonical_index))
            .filter(|key| node_set.contains(key));
        match (placed, built_coordinates(record)) {
            (Some(key), Some(position)) => {
                built.insert(key, position);
                placements.insert(key, position);
            }
            // A record naming no node, an unknown address, or a partial coordinate
            // is NOT a placement. It is counted so the manifest reports the gap
            // rather than presenting the scaffold as if nothing were missing.
            _ => built_unresolved += 1,
        }
    }

    Ok(ComputedLayout {
        placements,
        built,
        built_declared,
        built_unresolved,
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
