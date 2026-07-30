//! Graph-native spatial layout — the position projection authority.
//!
//! The desktop snapshot bin currently places every entity on one global
//! Fibonacci sphere keyed only by `index` + `count`. That is structurally blind:
//! it ignores Space containment, link semantics, and the universe's metric
//! calibration. This module is the honest replacement kernel — a *deterministic*
//! function from graph structure to 3D position, with four commitments matching
//! the design:
//!
//! 1. **Space first, scale per descent.** The universe is calibrated so a typical
//!    node is ~1m. Layout is computed per Space in a LOCAL frame; descending into
//!    a nested Space multiplies absolute scale by `scale_per_descent` (< 1). A
//!    node's global position is `parent_origin + local * scale(depth)`.
//! 2. **Intra-space forces from link attributes.** Within a Space, sibling
//!    placement is force-directed from three per-link scalars: `similarity`
//!    (embedding cosine, upstream) shortens the rest length; `hierarchy` biases a
//!    vertical axis (parent above child); `polarity` signs the spring
//!    (attract vs. repel).
//! 3. **Hitbox packing.** Every node carries a radius (default 0.5m ⇒ 1m across,
//!    the calibration). After the force pass a bounded relaxation guarantees no
//!    two hitboxes overlap — "leave room for everyone".
//! 4. **Special `inside` links define the tree**, they are not a force. Other
//!    predicates are generic forces.
//!
//! The kernel is dependency-free and epistemically honest: it consumes a
//! projection (`LayoutInput`) whose link scalars were measured elsewhere, and
//! never invents an embedding. A node with no similarity signal simply gets the
//! neutral rest length — an honest "unknown", not a convenient default.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use universe_core::EntityKey;

/// Default hitbox radius: 0.5m ⇒ 1m across, the universe metric calibration.
pub const DEFAULT_RADIUS: f64 = 0.5;
/// Default absolute-scale multiplier applied at each descent into a nested Space.
pub const DEFAULT_SCALE_PER_DESCENT: f64 = 0.1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutParams {
    /// Absolute-scale multiplier per nesting level (0 < s ≤ 1).
    pub scale_per_descent: f64,
    /// Force-directed iteration count (fixed ⇒ deterministic, bounded cost).
    pub iterations: u32,
    /// Bounded overlap-resolution passes after the force phase.
    pub packing_passes: u32,
    /// Pairwise repulsion strength (spreads nodes so there is room).
    pub repulsion: f64,
    /// Link attraction strength.
    pub attraction: f64,
    /// Per-step damping (0 < d ≤ 1) — stabilises the deterministic relaxation.
    pub damping: f64,
    /// Vertical separation a fully-hierarchical (`hierarchy = 1`) link imposes.
    pub hierarchy_lift: f64,
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self {
            scale_per_descent: DEFAULT_SCALE_PER_DESCENT,
            iterations: 240,
            packing_passes: 64,
            repulsion: 1.0,
            attraction: 0.35,
            damping: 0.85,
            hierarchy_lift: 1.5,
        }
    }
}

/// A node to be placed. `radius` is its hitbox half-extent (metres, local frame).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutNode {
    pub key: EntityKey,
    pub radius: f64,
}

impl LayoutNode {
    pub fn new(key: EntityKey) -> Self {
        Self {
            key,
            radius: DEFAULT_RADIUS,
        }
    }
}

/// A directed link between two nodes. `Inside` is the containment predicate that
/// builds the Space tree; every other link is a generic placement force.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutLink {
    pub source: EntityKey,
    pub target: EntityKey,
    /// True when the predicate is the containment relation (`source` is *inside*
    /// `target`). Containment defines the tree, not a force.
    pub inside: bool,
    /// Embedding cosine similarity in [0,1] — higher ⇒ shorter rest length.
    pub similarity: f64,
    /// Signed hierarchy in [-1,1], the canonical `physical_profile` range. Zero
    /// means the link is not hierarchical. Positive ⇒ `source` is the *part*
    /// (placed below the whole `target`); negative ⇒ `source` sits above.
    pub hierarchy: f64,
    /// Polarity as the canonical 2-vector `[forward, backward]`, each in [0,1].
    /// It is a bond STRENGTH (never repulsive): the mean drives attraction. A
    /// zero vector is an honest "no bond", not a default.
    pub polarity: [f64; 2],
}

impl LayoutLink {
    /// A generic non-containment link with a moderate symmetric bond.
    pub fn generic(source: EntityKey, target: EntityKey) -> Self {
        Self {
            source,
            target,
            inside: false,
            similarity: 0.0,
            hierarchy: 0.0,
            polarity: [0.5, 0.5],
        }
    }

    /// A containment link: `source` is inside `target`. Defines the tree, not a
    /// force — its similarity/hierarchy/polarity are ignored by the force pass.
    pub fn containment(source: EntityKey, target: EntityKey) -> Self {
        Self {
            source,
            target,
            inside: true,
            similarity: 0.0,
            hierarchy: 0.0,
            polarity: [0.0, 0.0],
        }
    }

    /// Mean bond strength of the polarity vector, in [0,1].
    pub fn bond(&self) -> f64 {
        0.5 * (self.polarity[0].clamp(0.0, 1.0) + self.polarity[1].clamp(0.0, 1.0))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutInput {
    pub nodes: Vec<LayoutNode>,
    pub links: Vec<LayoutLink>,
    pub params: LayoutParams,
}

/// A placed node: its global position, its depth in the Space tree, and the
/// absolute scale that applied at its level.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlacedNode {
    pub key: EntityKey,
    pub position: [f64; 3],
    pub depth: u32,
    pub scale: f64,
    pub radius: f64,
    /// The containing Space (parent via `inside`), or `None` at the root frame.
    pub space: Option<EntityKey>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub placements: Vec<PlacedNode>,
    /// Global-frame overlaps that survived packing (should be empty on success).
    pub residual_overlaps: usize,
    pub max_depth: u32,
}

impl Layout {
    pub fn position(&self, key: EntityKey) -> Option<[f64; 3]> {
        self.placements
            .iter()
            .find(|placed| placed.key == key)
            .map(|placed| placed.position)
    }
}

#[derive(Debug, PartialEq)]
pub enum LayoutError {
    /// A containment link points at a node absent from the input.
    DanglingContainment(EntityKey),
    /// The `inside` relation contains a cycle — no Space tree exists.
    ContainmentCycle,
    /// A parameter is out of its admissible range.
    InvalidParam(&'static str),
    /// Two nodes share the same key.
    DuplicateNode(EntityKey),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DanglingContainment(key) => {
                write!(f, "containment target {key} is not a node in the input")
            }
            Self::ContainmentCycle => write!(f, "`inside` relation contains a cycle"),
            Self::InvalidParam(name) => write!(f, "layout parameter {name} is out of range"),
            Self::DuplicateNode(key) => write!(f, "node {key} appears more than once"),
        }
    }
}

impl std::error::Error for LayoutError {}

// ---------------------------------------------------------------------------
// Projection: graph snapshot → LayoutInput (pure, store-agnostic).
// ---------------------------------------------------------------------------

/// A relation carried by its predicate NAME (resolved from the symbol table by
/// the caller), so the projection is testable without a store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelationInput {
    pub source: EntityKey,
    pub target: EntityKey,
    pub predicate: String,
}

/// The per-predicate force descriptor read from a `physical_profile` node:
/// `hierarchy` (signed, [-1,1]) and `polarity` (the 2-vector, each [0,1]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileInput {
    pub hierarchy: f64,
    pub polarity: [f64; 2],
}

/// Builds a `LayoutInput` from resolved graph data. Deliberately pure: the store
/// reading (symbol resolution, content reads) happens in the caller.
///
/// - a relation whose predicate is in `containment_predicates` becomes an
///   `inside` link (`source` inside `target`), which defines the Space tree;
/// - any other relation becomes a force link whose `hierarchy`/`polarity` come
///   from the predicate's `physical_profile` when known, else neutral (an honest
///   "unknown", never an invented default);
/// - `similarity(source, target)` supplies the embedding cosine in [0,1]; return
///   0.0 when no embedding is measured.
#[allow(clippy::too_many_arguments)]
pub fn project(
    node_keys: &[EntityKey],
    relations: &[RelationInput],
    profiles: &BTreeMap<String, ProfileInput>,
    containment_predicates: &BTreeSet<String>,
    similarity: &dyn Fn(EntityKey, EntityKey) -> f64,
    default_radius: f64,
    params: LayoutParams,
) -> LayoutInput {
    let present: BTreeSet<EntityKey> = node_keys.iter().copied().collect();
    let nodes = node_keys
        .iter()
        .map(|key| LayoutNode {
            key: *key,
            radius: default_radius,
        })
        .collect();

    let links = relations
        .iter()
        // Drop relations whose endpoints are not both projected nodes — a link
        // to an absent node is not placeable, not a silent zero.
        .filter(|relation| present.contains(&relation.source) && present.contains(&relation.target))
        .map(|relation| {
            if containment_predicates.contains(&relation.predicate) {
                LayoutLink::containment(relation.source, relation.target)
            } else {
                let profile = profiles.get(&relation.predicate).copied();
                LayoutLink {
                    source: relation.source,
                    target: relation.target,
                    inside: false,
                    similarity: similarity(relation.source, relation.target).clamp(0.0, 1.0),
                    hierarchy: profile.map(|p| p.hierarchy).unwrap_or(0.0),
                    polarity: profile.map(|p| p.polarity).unwrap_or([0.5, 0.5]),
                }
            }
        })
        .collect();

    LayoutInput {
        nodes,
        links,
        params,
    }
}

/// Computes a deterministic layout: identical input always yields identical
/// positions (no RNG, no clock, fixed iteration counts, ordered traversal).
pub fn compute(input: &LayoutInput) -> Result<Layout, LayoutError> {
    validate_params(&input.params)?;

    // Node set (ordered, duplicate-checked).
    let mut radii: BTreeMap<EntityKey, f64> = BTreeMap::new();
    for node in &input.nodes {
        if radii.insert(node.key, node.radius.max(1e-6)).is_some() {
            return Err(LayoutError::DuplicateNode(node.key));
        }
    }

    // --- Space tree from `inside` links: child -> parent space. ---
    let mut parent: BTreeMap<EntityKey, EntityKey> = BTreeMap::new();
    for link in input.links.iter().filter(|link| link.inside) {
        if !radii.contains_key(&link.target) {
            return Err(LayoutError::DanglingContainment(link.target));
        }
        if radii.contains_key(&link.source) {
            // Last containment wins deterministically (links are ordered input).
            parent.insert(link.source, link.target);
        }
    }
    let depth = compute_depths(&radii, &parent)?;
    let max_depth = depth.values().copied().max().unwrap_or(0);

    // Children grouped by parent space (None = root frame). Ordered by key.
    let mut groups: BTreeMap<Option<EntityKey>, Vec<EntityKey>> = BTreeMap::new();
    for key in radii.keys() {
        groups
            .entry(parent.get(key).copied())
            .or_default()
            .push(*key);
    }

    // --- Local force-directed placement, one group at a time. ---
    let mut local: BTreeMap<EntityKey, [f64; 3]> = BTreeMap::new();
    for (_, members) in &groups {
        let placed = layout_group(members, &radii, &input.links, &input.params);
        local.extend(placed);
    }

    // --- Compose global positions outward from the root, scaling per descent. ---
    let mut global: BTreeMap<EntityKey, [f64; 3]> = BTreeMap::new();
    // Order by depth so parents resolve before children.
    let mut by_depth: Vec<EntityKey> = radii.keys().copied().collect();
    by_depth.sort_by_key(|key| (depth[key], *key));
    for key in by_depth {
        let d = depth[&key];
        let scale = input.params.scale_per_descent.powi(d as i32);
        let local_pos = local[&key];
        let origin = match parent.get(&key) {
            Some(space) => global[space],
            None => [0.0, 0.0, 0.0],
        };
        global.insert(
            key,
            [
                origin[0] + local_pos[0] * scale,
                origin[1] + local_pos[1] * scale,
                origin[2] + local_pos[2] * scale,
            ],
        );
    }

    let placements: Vec<PlacedNode> = radii
        .keys()
        .map(|key| PlacedNode {
            key: *key,
            position: global[key],
            depth: depth[key],
            scale: input.params.scale_per_descent.powi(depth[key] as i32),
            radius: radii[key] * input.params.scale_per_descent.powi(depth[key] as i32),
            space: parent.get(key).copied(),
        })
        .collect();

    // Peer overlaps only: packing guarantees siblings (same Space) do not
    // collide. A node overlapping its own container is EXPECTED (it is inside
    // it), and cross-Space content spacing is a separate concern (container
    // hitbox sizing) — so neither is counted here as a residual failure.
    let residual_overlaps = count_peer_overlaps(&placements);

    Ok(Layout {
        placements,
        residual_overlaps,
        max_depth,
    })
}

fn validate_params(params: &LayoutParams) -> Result<(), LayoutError> {
    if !(params.scale_per_descent > 0.0 && params.scale_per_descent <= 1.0) {
        return Err(LayoutError::InvalidParam("scale_per_descent"));
    }
    if !(params.damping > 0.0 && params.damping <= 1.0) {
        return Err(LayoutError::InvalidParam("damping"));
    }
    if params.repulsion < 0.0 || params.attraction < 0.0 || params.hierarchy_lift < 0.0 {
        return Err(LayoutError::InvalidParam("negative force"));
    }
    Ok(())
}

/// Depth = distance from a root (a node inside nothing). Detects cycles.
fn compute_depths(
    radii: &BTreeMap<EntityKey, f64>,
    parent: &BTreeMap<EntityKey, EntityKey>,
) -> Result<BTreeMap<EntityKey, u32>, LayoutError> {
    let mut depth: BTreeMap<EntityKey, u32> = BTreeMap::new();
    for key in radii.keys() {
        if depth.contains_key(key) {
            continue;
        }
        // Walk up to a root or an already-known node, guarding against cycles.
        let mut chain: Vec<EntityKey> = Vec::new();
        let mut seen: BTreeSet<EntityKey> = BTreeSet::new();
        let mut cursor = *key;
        let base = loop {
            if let Some(known) = depth.get(&cursor) {
                break *known;
            }
            if !seen.insert(cursor) {
                return Err(LayoutError::ContainmentCycle);
            }
            chain.push(cursor);
            match parent.get(&cursor) {
                Some(next) => cursor = *next,
                None => break 0u32.wrapping_sub(1), // sentinel: root has no parent
            }
        };
        // `base` is either a known depth, or the sentinel meaning "chain ends at root".
        // Assign depths from the far end of the chain inward.
        let mut running = if base == u32::MAX {
            // chain.last() is a root at depth 0.
            let root = *chain.last().unwrap();
            depth.insert(root, 0);
            chain.pop();
            0
        } else {
            base
        };
        for node in chain.iter().rev() {
            running += 1;
            depth.insert(*node, running);
        }
    }
    Ok(depth)
}

/// Force-directed placement of one sibling group in a local frame centred at the
/// origin, followed by hitbox packing. Deterministic: golden-angle seed, fixed
/// iterations, ordered traversal.
fn layout_group(
    members: &[EntityKey],
    radii: &BTreeMap<EntityKey, f64>,
    links: &[LayoutLink],
    params: &LayoutParams,
) -> BTreeMap<EntityKey, [f64; 3]> {
    let n = members.len();
    let mut pos: BTreeMap<EntityKey, [f64; 3]> = BTreeMap::new();
    if n == 0 {
        return pos;
    }
    if n == 1 {
        pos.insert(members[0], [0.0, 0.0, 0.0]);
        return pos;
    }

    let index: BTreeMap<EntityKey, usize> =
        members.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let member_set: BTreeSet<EntityKey> = members.iter().copied().collect();
    let max_radius = members.iter().map(|k| radii[k]).fold(0.0_f64, f64::max);
    // Seed radius sized so N unit-ish nodes have room on a sphere.
    let seed_r = max_radius.max(DEFAULT_RADIUS) * (n as f64).cbrt() * 2.5;

    // Golden-angle spherical seed — deterministic, index-based, but LOCAL to the
    // group (not the whole universe) so it is only a starting point for forces.
    let golden = std::f64::consts::PI * (1.0 + 5.0_f64.sqrt());
    for (i, key) in members.iter().enumerate() {
        let fi = i as f64 + 0.5;
        let phi = (1.0 - 2.0 * fi / n as f64).clamp(-1.0, 1.0).acos();
        let theta = golden * fi;
        pos.insert(
            *key,
            [
                seed_r * phi.sin() * theta.cos(),
                seed_r * phi.sin() * theta.sin(),
                seed_r * phi.cos(),
            ],
        );
    }

    // Only intra-group links participate as forces this iteration.
    let group_links: Vec<&LayoutLink> = links
        .iter()
        .filter(|link| {
            !link.inside
                && member_set.contains(&link.source)
                && member_set.contains(&link.target)
        })
        .collect();

    let keys: Vec<EntityKey> = members.to_vec();
    for _ in 0..params.iterations {
        let mut disp = vec![[0.0_f64; 3]; n];

        // Pairwise repulsion — every pair pushes apart (inverse-square, scaled by
        // desired separation) so the group spreads and leaves room.
        for a in 0..n {
            for b in (a + 1)..n {
                let (pa, pb) = (pos[&keys[a]], pos[&keys[b]]);
                let d = sub(pa, pb);
                let dist = norm(d).max(1e-6);
                let want = radii[&keys[a]] + radii[&keys[b]];
                let mag = params.repulsion * want * want / (dist * dist);
                let dir = scale(d, 1.0 / dist);
                disp[a] = add(disp[a], scale(dir, mag));
                disp[b] = sub(disp[b], scale(dir, mag));
            }
        }

        // Link forces: attraction/repulsion by polarity, rest length by
        // similarity, vertical bias by hierarchy.
        for link in &group_links {
            let a = index[&link.source];
            let b = index[&link.target];
            let (pa, pb) = (pos[&keys[a]], pos[&keys[b]]);
            let d = sub(pb, pa); // from source to target
            let dist = norm(d).max(1e-6);
            let dir = scale(d, 1.0 / dist);

            let base = radii[&keys[a]] + radii[&keys[b]];
            // Higher similarity ⇒ shorter rest length (down to touching).
            let rest = base * (2.0 - link.similarity.clamp(0.0, 1.0));
            // Polarity bond in [0,1] scales an always-attractive spring around
            // the rest length. Repulsion comes only from the all-pairs term.
            let bond = link.bond();
            let mag = params.attraction * bond * (dist - rest);
            // Source pulled toward target when too far, pushed to rest when too close.
            disp[a] = add(disp[a], scale(dir, mag));
            disp[b] = sub(disp[b], scale(dir, mag));

            // Signed hierarchy: positive ⇒ source (the part) sits below the whole
            // target; negative ⇒ source sits above. Zero is not hierarchical.
            let h = link.hierarchy.clamp(-1.0, 1.0);
            if h != 0.0 {
                let target_gap = base + params.hierarchy_lift * h.abs();
                let desired = h.signum() * target_gap; // desired (target.y - source.y)
                let cur_gap = pb[1] - pa[1];
                let corr = params.attraction * (desired - cur_gap);
                disp[a][1] -= corr;
                disp[b][1] += corr;
            }
        }

        // Apply damped, step-limited displacement.
        let step_limit = seed_r * 0.25;
        for (i, key) in keys.iter().enumerate() {
            let mut step = scale(disp[i], params.damping);
            let sn = norm(step);
            if sn > step_limit {
                step = scale(step, step_limit / sn);
            }
            let p = pos[key];
            pos.insert(*key, add(p, step));
        }
    }

    pack(&keys, radii, &mut pos, params.packing_passes);
    center(&keys, &mut pos);
    pos
}

/// Bounded overlap resolution: push overlapping pairs apart by half the overlap
/// each, repeatedly, until clear or the pass budget is spent. Guarantees room.
fn pack(
    keys: &[EntityKey],
    radii: &BTreeMap<EntityKey, f64>,
    pos: &mut BTreeMap<EntityKey, [f64; 3]>,
    passes: u32,
) {
    let n = keys.len();
    for _ in 0..passes {
        let mut moved = false;
        for a in 0..n {
            for b in (a + 1)..n {
                let (pa, pb) = (pos[&keys[a]], pos[&keys[b]]);
                let min_sep = radii[&keys[a]] + radii[&keys[b]];
                let d = sub(pa, pb);
                let dist = norm(d);
                if dist < min_sep {
                    moved = true;
                    // Deterministic separation direction even when coincident.
                    let dir = if dist > 1e-6 {
                        scale(d, 1.0 / dist)
                    } else {
                        let seed = (a as f64 + 1.0) * 0.7548776662; // plastic-number jitter
                        [seed.fract() - 0.5, ((a + b) as f64 * 0.3).fract() - 0.5, 0.5]
                    };
                    let push = (min_sep - dist.max(1e-6)) * 0.5;
                    let pd = scale(normalize(dir), push);
                    pos.insert(keys[a], add(pa, pd));
                    pos.insert(keys[b], sub(pb, pd));
                }
            }
        }
        if !moved {
            break;
        }
    }
}

fn center(keys: &[EntityKey], pos: &mut BTreeMap<EntityKey, [f64; 3]>) {
    if keys.is_empty() {
        return;
    }
    let mut c = [0.0; 3];
    for key in keys {
        c = add(c, pos[key]);
    }
    c = scale(c, 1.0 / keys.len() as f64);
    for key in keys {
        let p = pos[key];
        pos.insert(*key, sub(p, c));
    }
}

/// Counts overlaps between PEERS — nodes sharing the same containing Space. This
/// is exactly the guarantee the per-group packing enforces; container/content
/// and cross-Space pairs are intentionally excluded.
fn count_peer_overlaps(placements: &[PlacedNode]) -> usize {
    let mut overlaps = 0;
    for a in 0..placements.len() {
        for b in (a + 1)..placements.len() {
            if placements[a].space != placements[b].space {
                continue;
            }
            let min_sep = placements[a].radius + placements[b].radius;
            let d = sub(placements[a].position, placements[b].position);
            // Tolerate a hair of floating error.
            if norm(d) + 1e-6 < min_sep {
                overlaps += 1;
            }
        }
    }
    overlaps
}

// --- tiny vec3 helpers (kept local; no external math dependency) ---
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn normalize(a: [f64; 3]) -> [f64; 3] {
    let n = norm(a);
    if n < 1e-9 {
        [0.0, 0.0, 1.0]
    } else {
        scale(a, 1.0 / n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(keys: &[u128]) -> Vec<LayoutNode> {
        keys.iter().map(|k| LayoutNode::new(EntityKey(*k))).collect()
    }

    #[test]
    fn deterministic_same_input_same_output() {
        let input = LayoutInput {
            nodes: nodes(&[1, 2, 3, 4, 5]),
            links: vec![
                LayoutLink::generic(EntityKey(1), EntityKey(2)),
                LayoutLink::generic(EntityKey(3), EntityKey(4)),
            ],
            params: LayoutParams::default(),
        };
        let a = compute(&input).unwrap();
        let b = compute(&input).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn packing_leaves_room_for_everyone() {
        // 40 nodes crammed with no links must end with zero hitbox overlaps.
        let keys: Vec<u128> = (1..=40).collect();
        let input = LayoutInput {
            nodes: nodes(&keys),
            links: vec![],
            params: LayoutParams::default(),
        };
        let layout = compute(&input).unwrap();
        assert_eq!(
            layout.residual_overlaps, 0,
            "no two hitboxes may overlap after packing"
        );
    }

    #[test]
    fn nested_space_loses_scale() {
        // room (1) contains desk (2); desk contains pen (3).
        let input = LayoutInput {
            nodes: nodes(&[1, 2, 3]),
            links: vec![
                LayoutLink::containment(EntityKey(2), EntityKey(1)),
                LayoutLink::containment(EntityKey(3), EntityKey(2)),
            ],
            params: LayoutParams::default(),
        };
        let layout = compute(&input).unwrap();
        let room = layout.placements.iter().find(|p| p.key == EntityKey(1)).unwrap();
        let desk = layout.placements.iter().find(|p| p.key == EntityKey(2)).unwrap();
        let pen = layout.placements.iter().find(|p| p.key == EntityKey(3)).unwrap();
        assert_eq!(room.depth, 0);
        assert_eq!(desk.depth, 1);
        assert_eq!(pen.depth, 2);
        assert!(desk.scale < room.scale, "descent must shrink absolute scale");
        assert!(pen.scale < desk.scale, "each descent shrinks further");
        assert!(pen.radius < desk.radius && desk.radius < room.radius);
    }

    #[test]
    fn similar_linked_nodes_end_closer_than_dissimilar() {
        // 1—2 highly similar (attract close); 1—3 dissimilar. Both attractive.
        let mut close = LayoutLink::generic(EntityKey(1), EntityKey(2));
        close.similarity = 1.0;
        close.polarity = [1.0, 1.0];
        let mut far = LayoutLink::generic(EntityKey(1), EntityKey(3));
        far.similarity = 0.0;
        far.polarity = [1.0, 1.0];
        let input = LayoutInput {
            nodes: nodes(&[1, 2, 3]),
            links: vec![close, far],
            params: LayoutParams::default(),
        };
        let layout = compute(&input).unwrap();
        let p1 = layout.position(EntityKey(1)).unwrap();
        let p2 = layout.position(EntityKey(2)).unwrap();
        let p3 = layout.position(EntityKey(3)).unwrap();
        let d12 = norm(sub(p1, p2));
        let d13 = norm(sub(p1, p3));
        assert!(
            d12 < d13,
            "higher similarity must place nodes closer: d12={d12} d13={d13}"
        );
    }

    #[test]
    fn positive_hierarchy_places_source_below_target() {
        let mut link = LayoutLink::generic(EntityKey(1), EntityKey(2));
        link.hierarchy = 1.0; // source is the part → below the whole
        let input = LayoutInput {
            nodes: nodes(&[1, 2]),
            links: vec![link],
            params: LayoutParams::default(),
        };
        let layout = compute(&input).unwrap();
        let p1 = layout.position(EntityKey(1)).unwrap();
        let p2 = layout.position(EntityKey(2)).unwrap();
        assert!(p1[1] < p2[1], "positive-hierarchy source should sit below target");
    }

    #[test]
    fn negative_hierarchy_places_source_above_target() {
        let mut link = LayoutLink::generic(EntityKey(1), EntityKey(2));
        link.hierarchy = -1.0; // source sits above
        let input = LayoutInput {
            nodes: nodes(&[1, 2]),
            links: vec![link],
            params: LayoutParams::default(),
        };
        let layout = compute(&input).unwrap();
        let p1 = layout.position(EntityKey(1)).unwrap();
        let p2 = layout.position(EntityKey(2)).unwrap();
        assert!(p1[1] > p2[1], "negative-hierarchy source should sit above target");
    }

    #[test]
    fn stronger_polarity_bond_pulls_closer() {
        // Same similarity; a stronger polarity bond should settle closer against
        // the same all-pairs repulsion.
        let mut strong = LayoutLink::generic(EntityKey(1), EntityKey(2));
        strong.polarity = [1.0, 1.0];
        let mut weak = LayoutLink::generic(EntityKey(1), EntityKey(2));
        weak.polarity = [0.1, 0.1];
        let dist = |link: LayoutLink| {
            let l = compute(&LayoutInput {
                nodes: nodes(&[1, 2]),
                links: vec![link],
                params: LayoutParams::default(),
            })
            .unwrap();
            norm(sub(
                l.position(EntityKey(1)).unwrap(),
                l.position(EntityKey(2)).unwrap(),
            ))
        };
        let ds = dist(strong);
        let dw = dist(weak);
        assert!(ds < dw, "stronger bond must settle closer: strong={ds} weak={dw}");
    }

    #[test]
    fn containment_cycle_is_rejected() {
        let input = LayoutInput {
            nodes: nodes(&[1, 2]),
            links: vec![
                LayoutLink::containment(EntityKey(1), EntityKey(2)),
                LayoutLink::containment(EntityKey(2), EntityKey(1)),
            ],
            params: LayoutParams::default(),
        };
        assert_eq!(compute(&input), Err(LayoutError::ContainmentCycle));
    }

    #[test]
    fn dangling_containment_is_rejected() {
        let input = LayoutInput {
            nodes: nodes(&[1]),
            links: vec![LayoutLink::containment(EntityKey(1), EntityKey(999))],
            params: LayoutParams::default(),
        };
        assert_eq!(
            compute(&input),
            Err(LayoutError::DanglingContainment(EntityKey(999)))
        );
    }

    fn containment_set() -> BTreeSet<String> {
        BTreeSet::from(["PART_OF".to_owned()])
    }

    #[test]
    fn projection_maps_part_of_to_containment_tree() {
        // pen PART_OF desk PART_OF room ⇒ a 3-level Space tree.
        let relations = vec![
            RelationInput {
                source: EntityKey(2),
                target: EntityKey(1),
                predicate: "PART_OF".to_owned(),
            },
            RelationInput {
                source: EntityKey(3),
                target: EntityKey(2),
                predicate: "PART_OF".to_owned(),
            },
        ];
        let input = project(
            &[EntityKey(1), EntityKey(2), EntityKey(3)],
            &relations,
            &BTreeMap::new(),
            &containment_set(),
            &|_, _| 0.0,
            DEFAULT_RADIUS,
            LayoutParams::default(),
        );
        assert!(input.links.iter().all(|link| link.inside));
        let layout = compute(&input).unwrap();
        assert_eq!(layout.max_depth, 2);
    }

    #[test]
    fn projection_reads_profile_for_force_links() {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "UNLOCKS".to_owned(),
            ProfileInput {
                hierarchy: 0.15,
                polarity: [0.95, 0.15],
            },
        );
        let relations = vec![RelationInput {
            source: EntityKey(1),
            target: EntityKey(2),
            predicate: "UNLOCKS".to_owned(),
        }];
        let input = project(
            &[EntityKey(1), EntityKey(2)],
            &relations,
            &profiles,
            &containment_set(),
            &|_, _| 0.7,
            DEFAULT_RADIUS,
            LayoutParams::default(),
        );
        assert_eq!(input.links.len(), 1);
        let link = &input.links[0];
        assert!(!link.inside);
        assert_eq!(link.hierarchy, 0.15);
        assert_eq!(link.polarity, [0.95, 0.15]);
        assert_eq!(link.similarity, 0.7);
    }

    #[test]
    fn projection_unknown_predicate_is_neutral_not_defaulted_wrong() {
        let relations = vec![RelationInput {
            source: EntityKey(1),
            target: EntityKey(2),
            predicate: "MYSTERY".to_owned(),
        }];
        let input = project(
            &[EntityKey(1), EntityKey(2)],
            &relations,
            &BTreeMap::new(),
            &containment_set(),
            &|_, _| 0.0,
            DEFAULT_RADIUS,
            LayoutParams::default(),
        );
        let link = &input.links[0];
        assert_eq!(link.hierarchy, 0.0, "unknown predicate is not hierarchical");
        assert_eq!(link.polarity, [0.5, 0.5], "unknown predicate is neutrally bonded");
    }

    #[test]
    fn projection_drops_links_to_absent_nodes() {
        let relations = vec![RelationInput {
            source: EntityKey(1),
            target: EntityKey(999), // not a projected node
            predicate: "UNLOCKS".to_owned(),
        }];
        let input = project(
            &[EntityKey(1)],
            &relations,
            &BTreeMap::new(),
            &containment_set(),
            &|_, _| 0.0,
            DEFAULT_RADIUS,
            LayoutParams::default(),
        );
        assert!(input.links.is_empty(), "a link to an absent node is not placeable");
    }

    #[test]
    fn duplicate_node_is_rejected() {
        let input = LayoutInput {
            nodes: nodes(&[1, 1]),
            links: vec![],
            params: LayoutParams::default(),
        };
        assert_eq!(compute(&input), Err(LayoutError::DuplicateNode(EntityKey(1))));
    }
}
