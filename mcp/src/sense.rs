//! `sense` — bounded local perception of the Universe, *as an actor*.
//!
//! `sense` observes; it never mutates. It resolves the perceiving actor, solves
//! the physics over a *bounded* candidate cluster, then keeps a **sphere** of
//! that cluster around the actor's inferred position — reconstructing the local
//! cluster by spatial proximity, not by graph adjacency. It returns the actor's
//! POV plus a first-person text of the spheres around it.
//!
//! **Positions are inferred from the physics — there are no coordinates in the
//! store.** A node's place is the OUTPUT of the graph-native layout solver
//! (`universe-assets::layout`), which settles positions from link forces and
//! containment — not a stored `position_mm`. `sense` runs that solver over the
//! **bounded neighbourhood only** (never the whole Universe) and reports the
//! result as `inferred_from_physics`. It is a derivation, never a measurement:
//! `uncertainty` is `inferred`, never `measured`. A coordinate is emergent,
//! recomputed, and owned by the solver — the adapter never writes one.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use universe_assets::layout::{self, Layout, LayoutParams, ProfileInput, RelationInput};
use universe_core::EntityKey;
use universe_store::{ContentRef, UniverseSnapshot};
use universe_supervisor::RuntimeInventory;

use crate::frame;
use crate::pov::{self, Pov, SphereSighting};
use crate::session::ActorSession;

/// Bounded observation budget. `sense` refuses to materialise more than this in
/// one perception, so a large Universe never turns into an unbounded export —
/// and the physics is solved only over this bounded set.
const MAX_OBJECTS: usize = 64;
const MAX_RELATIONS: usize = 128;
/// Candidate ceiling for the cluster fed to the solver. Bounded so a large
/// Universe never becomes an unbounded layout, while still giving the sphere a
/// cluster wider than the object budget to cull against.
const MAX_CLUSTER: usize = MAX_OBJECTS * 6;

/// The containment predicate the layout solver treats as the Space tree.
const CONTAINMENT: &str = "PART_OF";
/// `position_mm:*` symbol nodes are vestigial genesis coordinate carriers, not
/// things to perceive.
const POSITION_PREFIX: &str = "position_mm:";

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SenseParams {
    /// The perceiving actor: a 32-hex EntityKey or a symbol name. Situated when
    /// it matches an entity, otherwise an external observer of the neighbourhood.
    #[serde(default)]
    pub actor_id: Option<String>,
    #[serde(default)]
    pub r#where: Option<String>,
    #[serde(default)]
    pub focus: Option<String>,
    #[serde(default)]
    pub scale: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    /// Sphere radius (metres) around the actor. When absent, the sphere
    /// self-calibrates to the shell that just holds the object budget.
    #[serde(default)]
    pub radius_m: Option<f64>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Uncertainty {
    /// Read directly from a real measurement (positions never are — kept for a
    /// future genuinely-measured signal).
    #[allow(dead_code)]
    Measured,
    /// Positions were inferred by the physics solver; identity may be fuzzy.
    Inferred,
    /// No Universe is mounted; nothing was measured.
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    Complete,
    BudgetExhausted,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObservedObject {
    pub key: String,
    pub semantic_type: String,
    /// The node's `canonical_id`, read from its content — its identity, distinct
    /// from its type. `None` when the entity carries no content (or no id).
    pub identity: Option<String>,
    pub generation: u32,
    /// `inferred_from_physics` when the solver placed it, else `unplaced`.
    pub position_source: &'static str,
    pub position: Option<[f64; 3]>,
    pub distance_m: Option<f64>,
    pub bearing: Option<&'static str>,
    pub origin: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObservedProcess {
    pub kind: String,
    pub name: String,
    pub activations: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub situation: serde_json::Value,
    pub pov: Option<Pov>,
    /// A deterministic first-person SVG frame of the sphere — an inferred
    /// projection, never a photograph. `None` when there is nothing placed to
    /// see (unmounted / empty).
    pub image: Option<String>,
    /// The same frame rasterised to a base64 JPEG, for MCP clients that render
    /// only raster image blocks. Same projection as `image`; `None` in lockstep.
    pub image_jpeg: Option<String>,
    pub text: String,
    pub objects: Vec<ObservedObject>,
    pub processes: Vec<ObservedProcess>,
    pub changes: serde_json::Value,
    pub affordances: Vec<String>,
    pub uncertainty: Uncertainty,
}

pub fn observe_unmounted(reason: &str) -> Observation {
    Observation {
        situation: serde_json::json!({ "mounted": false, "reason": reason }),
        pov: None,
        image: None,
        image_jpeg: None,
        text: format!("No Universe is mounted, so there is nothing to sense.\n{reason}\n"),
        objects: Vec::new(),
        processes: Vec::new(),
        changes: serde_json::json!({ "note": "no mounted Universe to read receipts from" }),
        affordances: vec!["sense".to_owned()],
        uncertainty: Uncertainty::Unknown,
    }
}

fn symbol_name(snapshot: &UniverseSnapshot, index: u32) -> String {
    snapshot
        .symbols
        .get(index as usize)
        .cloned()
        .unwrap_or_else(|| format!("symbol#{index}"))
}

/// Resolves a 32-hex EntityKey or a symbol name to an existing entity.
fn resolve_entity(snapshot: &UniverseSnapshot, wanted: &str) -> Option<EntityKey> {
    if wanted.len() == 32 {
        if let Ok(raw) = u128::from_str_radix(wanted, 16) {
            let key = EntityKey(raw);
            if snapshot.entities.iter().any(|e| e.key == key) {
                return Some(key);
            }
        }
    }
    let index = snapshot.symbols.iter().position(|s| s == wanted)? as u32;
    snapshot.entities.iter().find(|e| e.symbol == index).map(|e| e.key)
}

/// Walks the graph outward from `origin` breadth-first (skipping vestigial
/// coordinate-carrier nodes) until the candidate ceiling is reached. BFS means
/// the graph-nearest ring is taken first, so a truncated cluster is still the
/// closest `MAX_CLUSTER` nodes — the right seed for a sphere centred on the
/// actor. Returns the cluster and whether the walk was cut short (more was
/// reachable than the ceiling admits).
fn gather_cluster(snapshot: &UniverseSnapshot, origin: EntityKey) -> (Vec<EntityKey>, bool) {
    let is_placement = |key: EntityKey| {
        snapshot
            .entities
            .iter()
            .find(|e| e.key == key)
            .is_some_and(|e| symbol_name(snapshot, e.symbol).starts_with(POSITION_PREFIX))
    };
    let mut cluster = vec![origin];
    let mut seen: BTreeSet<EntityKey> = BTreeSet::from([origin]);
    let mut frontier: VecDeque<EntityKey> = VecDeque::from([origin]);
    let mut truncated = false;
    while let Some(node) = frontier.pop_front() {
        for relation in &snapshot.relations {
            if relation.source != node && relation.target != node {
                continue;
            }
            let other = if relation.source == node {
                relation.target
            } else {
                relation.source
            };
            if other == node || seen.contains(&other) || is_placement(other) {
                continue;
            }
            if cluster.len() >= MAX_CLUSTER {
                truncated = true;
                return (cluster, truncated);
            }
            seen.insert(other);
            cluster.push(other);
            frontier.push_back(other);
        }
    }
    (cluster, truncated)
}

/// Runs the graph-native layout solver over the BOUNDED neighbourhood to infer
/// positions from link forces + containment. Structural only: force profiles and
/// similarity are not sampled here (honestly neutral), so this agrees with the
/// renderer's kernel in shape but not in force detail — a declared fidelity gap.
fn local_physics_layout(snapshot: &UniverseSnapshot, keys: &[EntityKey]) -> Option<Layout> {
    let present: BTreeSet<EntityKey> = keys.iter().copied().collect();
    let relations: Vec<RelationInput> = snapshot
        .relations
        .iter()
        .filter(|r| present.contains(&r.source) && present.contains(&r.target))
        .map(|r| RelationInput {
            source: r.source,
            target: r.target,
            predicate: symbol_name(snapshot, r.predicate),
        })
        .collect();
    let profiles: BTreeMap<String, ProfileInput> = BTreeMap::new();
    let containment: BTreeSet<String> = BTreeSet::from([CONTAINMENT.to_owned()]);
    let similarity = |_: EntityKey, _: EntityKey| 0.0;
    let input = layout::project(
        keys,
        &relations,
        &profiles,
        &containment,
        &similarity,
        layout::DEFAULT_RADIUS,
        LayoutParams::default(),
    );
    layout::compute(&input).ok()
}

pub fn observe(
    snapshot: &UniverseSnapshot,
    inventory: &RuntimeInventory,
    params: &SenseParams,
    session: Option<&ActorSession>,
    read_content: &dyn Fn(&ContentRef) -> Option<serde_json::Value>,
) -> Observation {
    let universe = snapshot.universe.to_string();
    let revision = snapshot.revision.0;
    let tick = snapshot.tick.0;

    let actor_key = params.actor_id.as_deref().and_then(|a| resolve_entity(snapshot, a));

    // Origin: `where`, else the actor, else the snapshot origin.
    let origin = params
        .r#where
        .as_deref()
        .and_then(|w| resolve_entity(snapshot, w))
        .or(actor_key)
        .or_else(|| snapshot.entities.first().map(|e| e.key));

    let Some(origin) = origin else {
        return empty_observation(&universe, revision, tick, inventory, session);
    };

    // A physical SPHERE around the actor, reconstructed from the physics — not a
    // graph one-hop. Gather a bounded candidate CLUSTER by walking outward from
    // the origin, solve the layout over that whole cluster, then keep only what
    // falls inside a sphere of `radius_m` around the actor's inferred position.
    // The sphere is a spatial budget: exactly the "local observation" CLAUDE.md
    // prescribes, but bounded by distance rather than by adjacency depth.
    let (candidates, bfs_truncated) = gather_cluster(snapshot, origin);

    // Infer positions from the physics over the whole candidate cluster, so the
    // sphere is culled against real solved coordinates (never one-hop adjacency).
    let layout = local_physics_layout(snapshot, &candidates);
    let position_of = |key: EntityKey| -> Option<[f64; 3]> {
        layout.as_ref().and_then(|l| l.position(key))
    };

    // Sphere centre: the actor if it is placed, else the origin, else the cluster
    // centroid — never an invented coordinate.
    let center = actor_key
        .and_then(position_of)
        .or_else(|| position_of(origin))
        .or_else(|| centroid(candidates.iter().filter_map(|k| position_of(*k))))
        .unwrap_or([0.0, 0.0, 0.0]);

    // Rank the placed cluster by physical distance to the centre.
    let mut ranked: Vec<(EntityKey, f64)> = candidates
        .iter()
        .filter_map(|k| position_of(*k).map(|p| (*k, pov::distance(center, p))))
        .collect();
    ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Sphere radius: explicit `radius_m`, else self-calibrating to the shell that
    // just holds the object budget (the MAX_OBJECTS-th nearest). A cluster
    // smaller than the budget yields the whole cluster.
    let sphere_radius = params.radius_m.filter(|r| *r > 0.0).unwrap_or_else(|| {
        ranked
            .get(MAX_OBJECTS - 1)
            .or_else(|| ranked.last())
            .map(|(_, d)| *d)
            .unwrap_or(0.0)
    });
    let within: Vec<EntityKey> = ranked
        .iter()
        .take_while(|(_, d)| *d <= sphere_radius + f64::EPSILON)
        .map(|(k, _)| *k)
        .collect();
    let within_count = within.len();
    // Budget: more nodes sit inside the sphere than we will materialise.
    let sphere_truncated = within_count > MAX_OBJECTS;
    let mut keys: Vec<EntityKey> = vec![origin];
    for key in within {
        if keys.len() >= MAX_OBJECTS {
            break;
        }
        if key != origin {
            keys.push(key);
        }
    }
    let truncated = bfs_truncated || sphere_truncated;

    // Eye: a situated actor sits at the sphere centre; anyone else observes the
    // sphere from just outside it (never an invented coordinate).
    let (eye, eye_source) = match actor_key.and_then(position_of) {
        Some(pos) => (pos, "situated"),
        None => (external_vantage(&keys, &position_of), "external_observer"),
    };
    let look_at = centroid(keys.iter().filter_map(|k| {
        (Some(*k) != actor_key).then(|| position_of(*k)).flatten()
    }))
    .unwrap_or(eye);
    let (yaw, pitch) = pov::orientation_from_look_at(eye, look_at);

    let actor_label = params
        .actor_id
        .clone()
        .or_else(|| actor_key.map(|k| label_of_key(snapshot, k)))
        .unwrap_or_else(|| "anonymous".to_owned());
    let pov = Pov {
        actor: actor_key.map(|k| k.to_string()).unwrap_or(actor_label.clone()),
        generated: actor_key.is_none(),
        eye,
        eye_source,
        look_at,
        yaw,
        pitch,
        projection: "physics_sphere",
    };

    let mut objects = Vec::new();
    let mut sightings = Vec::new();
    let mut unplaced = 0usize;
    for key in &keys {
        let Some(entity) = snapshot.entities.iter().find(|e| e.key == *key) else {
            continue;
        };
        let semantic_type = symbol_name(snapshot, entity.symbol);
        // Identity lives in the content (`canonical_id`), distinct from the type.
        // Reading it is what turns a wall of generic `narrative`/`thing` objects
        // into named ones (Balise Zéro, the energy pen, ...).
        let identity = entity
            .content
            .as_ref()
            .and_then(|c| read_content(c))
            .and_then(|v| v.get("canonical_id").and_then(|id| id.as_str()).map(str::to_owned));
        // The sighting label prefers identity; the type is the fallback.
        let label = identity.clone().unwrap_or_else(|| semantic_type.clone());
        let (position, source, distance_m, bearing) = match position_of(*key) {
            Some(p) => (
                Some(p),
                "inferred_from_physics",
                Some(pov::distance(eye, p)),
                Some(pov::bearing(eye, yaw, p)),
            ),
            None => {
                unplaced += 1;
                (None, "unplaced", None, None)
            }
        };
        if let (Some(p), Some(d), Some(b)) = (position, distance_m, bearing) {
            if Some(*key) != actor_key {
                sightings.push(SphereSighting {
                    key: entity.key.to_string(),
                    label: label.clone(),
                    primitive: "sphere",
                    position: p,
                    distance_m: d,
                    bearing: b,
                });
            }
        }
        objects.push(ObservedObject {
            key: entity.key.to_string(),
            semantic_type,
            identity,
            generation: entity.generation,
            position_source: source,
            position,
            distance_m,
            bearing,
            origin: entity.key == origin,
        });
    }
    sightings.sort_by(|a, b| a.distance_m.partial_cmp(&b.distance_m).unwrap());

    // Positions are inferred by the solver, never observed: always `inferred`.
    let uncertainty = Uncertainty::Inferred;
    let mut text = pov::render_text(
        &actor_label,
        &pov,
        &universe,
        revision,
        tick,
        &sightings,
        "inferred",
    );
    if unplaced > 0 {
        text.push_str(&format!("({unplaced} more are present but the solver placed no position.)\n"));
    }
    text.push_str(&format!(
        "(A physics sphere of radius {sphere_radius:.1} m around you holds {} of {within_count} nearby objects; positions are inferred from local physics.)\n",
        keys.len(),
    ));

    let completion = if truncated {
        Completion::BudgetExhausted
    } else {
        Completion::Complete
    };

    // The first-person frame: a deterministic, inferred projection of the sphere.
    let caption =
        format!("{universe} rev {revision} tick {tick} — inferred physics-sphere projection");
    let image = (!sightings.is_empty()).then(|| frame::render_svg(&pov, &sightings, &caption));
    // The raster twin of the same frame, base64-encoded so it can ride in an MCP
    // `image` content block for clients that render only raster.
    let image_jpeg = (!sightings.is_empty())
        .then(|| crate::raster::base64(&crate::raster::render_jpeg(&pov, &sightings, &caption)));

    Observation {
        situation: serde_json::json!({
            "mounted": true,
            "universe": universe,
            "revision": revision,
            "tick": tick,
            "origin": origin.to_string(),
            "actor": params.actor_id,
            "where": params.r#where,
            "focus": params.focus,
            "scale": params.scale,
            "completion": completion,
            "frame": "physics_sphere",
            "sphere": {
                "center": center,
                "radius_m": sphere_radius,
                "radius_source": if params.radius_m.filter(|r| *r > 0.0).is_some() { "requested" } else { "self_calibrated_to_budget" },
                "cluster_candidates": candidates.len(),
                "within_sphere": within_count,
                "materialised": keys.len(),
            },
            "session": session.map(|s| s.passport()),
            "budget": { "max_objects": MAX_OBJECTS, "max_relations": MAX_RELATIONS, "max_cluster": MAX_CLUSTER },
        }),
        pov: Some(pov),
        image,
        image_jpeg,
        text,
        objects,
        processes: processes_of(inventory),
        changes: serde_json::json!({
            "revision": revision,
            "tick": tick,
            "since": params.since,
            "note": "revision/tick are measured; an itemised receipt log for `since` is a declared gap",
        }),
        affordances: vec!["sense".to_owned(), "act".to_owned()],
        uncertainty,
    }
}

/// A vantage point outside the neighbourhood, for an actor that has no inferred
/// position of its own (an external observer). Derived from the placed set, so
/// it is never a hardcoded coordinate.
fn external_vantage(
    keys: &[EntityKey],
    position_of: &impl Fn(EntityKey) -> Option<[f64; 3]>,
) -> [f64; 3] {
    let Some(c) = centroid(keys.iter().filter_map(|k| position_of(*k))) else {
        return [0.0, 0.0, 0.0];
    };
    let spread = keys
        .iter()
        .filter_map(|k| position_of(*k))
        .map(|p| pov::distance(c, p))
        .fold(1.0_f64, f64::max);
    [c[0], c[1] + spread * 0.6, c[2] + spread * 1.8]
}

fn empty_observation(
    universe: &str,
    revision: u64,
    tick: u64,
    inventory: &RuntimeInventory,
    session: Option<&ActorSession>,
) -> Observation {
    Observation {
        situation: serde_json::json!({
            "mounted": true, "universe": universe, "revision": revision, "tick": tick,
            "session": session.map(|s| s.passport()),
            "note": "the mounted Universe has no entities",
        }),
        pov: None,
        image: None,
        image_jpeg: None,
        text: format!("The mounted Universe (revision {revision}) has no entities to sense.\n"),
        objects: Vec::new(),
        processes: processes_of(inventory),
        changes: serde_json::json!({ "revision": revision, "tick": tick }),
        affordances: vec!["sense".to_owned(), "act".to_owned()],
        uncertainty: Uncertainty::Inferred,
    }
}

fn processes_of(inventory: &RuntimeInventory) -> Vec<ObservedProcess> {
    inventory
        .mechanisms
        .iter()
        .map(|m| ObservedProcess {
            kind: format!("{:?}", m.kind),
            name: m.name.clone(),
            activations: m.activations,
        })
        .collect()
}

fn label_of_key(snapshot: &UniverseSnapshot, key: EntityKey) -> String {
    snapshot
        .entities
        .iter()
        .find(|e| e.key == key)
        .map(|e| symbol_name(snapshot, e.symbol))
        .unwrap_or_else(|| key.to_string())
}

fn centroid(points: impl Iterator<Item = [f64; 3]>) -> Option<[f64; 3]> {
    let mut acc = [0.0; 3];
    let mut n = 0.0;
    for p in points {
        acc[0] += p[0];
        acc[1] += p[1];
        acc[2] += p[2];
        n += 1.0;
    }
    (n > 0.0).then(|| [acc[0] / n, acc[1] / n, acc[2] / n])
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::{RelationKey, Revision, Tick, UniverseId};
    use universe_store::{EntityRecord, RelationRecord};

    fn linked_world() -> UniverseSnapshot {
        let ent = |key: u128, symbol: u32| EntityRecord {
            key: EntityKey(key),
            generation: 0,
            symbol,
            content: None,
        };
        UniverseSnapshot {
            format_version: 0,
            universe: UniverseId(0xC0DE),
            revision: Revision(7),
            tick: Tick(3),
            symbols: vec!["root".into(), "leaf".into(), "GROUNDS".into()],
            entities: vec![ent(1, 0), ent(2, 1)],
            relations: vec![RelationRecord {
                key: RelationKey(1),
                generation: 0,
                source: EntityKey(1),
                target: EntityKey(2),
                predicate: 2,
                content: None,
            }],
            event_keys: Default::default(),
        }
    }

    fn inv() -> RuntimeInventory {
        RuntimeInventory::default()
    }

    /// A content reader that reads nothing — the default for worlds whose
    /// entities carry no content.
    fn no_content(_: &ContentRef) -> Option<serde_json::Value> {
        None
    }

    /// A dummy ContentRef (never dereferenced — the reader closure ignores it).
    fn dummy_ref() -> ContentRef {
        ContentRef {
            pointer: universe_core::ContentPtr { segment: 0, offset: 0, length: 0 },
            sha256: "0".repeat(64),
        }
    }

    fn params(actor: Option<&str>) -> SenseParams {
        SenseParams {
            actor_id: actor.map(str::to_owned),
            ..SenseParams::default()
        }
    }

    #[test]
    fn a_situated_actor_gets_physics_inferred_positions() {
        let obs = observe(&linked_world(), &inv(), &params(Some("root")), None, &no_content);
        // Never claims measured: positions are inferred by the solver.
        assert_eq!(obs.uncertainty, Uncertainty::Inferred);
        assert_eq!(obs.pov.as_ref().unwrap().eye_source, "situated");
        let leaf = obs.objects.iter().find(|o| o.semantic_type == "leaf").unwrap();
        assert_eq!(leaf.position_source, "inferred_from_physics");
        assert!(leaf.position.is_some());
        assert!(obs.text.contains("physics sphere"));
    }

    #[test]
    fn an_objects_identity_is_read_from_its_canonical_id() {
        // A world whose leaf carries content (a ContentRef), so the reader is
        // consulted for its identity.
        let mut world = linked_world();
        world.entities[1].content = Some(dummy_ref());
        // The reader returns the leaf's canonical_id, as the real store would.
        let reader = |_: &ContentRef| {
            Some(serde_json::json!({ "canonical_id": "space:l2:lumina-prime:orientation-beacon-v0" }))
        };
        let obs = observe(&world, &inv(), &params(Some("root")), None, &reader);
        let leaf = obs.objects.iter().find(|o| o.semantic_type == "leaf").unwrap();
        // Type is unchanged; identity is now populated from the content.
        assert_eq!(
            leaf.identity.as_deref(),
            Some("space:l2:lumina-prime:orientation-beacon-v0")
        );
        // The root, which carries no content, has no identity — honestly None.
        let root = obs.objects.iter().find(|o| o.origin).unwrap();
        assert_eq!(root.identity, None);
    }

    #[test]
    fn the_situation_reports_a_measured_physics_sphere() {
        let obs = observe(&linked_world(), &inv(), &params(Some("root")), None, &no_content);
        let sphere = &obs.situation["sphere"];
        // Default: the radius self-calibrates to the object budget.
        assert_eq!(sphere["radius_source"], "self_calibrated_to_budget");
        // Every materialised object was culled from the candidate cluster.
        assert_eq!(sphere["materialised"], obs.objects.len());
        assert!(sphere["radius_m"].as_f64().unwrap() >= 0.0);
        assert_eq!(obs.pov.as_ref().unwrap().projection, "physics_sphere");
    }

    #[test]
    fn an_explicit_radius_is_honoured_and_reported() {
        let mut p = params(Some("root"));
        p.radius_m = Some(1000.0); // a wide sphere: the whole cluster is within
        let obs = observe(&linked_world(), &inv(), &p, None, &no_content);
        let sphere = &obs.situation["sphere"];
        assert_eq!(sphere["radius_source"], "requested");
        assert_eq!(sphere["radius_m"].as_f64().unwrap(), 1000.0);
        // Both nodes of the linked world sit inside a 1 km sphere.
        assert_eq!(sphere["within_sphere"].as_u64().unwrap(), 2);
    }

    #[test]
    fn an_unknown_actor_observes_from_outside() {
        let obs = observe(&linked_world(), &inv(), &params(Some("visitor-42")), None, &no_content);
        assert_eq!(obs.pov.as_ref().unwrap().eye_source, "external_observer");
        assert_eq!(obs.uncertainty, Uncertainty::Inferred);
    }

    #[test]
    fn no_object_carries_an_authored_or_measured_position() {
        let obs = observe(&linked_world(), &inv(), &params(Some("root")), None, &no_content);
        for o in &obs.objects {
            assert!(matches!(o.position_source, "inferred_from_physics" | "unplaced"));
        }
    }

    #[test]
    fn unmounted_sense_is_unknown_and_invents_nothing() {
        let obs = observe_unmounted("no store");
        assert_eq!(obs.uncertainty, Uncertainty::Unknown);
        assert!(obs.objects.is_empty());
        assert!(obs.pov.is_none());
    }
}
