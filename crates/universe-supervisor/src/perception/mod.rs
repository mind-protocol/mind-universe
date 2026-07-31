//! `perception` — bounded local perception of the Universe, *as an actor*.
//!
//! This is the observation-builder: it takes a committed [`UniverseSnapshot`] and
//! produces a bounded [`Observation`] — the "build a bounded WorldObservation"
//! step both the headless MCP `sense` adapter and the endogenous L1 loop share.
//! It lives in the supervisor library (not in the detached `mcp` binary) so BOTH
//! callers reach the same code; IMAGE rendering (SVG/JPEG) is a transport concern
//! and stays in the MCP adapter, fed from this observation's [`Pov`] + `sightings`.
//!
//! Perception observes; it never mutates. It resolves the perceiving actor,
//! solves the physics over a *bounded* candidate cluster, then keeps a **sphere**
//! of that cluster around the actor's inferred position — reconstructing the local
//! cluster by spatial proximity, not by graph adjacency. It returns the actor's
//! POV plus a first-person text of the spheres around it.
//!
//! **Positions are inferred from the physics — there are no coordinates in the
//! store.** A node's place is the OUTPUT of the graph-native layout solver
//! (`universe-assets::layout`), which settles positions from link forces and
//! containment — not a stored `position_mm`. Perception runs that solver over the
//! **bounded neighbourhood only** (never the whole Universe) and reports the
//! result as `inferred_from_physics`. It is a derivation, never a measurement:
//! `uncertainty` is `inferred`, never `measured`. A coordinate is emergent,
//! recomputed, and owned by the solver — the observer never writes one.

pub mod pov;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use universe_assets::layout::{self, Layout, LayoutParams, ProfileInput, RelationInput};
use universe_core::EntityKey;
use universe_store::{ContentRef, UniverseSnapshot};

use crate::RuntimeInventory;
// Re-exported so both callers (the MCP adapter, the future L1 loop) reach the
// POV geometry types through `universe_supervisor::perception::{Pov, SphereSighting}`;
// this `pub use` also brings the names into scope for this module.
pub use pov::{Pov, SphereSighting};

/// Bounded observation budget. Perception refuses to materialise more than this
/// in one observation, so a large Universe never turns into an unbounded export —
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
/// The type+level convention for an L1 actor's identity: `actor:l1:<ns>:<name>`,
/// read from the node's `canonical_id`. The level lives in the id, not the type
/// symbol — actors are typed `actor` regardless of level.
const L1_ACTOR_PREFIX: &str = "actor:l1:";
/// Bounded: resolving an inhabitant to embody reads content for at most this many
/// actor-typed nodes. It is a selection, not an export — it must never hydrate
/// the whole Universe just to find someone to look through.
const MAX_ACTOR_SCAN: usize = MAX_CLUSTER;

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
    /// A short, readable label for a human/LLM reader: the node's AUTHORED
    /// `content.name` — its Space name, the world-authored human label — when it
    /// carries one; else an honest, language-neutral humanisation of the canonical
    /// id's final segment (or the type). Never a hand-maintained dictionary. The
    /// human `text` render groups nodes and labels them from the same store data,
    /// so the structured `name` and the rendered label agree.
    pub name: String,
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

/// The genre of an affordance-part, per the object model: an object is a
/// thing-node (its body) plus affordances (parts of that body).
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AffordanceGenre {
    /// Perceived only — a label / state light / halo. Carries NO precondition or
    /// effect cluster; it is just rendered. Actioning it does nothing.
    Visible,
    /// Invocable now: its precondition gate is fired (or trivially satisfied, as
    /// for a read-only surface whose precondition is `none`).
    Active,
    /// Present but not yet invocable — its precondition cluster cannot fire yet
    /// (a missing capability, or a physics gate that has not fired). An inert
    /// affordance IS a need.
    Inert,
}

/// Whether the affordance's precondition gate has fired — the single boolean a
/// 2B model reads ("invocable? = its precondition gate has fired"). `Unknown` is
/// honest: the gate's fired-state is not readable from this bounded read path
/// (no live atom energy in the committed snapshot, no actor authority-set in
/// view), so we never fabricate `true`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invocable {
    Yes,
    No,
    Unknown,
}

impl Serialize for Invocable {
    /// Serialises to exactly `true | false | "unknown"` — a boolean the 2B model
    /// reads directly, or the honest `"unknown"` tag when the gate is not
    /// measurable.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Invocable::Yes => s.serialize_bool(true),
            Invocable::No => s.serialize_bool(false),
            Invocable::Unknown => s.serialize_str("unknown"),
        }
    }
}

/// One affordance-part of a perceived object: a verb the model can consider,
/// with its genre, its (honest) invocability, and the clusters under it. Atoms
/// are the physics *under* the affordance; the model sees only this shape.
#[derive(Clone, Debug, Serialize)]
pub struct Affordance {
    /// A stable id for this affordance-part: `<target>#<verb>`.
    pub key: String,
    /// The object (thing-node) this affordance is a part of — its entity key.
    pub target: String,
    /// The object's identity/type label, for a reader holding only this list.
    pub target_label: String,
    pub verb: String,
    pub genre: AffordanceGenre,
    pub invocable: Invocable,
    /// The precondition-cluster, rendered as a readable string. `None` = NO
    /// cluster (a visible affordance is rendered, never gated).
    pub precondition: Option<String>,
    /// The effect-cluster, rendered as a readable string. `None` = no effect (a
    /// read-only or visible affordance produces none).
    pub effect: Option<String>,
    pub justification: Option<String>,
    /// The gate kind — WHY `invocable` is what it is:
    /// `read_only` | `capability` | `physics_trigger` | `display`.
    pub gate: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub situation: serde_json::Value,
    pub pov: Option<Pov>,
    /// The visible spheres around the actor, sorted nearest-first. This is the
    /// projection an image renderer (a transport concern, e.g. the MCP adapter)
    /// draws the first-person frame from — the observation-builder places them but
    /// never rasterises: an image is a transport, not a perception, concern.
    pub sightings: Vec<SphereSighting>,
    pub text: String,
    pub objects: Vec<ObservedObject>,
    pub processes: Vec<ObservedProcess>,
    pub changes: serde_json::Value,
    /// The REAL affordance-parts of the perceived objects, read from the graph
    /// and classified visible / active / inert — NOT the adapter's tool names.
    /// Each item references its object via `target`.
    pub affordances: Vec<Affordance>,
    pub uncertainty: Uncertainty,
}

pub fn observe_unmounted(reason: &str) -> Observation {
    Observation {
        situation: serde_json::json!({ "mounted": false, "reason": reason }),
        pov: None,
        sightings: Vec::new(),
        text: format!("No Universe is mounted, so there is nothing to sense.\n{reason}\n"),
        objects: Vec::new(),
        processes: Vec::new(),
        changes: serde_json::json!({ "note": "no mounted Universe to read receipts from" }),
        // No mounted world -> no perceived objects -> no world affordances.
        // Honest absence, never the adapter's own tool names.
        affordances: Vec::new(),
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
    session_passport: Option<serde_json::Value>,
    read_content: &dyn Fn(&ContentRef) -> Option<serde_json::Value>,
) -> Observation {
    let universe = snapshot.universe.to_string();
    let revision = snapshot.revision.0;
    let tick = snapshot.tick.0;

    // Resolve the perceiving actor. A NAMED actor is honoured (an unknown name
    // still yields an external-observer vantage, unchanged). When the caller
    // names NONE, embody an arbitrary L1 inhabitant so `sense` gives a situated
    // POV rather than floating outside — the "through whose eyes?" default.
    // `actor_resolution` records which path was taken, so the caller is never
    // misled into thinking it chose the inhabitant.
    let (actor_key, actor_resolution) = match params.actor_id.as_deref() {
        Some(named) => match resolve_entity(snapshot, named) {
            // A 32-hex EntityKey or a type SYMBOL name resolved directly.
            Some(key) => (Some(key), "named"),
            // No symbol/key matched. A session-style id (e.g. `claude:<uuid>`)
            // names no symbol, so match it against actor nodes' stored CONTENT
            // `canonical_id` — embodying the actor at its OWN placement (situated),
            // no hardcoded vantage needed. Honest tag when it hits, plain external
            // fallback when it does not.
            None => match resolve_actor_by_canonical_id(snapshot, read_content, named) {
                Some(key) => (Some(key), "named_canonical"),
                None => (None, "named"),
            },
        },
        None => {
            let seed = snapshot.revision.0 ^ snapshot.tick.0.rotate_left(32);
            match resolve_random_l1_actor(snapshot, read_content, seed) {
                Some(key) => (Some(key), "random_l1"),
                None => (None, "no_l1_actor_external"),
            }
        }
    };

    // Origin: `where`, else the actor, else the snapshot origin.
    let origin = params
        .r#where
        .as_deref()
        .and_then(|w| resolve_entity(snapshot, w))
        .or(actor_key)
        .or_else(|| snapshot.entities.first().map(|e| e.key));

    let Some(origin) = origin else {
        return empty_observation(&universe, revision, tick, inventory, session_passport);
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
        .or_else(|| actor_key.and_then(|k| identity_or_type(snapshot, k, read_content)))
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
    let mut affordances: Vec<Affordance> = Vec::new();
    // The human `text` groups perceived nodes as it goes (their content is in hand
    // here): a construct's anatomy faces (which share a canonical-id tail) and a
    // change-ground moment stream (per-change hash dropped) collapse to ONE line
    // via `group_key`; that line's NAME is derived from the node's own store data —
    // its `content.name` if present, else its humanised canonical_id — never a
    // per-construct dictionary. Purely a projection: `objects`/`affordances` keep
    // one entry per entity.
    let mut node_lines: Vec<pov::NodeLine> = Vec::new();
    let mut group_index: BTreeMap<String, usize> = BTreeMap::new();
    let mut origin_name = "cet endroit".to_owned();
    for key in &keys {
        let Some(entity) = snapshot.entities.iter().find(|e| e.key == *key) else {
            continue;
        };
        let semantic_type = symbol_name(snapshot, entity.symbol);
        // Read the content wrapper ONCE and reuse it for identity + name + affordances.
        let wrapper = entity.content.as_ref().and_then(|c| read_content(c));
        // Identity lives in the content (`canonical_id`), distinct from the type.
        // Reading it is what turns a wall of generic `narrative`/`thing` objects
        // into named ones (the beacon, the energy pen, ...).
        let identity = wrapper
            .as_ref()
            .and_then(|v| v.get("canonical_id").and_then(|id| id.as_str()).map(str::to_owned));
        // The node's OWN display name for the text, if it stores one (`content.name`)
        // — the store's data, preferred over any derived slug. The real store nests
        // authored content under `.content`; a flat fixture value is its own content.
        let content_name = wrapper.as_ref().and_then(|w| {
            w.get("content")
                .unwrap_or(w)
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        // The sighting label prefers identity; the type is the fallback.
        let label = identity.clone().unwrap_or_else(|| semantic_type.clone());
        // The short, readable name the structured reader shares (language-neutral).
        let name = readable_label(identity.as_deref(), &semantic_type);
        // The object's REAL affordance-parts, read from its stored content and
        // classified visible / active / inert. Read-only: it only reads the
        // content wrapper `sense` already holds; it never mutates.
        let affordances_before = affordances.len();
        if let Some(w) = wrapper.as_ref() {
            affordances.extend(derive_affordances(&entity.key.to_string(), &label, w));
        }
        // Fold this node into its text line: the group collapses anatomy faces /
        // moment streams; the display name comes from the node's own data, and the
        // node's fresh affordance verbs are humanised into short action phrases.
        let gkey = group_key(identity.as_deref(), &semantic_type);
        let content_name_ref = content_name.as_deref();
        let identity_ref = identity.as_deref();
        let gi = *group_index.entry(gkey.clone()).or_insert_with(|| {
            let display = group_display_name(&gkey, content_name_ref, identity_ref, &semantic_type);
            node_lines.push(pov::NodeLine { name: display, actions: Vec::new() });
            node_lines.len() - 1
        });
        for aff in &affordances[affordances_before..] {
            let phrase = humanize_verb(&aff.verb);
            if !phrase.is_empty() && !node_lines[gi].actions.contains(&phrase) {
                node_lines[gi].actions.push(phrase);
            }
        }
        if entity.key == origin {
            origin_name = node_lines[gi].name.clone();
        }
        let (position, source, distance_m, bearing) = match position_of(*key) {
            Some(p) => (
                Some(p),
                "inferred_from_physics",
                Some(pov::distance(eye, p)),
                Some(pov::bearing(eye, yaw, p)),
            ),
            None => (None, "unplaced", None, None),
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
            name,
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

    // The human `text` is a filtered projection of the structured fields: a prose
    // situation line at the world's DERIVED place (the origin's namespace, never a
    // universe hex id, never physics-sphere wording), then the grouped nodes by
    // NAME with each node's affordances beneath it as short, humanised phrases. No
    // distance/bearing/position/type — a readable situation the 2B loop and MCP
    // `sense` both read. Names, verbs and place are all DERIVED from the store
    // (content.name / canonical_id / the raw verb / the namespace slug), never a
    // hardcoded dictionary. The node grouping was built alongside `objects` above.
    let place = place_name(&objects);
    let situated = pov.eye_source != "external_observer";
    let text = pov::render_text(place.as_deref(), revision, situated, &origin_name, &node_lines);

    let completion = if truncated {
        Completion::BudgetExhausted
    } else {
        Completion::Complete
    };

    Observation {
        situation: serde_json::json!({
            "mounted": true,
            "universe": universe,
            "revision": revision,
            "tick": tick,
            "origin": origin.to_string(),
            "actor": params.actor_id,
            // How the perceiving actor was resolved: `named` (a hex key or type
            // symbol matched, or nothing matched -> external observer),
            // `named_canonical` (the id matched an actor node's stored canonical_id,
            // e.g. a `claude:<uuid>` session -> embodied at its own placement),
            // `random_l1` (none given, so an arbitrary L1 inhabitant is embodied),
            // or `no_l1_actor_external` (none given and none exists -> observe from
            // outside). `embodied_actor` is the key actually looked through, if any.
            "actor_resolution": actor_resolution,
            "embodied_actor": actor_key.map(|k| k.to_string()),
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
            "session": session_passport,
            "budget": { "max_objects": MAX_OBJECTS, "max_relations": MAX_RELATIONS, "max_cluster": MAX_CLUSTER },
        }),
        pov: Some(pov),
        sightings,
        text,
        objects,
        processes: processes_of(inventory),
        changes: serde_json::json!({
            "revision": revision,
            "tick": tick,
            "since": params.since,
            "note": "revision/tick are measured; an itemised receipt log for `since` is a declared gap",
        }),
        affordances,
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
    session_passport: Option<serde_json::Value>,
) -> Observation {
    Observation {
        situation: serde_json::json!({
            "mounted": true, "universe": universe, "revision": revision, "tick": tick,
            "session": session_passport,
            "note": "the mounted Universe has no entities",
        }),
        pov: None,
        sightings: Vec::new(),
        text: format!("The mounted Universe (revision {revision}) has no entities to sense.\n"),
        objects: Vec::new(),
        processes: processes_of(inventory),
        changes: serde_json::json!({ "revision": revision, "tick": tick }),
        // The mounted Universe has no entities -> nothing perceived -> no world
        // affordances. Honest emptiness, never the adapter's tool names.
        affordances: Vec::new(),
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

/// A human label for a key: its `canonical_id` identity when the content carries
/// one, else its type symbol name. Used to name the embodied inhabitant in the
/// first-person text without pretending an anonymous type is an identity.
fn identity_or_type(
    snapshot: &UniverseSnapshot,
    key: EntityKey,
    read_content: &dyn Fn(&ContentRef) -> Option<serde_json::Value>,
) -> Option<String> {
    let entity = snapshot.entities.iter().find(|e| e.key == key)?;
    let identity = entity
        .content
        .as_ref()
        .and_then(|c| read_content(c))
        .and_then(|v| v.get("canonical_id").and_then(|id| id.as_str()).map(str::to_owned));
    Some(identity.unwrap_or_else(|| symbol_name(snapshot, entity.symbol)))
}

/// Resolves an arbitrary L1 actor to embody when the caller named none.
///
/// "Random" here means *an arbitrary inhabitant*, not a fixed one: the pick is
/// seeded from the live revision+tick (then mixed), so it is reproducible within
/// a revision — a stable perception — yet rotates as the city lives. There is no
/// RNG and no clock read inside `observe`; the seed is a pure function of the
/// snapshot, so the choice stays testable.
///
/// It reads content only for `actor`-typed nodes (the level lives in the
/// `canonical_id`, so the content must be consulted) and stops after
/// `MAX_ACTOR_SCAN` — a bounded selection, never a full-Universe hydration.
/// Returns `None` when no L1 actor is in view; the caller then falls back to an
/// honest external-observer vantage rather than substituting a non-L1 actor.
fn resolve_random_l1_actor(
    snapshot: &UniverseSnapshot,
    read_content: &dyn Fn(&ContentRef) -> Option<serde_json::Value>,
    seed: u64,
) -> Option<EntityKey> {
    let mut candidates: Vec<EntityKey> = Vec::new();
    let mut examined = 0usize;
    for entity in &snapshot.entities {
        if examined >= MAX_ACTOR_SCAN {
            break;
        }
        // Cheap type gate first: only actor-typed nodes can be actors, so the
        // rest never trigger a content read. Casing-proof — genesis types
        // `Actor`, the canonical store types `actor`.
        if !symbol_name(snapshot, entity.symbol).eq_ignore_ascii_case("actor") {
            continue;
        }
        examined += 1;
        let is_l1 = entity
            .content
            .as_ref()
            .and_then(|c| read_content(c))
            .and_then(|v| v.get("canonical_id").and_then(|id| id.as_str()).map(str::to_owned))
            .is_some_and(|id| id.starts_with(L1_ACTOR_PREFIX));
        if is_l1 {
            candidates.push(entity.key);
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let pick = (mix64(seed) % candidates.len() as u64) as usize;
    Some(candidates[pick])
}

/// Resolves a session-style `actor_id` (e.g. `claude:<uuid>`) to the L1 actor
/// node whose stored `canonical_id` carries the same session tail — the data-
/// driven embodiment path. An actor that names itself by its session id (which
/// matches no symbol or key) is matched to its OWN placed body, so it perceives
/// as SITUATED with no hardcoded vantage.
///
/// The session id is sanitised to a single canonical-id tail segment (the segment
/// separator `:` cannot appear inside one segment, so `claude:<uuid>` is carried
/// as `claude-<uuid>`), then compared against each actor node's `canonical_id`,
/// which must be an L1 actor id (`actor:l1:...`) equalling — or ending with — that
/// tail. Bounded exactly like `resolve_random_l1_actor`: it reads content only for
/// actor-typed nodes and stops after `MAX_ACTOR_SCAN`, never a full hydration.
fn resolve_actor_by_canonical_id(
    snapshot: &UniverseSnapshot,
    read_content: &dyn Fn(&ContentRef) -> Option<serde_json::Value>,
    actor_id: &str,
) -> Option<EntityKey> {
    let sanitized = sanitize_session_id(actor_id);
    if sanitized.is_empty() {
        return None;
    }
    let mut examined = 0usize;
    for entity in &snapshot.entities {
        if examined >= MAX_ACTOR_SCAN {
            break;
        }
        // Cheap type gate first: only actor-typed nodes can be actors, so the rest
        // never trigger a content read. Casing-proof, like the L1 scan.
        if !symbol_name(snapshot, entity.symbol).eq_ignore_ascii_case("actor") {
            continue;
        }
        examined += 1;
        let canonical = entity
            .content
            .as_ref()
            .and_then(|c| read_content(c))
            .and_then(|v| v.get("canonical_id").and_then(|id| id.as_str()).map(str::to_owned));
        if let Some(id) = canonical {
            if id.starts_with(L1_ACTOR_PREFIX) && (id == sanitized || id.ends_with(&sanitized)) {
                return Some(entity.key);
            }
        }
    }
    None
}

/// Sanitises a session id into a single canonical-id tail segment: the segment
/// separator `:` cannot appear inside one segment, so a session id like
/// `claude:<uuid>` is carried as the tail `claude-<uuid>`. Pure string transform.
fn sanitize_session_id(actor_id: &str) -> String {
    actor_id.trim().replace(':', "-")
}

/// A splitmix64 finalizer — scatters a seed so the embodied inhabitant is not a
/// plain modular rotation of the revision. Pure: no RNG, no clock.
fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
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

// ---------------------------------------------------------------------------
// Readable naming — projections of the store's canonical ids, display names and
// affordance verbs into the human `text`. GENERIC and data-derived: no per-
// construct or per-verb dictionary. Pure string helpers: they read nothing and
// mutate nothing.
// ---------------------------------------------------------------------------

/// A short, readable label for a node: the identity's final name segment(s) with
/// the `<type>:<level>:<namespace>:` prefix stripped and slug separators
/// humanised (e.g. `space:l2:lumina-prime:orientation-beacon-v0` ->
/// `orientation beacon v0`). Falls back to the humanised type when the node
/// carries no `canonical_id`.
fn readable_label(identity: Option<&str>, semantic_type: &str) -> String {
    match identity {
        Some(id) => humanize_id(id),
        None => humanize_token(semantic_type),
    }
}

/// Strips a canonical id's `<type>:<level>:<namespace>:` prefix, keeping the
/// trailing name segment(s), then humanises the slug. A port id such as
/// `port:l2:lumina-prime:orientation-beacon:emergency-broadcast-v0` keeps its two
/// trailing segments (`orientation beacon emergency broadcast v0`).
fn humanize_id(id: &str) -> String {
    let parts: Vec<&str> = id.split(':').collect();
    let tail = if parts.len() >= 4 && is_level(parts[1]) {
        parts[3..].join(" ")
    } else {
        parts.last().copied().unwrap_or(id).to_owned()
    };
    humanize_token(&tail)
}

/// A level tag in a canonical id: `l1` | `l2` | `l3` | `l4` (letter `l` + digit).
fn is_level(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0] == b'l' && b[1].is_ascii_digit()
}

/// Turns a slug into readable words: `-`, `_`, `:` become spaces.
fn humanize_token(s: &str) -> String {
    s.replace(['-', '_', ':'], " ")
}

/// The world's readable place name, derived from the `<namespace>` segment of the
/// origin's `canonical_id` (`...:l2:mind-universe:...` -> `Mind Universe`), title-
/// cased generically. Falls back to any other identified node's namespace, and to
/// `None` (the place is then omitted) when no node carries an identity — never a
/// hardcoded literal, never a universe hex id.
fn place_name(objects: &[ObservedObject]) -> Option<String> {
    let ns = |o: &ObservedObject| -> Option<String> {
        let id = o.identity.as_deref()?;
        let parts: Vec<&str> = id.split(':').collect();
        (parts.len() >= 3 && is_level(parts[1]) && !parts[2].is_empty())
            .then(|| title_case(parts[2]))
    };
    objects
        .iter()
        .find(|o| o.origin)
        .and_then(ns)
        .or_else(|| objects.iter().find_map(ns))
}

/// Title-cases a slug namespace: `lumina-prime` -> `Lumina Prime`.
fn title_case(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// --- Text grouping: collapse a construct's faces / a moment stream to ONE line --
// A construct's anatomy faces (space / code / objective / metric / health…) share
// a canonical-id tail, and a change-ground moment stream shares a class once its
// per-change hash is dropped. `group_key` reduces both to a stable key so they
// collapse to one text line; `group_display_name` derives that line's NAME from
// the node's OWN store data — never a per-construct dictionary. These are GENERIC
// transforms (prefix strip, version strip, hash drop, humanise), not hardcoded
// names.

/// The key that folds anatomy faces and a moment stream into ONE text line: the
/// canonical_id's `type:l2:<ns>:` prefix stripped and `-vN` version dropped, with a
/// `change-ground` moment reduced to its class (the per-change hash dropped). A
/// node with no identity keys on its type, kept DISTINCT so two typed nodes never
/// merge into one generic line.
fn group_key(identity: Option<&str>, semantic_type: &str) -> String {
    let Some(id) = identity else {
        return format!("type:{semantic_type}");
    };
    let parts: Vec<&str> = id.split(':').collect();
    let tail = if parts.len() >= 4 && is_level(parts[1]) {
        parts[3..].join(":")
    } else {
        id.to_owned()
    };
    // A change-ground moment stream collapses by class (drop the per-change hash).
    // Order matters: `-rejected` is a superset of the plain form, so it wins first.
    if tail.contains("change-ground-rejected") {
        return "change-ground-rejected".to_owned();
    }
    if tail.contains("change-ground") {
        return "change-ground".to_owned();
    }
    strip_version(&tail)
}

/// The readable NAME of a grouped text line, derived from the node's own store
/// data — GENERIC, no dictionary. A collapsed moment class names itself from its
/// (hash-free) key, so the whole stream reads as one hash-free line rather than
/// the first moment's hashed name. Otherwise the node's own `content.name` wins;
/// failing that, its humanised `canonical_id`; failing that, its humanised type.
fn group_display_name(
    gkey: &str,
    content_name: Option<&str>,
    identity: Option<&str>,
    semantic_type: &str,
) -> String {
    if gkey.starts_with("change-ground") {
        return capitalize_first(&humanize_token(gkey));
    }
    if let Some(name) = content_name {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_owned();
        }
    }
    match identity {
        Some(id) => capitalize_first(&humanize_id(id)),
        None => capitalize_first(&humanize_token(semantic_type)),
    }
}

/// Drops a trailing `-vN` (any run of digits) — the hook's `/-v\d+$/`.
fn strip_version(tail: &str) -> String {
    if let Some(idx) = tail.rfind("-v") {
        let suffix = &tail[idx + 2..];
        if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
            return tail[..idx].to_owned();
        }
    }
    tail.to_owned()
}

/// Upper-cases the first character of `s`, leaving the rest untouched.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// --- Affordance verb -> a readable action phrase ----------------------------
// GENERIC humanisation only: the raw verb with its slug separators made spaces.
// No per-verb translation table — the phrase is the store's own verb, read back.

/// An affordance verb, humanised for the human text: `_` and `-` become spaces
/// (`run_until_quiescent` -> `run until quiescent`). The raw verb made readable —
/// no per-verb dictionary. Empty stays empty (the caller skips empties).
fn humanize_verb(verb: &str) -> String {
    verb.trim().replace(['_', '-'], " ").trim().to_owned()
}

// ---------------------------------------------------------------------------
// Affordance derivation — the REAL affordances of a perceived object, read
// READ-ONLY from its stored content and classified visible / active / inert.
//
// The authored encoding it reads is the same one `construct_map.rs::compute_offer`
// resolves and the ontology fixtures carry (a construct is a `space` node with
// member entities; each entity's content wrapper is `{canonical_id, node_type,
// subtype, content}`):
//   - `programming_contract.observer_capabilities` and a `*_port`'s
//     `observe_capabilities`      -> read verbs      (active; precondition=none)
//   - `programming_contract.author_capabilities`, gated by
//     `required_mutate_capability` -> author verbs   (inert; a capability need)
//   - a `code` member's `entrypoints` -> mechanism verbs (inert; physics-gated)
//   - `emits_effect_intent` / `effect_gate` / an `effect` naming an EffectIntent
//                                  -> effect verbs    (inert; physics-gated)
//   - a `physicalization_binding`  -> a visible affordance (no cluster)
//
// Honest epistemics (CLAUDE.md): the fired-state of a physics trigger gate is
// NOT in the committed snapshot (no live atom energy), and a read-only observer
// does not perceive the acting actor's authority-set, so a gated affordance is
// reported `invocable: "unknown"` — never fabricated `active`. A read-only
// surface is genuinely invocable now (reading needs no gate) -> `invocable:true`.
// ---------------------------------------------------------------------------
fn derive_affordances(target: &str, target_label: &str, wrapper: &Value) -> Vec<Affordance> {
    let subtype = wrapper.get("subtype").and_then(Value::as_str).unwrap_or("");
    // The real store wraps authored content under `.content`; a flat value is
    // its own content (test/fixture flexibility).
    let inner = wrapper.get("content").unwrap_or(wrapper);

    let mk = |verb: &str,
              genre: AffordanceGenre,
              invocable: Invocable,
              precondition: Option<String>,
              effect: Option<String>,
              justification: Option<String>,
              gate: &'static str|
     -> Affordance {
        Affordance {
            key: format!("{target}#{verb}"),
            target: target.to_owned(),
            target_label: target_label.to_owned(),
            verb: verb.to_owned(),
            genre,
            invocable,
            precondition,
            effect,
            justification,
            gate,
        }
    };

    let mut out: Vec<Affordance> = Vec::new();
    let contract = inner.get("programming_contract");
    // The contract rule is the shared justification for the contract's verbs.
    let rule = contract
        .and_then(|c| c.get("rule"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    // --- read / observe verbs: active, precondition = none (read-only) ---------
    let mut read_verbs: BTreeSet<String> = BTreeSet::new();
    if let Some(c) = contract {
        read_verbs.extend(str_array(c, "observer_capabilities"));
    }
    if subtype.ends_with("_port") {
        read_verbs.extend(str_array(inner, "observe_capabilities"));
    }
    for v in &read_verbs {
        out.push(mk(
            v,
            AffordanceGenre::Active,
            Invocable::Yes,
            None,
            None,
            rule.clone()
                .or_else(|| Some("read-only observation surface: reading is side-effect-free".into())),
            "read_only",
        ));
    }

    // --- author / mutate verbs: inert (a capability need); gate not measurable --
    let gate_cap = inner
        .get("required_mutate_capability")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| first_authority(inner));
    if let Some(c) = contract {
        for v in str_array(c, "author_capabilities") {
            let pre = format!(
                "actor holds {}",
                gate_cap
                    .clone()
                    .unwrap_or_else(|| "an authoring capability (see the construct's capability_port)".into())
            );
            out.push(mk(
                &v,
                AffordanceGenre::Inert,
                Invocable::Unknown,
                Some(pre),
                None,
                rule.clone(),
                "capability",
            ));
        }
    }

    // --- mechanism verbs (code entrypoints): inert; physics-gated, unreadable ---
    if subtype == "code" {
        let effect = code_effect_summary(inner);
        for v in str_array(inner, "entrypoints") {
            out.push(mk(
                &v,
                AffordanceGenre::Inert,
                Invocable::Unknown,
                Some(
                    "the construct's physics trigger gate (Sensor -> DepositBond -> Threshold); its \
                     fired-state is not measurable from this read path"
                        .into(),
                ),
                effect.clone(),
                Some("declared code entrypoint (the construct's callable mechanism surface)".into()),
                "physics_trigger",
            ));
        }
    }

    // --- effect affordances (a fired trigger emits a notify EffectIntent) -------
    let mut effects: Vec<(String, String)> = Vec::new();
    collect_effects(inner, &mut effects);
    effects.sort();
    effects.dedup();
    for (name, detail) in effects {
        out.push(mk(
            &name,
            AffordanceGenre::Inert,
            Invocable::Unknown,
            Some(
                "the construct's trigger gate must fire; its fired-state is not measurable from this \
                 read path"
                    .into(),
            ),
            Some(format!(
                "{detail}; a real effect needs an authorized transport + EffectReceipt (never proven by a firing)"
            )),
            None,
            "physics_trigger",
        ));
    }

    // --- visible affordance: a physicalization_binding is perceived-only --------
    if subtype == "physicalization_binding" {
        let note = inner.get("note").and_then(Value::as_str).map(str::to_owned);
        out.push(mk(
            "appearance",
            AffordanceGenre::Visible,
            Invocable::No,
            None,
            None,
            note,
            "display",
        ));
    }

    // Dedup by (verb, gate), preserving first occurrence.
    let mut seen: BTreeSet<(String, &'static str)> = BTreeSet::new();
    out.retain(|a| seen.insert((a.verb.clone(), a.gate)));
    out
}

/// The string members of a JSON array field, or empty.
fn str_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// The first `authority:*` string anywhere in a JSON tree (the capability an
/// author verb is gated behind, when the object names it inline).
fn first_authority(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if s.starts_with("authority:") => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(first_authority),
        Value::Object(o) => o.values().find_map(first_authority),
        _ => None,
    }
}

/// A short effect summary from a `code` member's `effect_intent_schema`.
fn code_effect_summary(inner: &Value) -> Option<String> {
    inner
        .get("effect_intent_schema")
        .and_then(|s| s.get("subtype"))
        .and_then(Value::as_str)
        .map(|sub| format!("emits a '{sub}' EffectIntent"))
}

/// Harvest declared world-facing effects (name, detail) from a member's content:
/// an `emits_effect_intent.effect`, an `effect_gate.on_effector_activation`, or a
/// plain `effect` string that names an EffectIntent / notify. Mirrors
/// `construct_map.rs::collect_effects`.
fn collect_effects(v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(o) => {
            if let Some(e) = o.get("emits_effect_intent").and_then(Value::as_object) {
                let name = e.get("effect").and_then(Value::as_str).unwrap_or("effect").to_owned();
                let cap = e.get("required_capability").and_then(Value::as_str).unwrap_or("-");
                out.push((name, format!("emits_effect_intent; required_capability={cap}")));
            }
            if let Some(g) = o.get("effect_gate").and_then(Value::as_object) {
                if let Some(act) = g.get("on_effector_activation").and_then(Value::as_str) {
                    out.push((act.to_owned(), "effect_gate.on_effector_activation".into()));
                }
            }
            if let Some(eff) = o.get("effect").and_then(Value::as_str) {
                if eff.contains("EffectIntent") || eff.contains("notify") {
                    out.push(("notify".into(), "behavior.effect (EffectIntent)".into()));
                }
            }
            for val in o.values() {
                collect_effects(val, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|val| collect_effects(val, out)),
        _ => {}
    }
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
        // The human text is the generic, filtered shape: a prose situation line,
        // no physics-sphere wording. This world carries NO canonical ids, so no
        // place can be derived — it is omitted, never a hardcoded "Lumina Prime" —
        // and the origin names itself generically from its type ("Root").
        assert!(obs.text.starts_with("Tu es près de Root"), "situated prose: {}", obs.text);
        assert!(!obs.text.contains(", à "), "no place clause when none derivable: {}", obs.text);
        assert!(!obs.text.contains("Lumina Prime"), "no hardcoded place: {}", obs.text);
        assert!(!obs.text.to_lowercase().contains("sphere"), "no sphere wording: {}", obs.text);
        assert!(!obs.text.to_lowercase().contains("uncertainty"), "no uncertainty line: {}", obs.text);
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
        // The external vantage reads as observing, in French.
        assert!(obs.text.starts_with("Tu observes"), "external prose: {}", obs.text);
        assert!(!obs.text.to_lowercase().contains("sphere"));
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
        // No world -> no world affordances, and never the adapter's tool names.
        assert!(obs.affordances.is_empty());
    }

    // -- No-actor perception: embody a random L1 inhabitant --------------------

    #[test]
    fn no_actor_named_embodies_a_random_l1_inhabitant() {
        // A world with an L1 actor (type `actor`, canonical_id `actor:l1:...`) and
        // a thing it can see. No `actor_id` is given, so `sense` embodies the
        // inhabitant and perceives from WITHIN it — a situated POV.
        let nodes = vec![
            n("actor:l1:lumina-prime:dreamer-01", "actor", "actor", serde_json::json!({ "name": "Dreamer 01" })),
            n("thing:l1:lumina-prime:pillow", "thing", "thing", serde_json::json!({ "name": "a pillow" })),
        ];
        let (snapshot, wrappers, root_hex) = world_from_nodes(&nodes);
        let reader = |c: &ContentRef| wrappers.get(c.pointer.offset as usize).cloned();
        let obs = observe(&snapshot, &inv(), &params(None), None, &reader);

        // The caller named nobody, so the actor was resolved as a random L1 one —
        // and the resolution is reported honestly, never dressed up as a choice.
        assert_eq!(obs.situation["actor_resolution"], "random_l1");
        assert_eq!(obs.situation["embodied_actor"].as_str(), Some(root_hex.as_str()));
        // Perceived from WITHIN the inhabitant: a situated, non-generated eye.
        let pov = obs.pov.as_ref().unwrap();
        assert_eq!(pov.eye_source, "situated");
        assert!(!pov.generated);
    }

    #[test]
    fn no_actor_and_no_l1_inhabitant_observes_from_outside() {
        // The plain linked world has no actor-typed node -> nobody to embody. The
        // fallback is an honest external-observer vantage, never a substitute.
        let obs = observe(&linked_world(), &inv(), &params(None), None, &no_content);
        assert_eq!(obs.situation["actor_resolution"], "no_l1_actor_external");
        assert!(obs.situation["embodied_actor"].is_null());
        assert_eq!(obs.pov.as_ref().unwrap().eye_source, "external_observer");
        assert!(obs.pov.as_ref().unwrap().generated);
    }

    #[test]
    fn a_non_l1_actor_is_not_embodied() {
        // An L2 actor is not an L1 inhabitant: the level lives in the canonical_id,
        // so `actor:l2:...` must NOT be picked when no actor is named.
        let nodes = vec![
            n("actor:l2:mind-universe:institution", "actor", "actor", serde_json::json!({ "name": "an L2 institution" })),
            n("thing:l2:mind-universe:desk", "thing", "thing", serde_json::json!({})),
        ];
        let (snapshot, wrappers, _root) = world_from_nodes(&nodes);
        let reader = |c: &ContentRef| wrappers.get(c.pointer.offset as usize).cloned();
        let obs = observe(&snapshot, &inv(), &params(None), None, &reader);
        assert_eq!(obs.situation["actor_resolution"], "no_l1_actor_external");
        assert!(obs.situation["embodied_actor"].is_null());
    }

    #[test]
    fn a_session_canonical_id_actor_resolves_to_a_situated_actor() {
        // The caller names its SESSION id (`claude:<uuid>`), which matches no type
        // symbol and no 32-hex key — but DOES match an L1 actor node's stored
        // `canonical_id` (session `:` carried as the tail `-`). The actor is
        // therefore embodied at its OWN placement: situated, non-generated, with no
        // hardcoded vantage. The resolution is reported honestly as `named_canonical`.
        let nodes = vec![
            n("actor:l1:lumina-prime:claude-1234-5678", "actor", "actor", serde_json::json!({ "name": "Claude" })),
            n("thing:l1:lumina-prime:pillow", "thing", "thing", serde_json::json!({ "name": "a pillow" })),
        ];
        let (snapshot, wrappers, root_hex) = world_from_nodes(&nodes);
        let reader = |c: &ContentRef| wrappers.get(c.pointer.offset as usize).cloned();
        let obs = observe(&snapshot, &inv(), &params(Some("claude:1234-5678")), None, &reader);

        assert_eq!(obs.situation["actor_resolution"], "named_canonical");
        assert_eq!(obs.situation["embodied_actor"].as_str(), Some(root_hex.as_str()));
        let pov = obs.pov.as_ref().unwrap();
        assert_eq!(pov.eye_source, "situated", "embodied at its own placement: {:?}", pov);
        assert!(!pov.generated, "a resolved actor is not a generated observer");
        // The session id echoes back in `actor`; the key looked through is the actor's.
        assert_eq!(obs.situation["actor"], "claude:1234-5678");
    }

    #[test]
    fn a_session_id_matching_no_canonical_id_falls_back_to_external() {
        // A session id that matches no actor node's canonical_id resolves to an
        // honest external observer — never a fabricated embodiment.
        let nodes = vec![
            n("actor:l1:lumina-prime:someone-else", "actor", "actor", serde_json::json!({ "name": "Someone" })),
        ];
        let (snapshot, wrappers, _root) = world_from_nodes(&nodes);
        let reader = |c: &ContentRef| wrappers.get(c.pointer.offset as usize).cloned();
        let obs = observe(&snapshot, &inv(), &params(Some("claude:no-such-session")), None, &reader);
        assert_eq!(obs.situation["actor_resolution"], "named");
        assert!(obs.situation["embodied_actor"].is_null());
        assert_eq!(obs.pov.as_ref().unwrap().eye_source, "external_observer");
    }

    // -- Affordance derivation -------------------------------------------------

    fn node_content_ref(idx: usize) -> ContentRef {
        ContentRef {
            pointer: universe_core::ContentPtr { segment: 0, offset: idx as u64, length: 0 },
            sha256: "0".repeat(64),
        }
    }

    /// Build a snapshot + content reader from authored nodes, mirroring the real
    /// store's wrapper shape `{canonical_id, node_type, subtype, content}` (as
    /// `world.rs::inject_fixture` writes it). Node 0 is the root; every later
    /// node is linked to it by PART_OF so the bounded sphere reaches it. Returns
    /// (snapshot, wrappers-by-entity-index, root-key-hex).
    fn world_from_nodes(
        nodes: &[(String, String, String, serde_json::Value)],
    ) -> (UniverseSnapshot, Vec<serde_json::Value>, String) {
        let mut symbols: Vec<String> = Vec::new();
        for (_, nt, _, _) in nodes {
            if !symbols.iter().any(|s| s == nt) {
                symbols.push(nt.clone());
            }
        }
        let part_of = symbols.len() as u32;
        symbols.push("PART_OF".into());

        let mut entities = Vec::new();
        let mut wrappers = Vec::new();
        for (i, (id, nt, st, content)) in nodes.iter().enumerate() {
            let symbol = symbols.iter().position(|s| s == nt).unwrap() as u32;
            entities.push(EntityRecord {
                key: EntityKey((i as u128) + 1),
                generation: 0,
                symbol,
                content: Some(node_content_ref(i)),
            });
            wrappers.push(serde_json::json!({
                "canonical_id": id, "node_type": nt, "subtype": st, "content": content,
            }));
        }
        let mut relations = Vec::new();
        for i in 1..nodes.len() {
            relations.push(RelationRecord {
                key: RelationKey(i as u128),
                generation: 0,
                source: EntityKey((i as u128) + 1),
                target: EntityKey(1),
                predicate: part_of,
                content: None,
            });
        }
        let snapshot = UniverseSnapshot {
            format_version: 0,
            universe: UniverseId(0xC0DE),
            revision: Revision(9),
            tick: Tick(1),
            symbols,
            entities,
            relations,
            event_keys: Default::default(),
        };
        (snapshot, wrappers, format!("{:032x}", 1u128))
    }

    fn n(id: &str, nt: &str, st: &str, content: serde_json::Value) -> (String, String, String, serde_json::Value) {
        (id.to_owned(), nt.to_owned(), st.to_owned(), content)
    }

    /// The REAL Sky-toolkit fixture nodes, read from disk, wrapped as the store
    /// would wrap them. Proving classification against this is proving it against
    /// authored, on-disk graph content — not an invented shape.
    fn sky_nodes() -> Vec<(String, String, String, serde_json::Value)> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/ontology/sky-toolkit-v0.json");
        let doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read sky fixture")).expect("parse sky");
        let pick = |v: &serde_json::Value| {
            n(
                v.get("id").and_then(|x| x.as_str()).unwrap(),
                v.get("node_type").and_then(|x| x.as_str()).unwrap(),
                v.get("subtype").and_then(|x| x.as_str()).unwrap(),
                v.get("content").cloned().unwrap_or(serde_json::Value::Null),
            )
        };
        let mut out = vec![pick(&doc)];
        for m in doc.get("members").and_then(|x| x.as_array()).unwrap() {
            let st = m.get("subtype").and_then(|x| x.as_str()).unwrap_or("");
            if matches!(st, "capability_port" | "code" | "physicalization_binding") {
                out.push(pick(m));
            }
        }
        out
    }

    #[test]
    fn real_authored_affordances_are_classified_from_the_graph() {
        let nodes = sky_nodes();
        let (snapshot, wrappers, root) = world_from_nodes(&nodes);
        let reader = |c: &ContentRef| wrappers.get(c.pointer.offset as usize).cloned();
        let mut p = params(Some(&root));
        p.radius_m = Some(1e9); // one wide sphere: every member is materialised
        let obs = observe(&snapshot, &inv(), &p, None, &reader);

        // The root's programming_contract.observer_capabilities -> ACTIVE read
        // verbs, invocable now (a read-only surface needs no gate).
        let observe_aff = obs
            .affordances
            .iter()
            .find(|a| a.verb == "observe")
            .expect("an observe read verb is surfaced");
        assert_eq!(observe_aff.genre, AffordanceGenre::Active);
        assert_eq!(observe_aff.invocable, Invocable::Yes);
        assert_eq!(observe_aff.gate, "read_only");
        assert!(observe_aff.precondition.is_none(), "a read verb has no precondition cluster");

        // The root's author_capabilities -> INERT (a capability need); the actor's
        // authority-set is not perceivable, so invocable is honestly UNKNOWN.
        let add_star = obs
            .affordances
            .iter()
            .find(|a| a.verb == "add_star")
            .expect("an author verb is surfaced");
        assert_eq!(add_star.genre, AffordanceGenre::Inert);
        assert_eq!(add_star.invocable, Invocable::Unknown);
        assert_eq!(add_star.gate, "capability");
        assert!(add_star.precondition.as_deref().unwrap().contains("actor holds"));

        // A capability_port member's observe_capabilities are also active reads.
        assert!(obs.affordances.iter().any(|a| a.verb == "read_constellation"
            && a.genre == AffordanceGenre::Active
            && a.invocable == Invocable::Yes));

        // A code member's entrypoints -> INERT mechanism verbs; the physics gate's
        // fired-state is not measurable -> invocable UNKNOWN (never fabricated).
        let run = obs
            .affordances
            .iter()
            .find(|a| a.verb == "run_until_quiescent")
            .expect("a code entrypoint is surfaced");
        assert_eq!(run.genre, AffordanceGenre::Inert);
        assert_eq!(run.invocable, Invocable::Unknown);
        assert_eq!(run.gate, "physics_trigger");

        // A physicalization_binding -> a VISIBLE affordance with NO cluster.
        let visible = obs
            .affordances
            .iter()
            .find(|a| a.genre == AffordanceGenre::Visible)
            .expect("a physicalization_binding yields a visible affordance");
        assert_eq!(visible.invocable, Invocable::No);
        assert!(visible.precondition.is_none() && visible.effect.is_none(), "visible = no cluster");
        assert_eq!(visible.gate, "display");

        // Every affordance points back at a real perceived object.
        let object_keys: BTreeSet<&str> = obs.objects.iter().map(|o| o.key.as_str()).collect();
        assert!(obs.affordances.iter().all(|a| object_keys.contains(a.target.as_str())));

        // The hardcoded adapter tool names are GONE.
        assert!(obs.affordances.iter().all(|a| a.verb != "sense" && a.verb != "act"));

        // The `invocable` field serialises to exactly true | false | "unknown".
        let j = serde_json::to_value(observe_aff).unwrap();
        assert_eq!(j["invocable"], serde_json::json!(true));
        let ju = serde_json::to_value(add_star).unwrap();
        assert_eq!(ju["invocable"], serde_json::json!("unknown"));

        // The human text is the generic, data-derived shape. Print it so
        // `cargo test -- --nocapture` surfaces the exact render for review.
        eprintln!("\n--- rendered `text` (sky toolkit) ---\n{}\n--- end ---\n", obs.text);

        // Place is DERIVED from the origin's namespace (`...:l2:mind-universe:...`),
        // title-cased — never a hardcoded literal, physics-sphere or hex dump.
        assert!(obs.text.contains("Mind Universe"), "derives the place: {}", obs.text);
        assert!(!obs.text.to_lowercase().contains("sphere"), "no sphere wording: {}", obs.text);
        // The node NAME is the store's OWN display name (`content.name`), not a
        // hardcoded dictionary entry — and the construct's anatomy faces (space +
        // code + physicalization_binding all share the `sky-toolkit` tail) COLLAPSE
        // to a single line, not one duplicated name per face.
        let name = "Sky toolkit v0 (programmable luminous constellations)";
        assert!(obs.text.contains(name), "content.name drives the node name: {}", obs.text);
        assert_eq!(
            obs.text.matches(&format!("\n{name}")).count(),
            1,
            "the faces collapse to ONE node line (dedup): {}",
            obs.text
        );
        // The port is a separate construct (a different tail), named from its own
        // stored display name — no dictionary.
        assert!(
            obs.text.contains("Sky authoring port (constellation mutation)"),
            "the port names itself from its content.name: {}",
            obs.text
        );
        // Verbs are the store's OWN verbs, humanised (`_`/`-` -> spaces), grouped as
        // `  · ` bullets — the read verb, a multi-word author verb, and a multi-word
        // entrypoint that used to leak snake_case.
        assert!(obs.text.contains("\n  · observe"), "grouped read verb: {}", obs.text);
        assert!(obs.text.contains("\n  · add star"), "grouped multi-word author verb: {}", obs.text);
        assert!(obs.text.contains("run until quiescent"), "entrypoint humanised: {}", obs.text);
        // No snake_case verb leaks through (every action was humanised).
        assert!(!obs.text.contains('_'), "no untranslated snake_case verb: {}", obs.text);
        assert!(!obs.text.contains("run_until_quiescent"), "no raw snake_case verb: {}", obs.text);
    }

    #[test]
    fn a_notify_effect_is_inert_with_an_unknown_physics_gate() {
        // A house-alarm-style construct root whose pattern declares a notify
        // EffectIntent. The effect affordance is present but its trigger gate's
        // fired-state is not measurable -> inert, invocable UNKNOWN.
        let root = n(
            "space:l2:lumina-prime:house-alarm-v0",
            "space",
            "house_alarm",
            serde_json::json!({
                "name": "Lumina Prime House Alarm v0",
                "construct_pattern": {
                    "effect": "The fire emits a 'notify' EffectIntent to the authorized recipient."
                }
            }),
        );
        let (snapshot, wrappers, root_hex) = world_from_nodes(&[root]);
        let reader = |c: &ContentRef| wrappers.get(c.pointer.offset as usize).cloned();
        let obs = observe(&snapshot, &inv(), &params(Some(&root_hex)), None, &reader);

        let notify = obs
            .affordances
            .iter()
            .find(|a| a.verb == "notify")
            .expect("the notify EffectIntent is surfaced as an affordance");
        assert_eq!(notify.genre, AffordanceGenre::Inert);
        assert_eq!(notify.invocable, Invocable::Unknown);
        assert_eq!(notify.gate, "physics_trigger");
        assert!(notify.effect.as_deref().unwrap().contains("EffectReceipt"));
    }

    #[test]
    fn a_content_less_world_surfaces_no_affordances_and_no_tool_names() {
        // The plain linked world carries no content -> nothing to classify. The
        // adapter never falls back to ["sense","act"].
        let obs = observe(&linked_world(), &inv(), &params(Some("root")), None, &no_content);
        assert!(obs.affordances.is_empty());
        assert!(obs.affordances.iter().all(|a| a.verb != "sense" && a.verb != "act"));
    }
}
