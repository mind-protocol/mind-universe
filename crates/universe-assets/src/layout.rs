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
    /// Macro layout — radial gap between successive layer shells (metres).
    pub layer_gap: f64,
    /// Macro layout — route ("membrane") attraction: a DELIBERATELY weak spring
    /// pulling linked graphs together angularly without breaking their layer
    /// shell. 0 disables; small (~0.05) nudges. Never strong enough to merge.
    pub route_attraction: f64,
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
            layer_gap: 2.0,
            route_attraction: 0.05,
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
    /// Footprint radius (ground extent). In the city, it shrinks under density.
    pub radius: f64,
    /// Built height. In the city, VOLUME (= weight) is conserved: when the
    /// footprint is squeezed by its locality, the node grows UP instead.
    pub height: f64,
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

// (Removed) `override_with_built` — the authored-Built-position overlay. There
// are no stored coordinates any more: every node's place is derived by the
// solver, so there is nothing authored to overlay.

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

/// Deterministic, stable `string id → EntityKey` (FNV-1a 128). Logical graphs
/// stored as document blobs key their nodes by STRING id (e.g. "l4-node-type-
/// mapping"); the layout needs an `EntityKey`. This mapping is pure and stable
/// across runs and processes, so the same id always yields the same key.
pub fn stable_key(id: &str) -> EntityKey {
    // FNV-1a 128-bit.
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013B;
    let mut hash = OFFSET;
    for byte in id.as_bytes() {
        hash ^= *byte as u128;
        hash = hash.wrapping_mul(PRIME);
    }
    EntityKey(hash)
}

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

/// Computes a deterministic single-graph layout: identical input always yields
/// identical positions (no RNG, no clock, fixed iteration counts, ordered
/// traversal). For a universe of several graphs, see [`compute_clustered`].
pub fn compute(input: &LayoutInput) -> Result<Layout, LayoutError> {
    validate_params(&input.params)?;
    let graph = layout_graph(&input.nodes, &input.links, &input.params)?;
    Ok(Layout {
        placements: graph.placements,
        residual_overlaps: graph.residual,
        max_depth: graph.max_depth,
    })
}

/// The placement of ONE graph, centred on its own origin, plus the evidence the
/// macro layer needs (its bounding radius and residual peer overlaps).
struct GraphOut {
    placements: Vec<PlacedNode>,
    residual: usize,
    max_depth: u32,
    /// Radius of the smallest origin-centred sphere containing every hitbox.
    bounding: f64,
}

/// Lays out a single graph: Space tree (containment) + scale-per-descent +
/// force-directed intra-space + bottom-up hitbox packing. Positions are centred
/// on the graph's own origin so the macro layer can offset the whole graph.
fn layout_graph(
    nodes: &[LayoutNode],
    links: &[LayoutLink],
    params: &LayoutParams,
) -> Result<GraphOut, LayoutError> {
    // Node set (ordered, duplicate-checked).
    let mut radii: BTreeMap<EntityKey, f64> = BTreeMap::new();
    for node in nodes {
        if radii.insert(node.key, node.radius.max(1e-6)).is_some() {
            return Err(LayoutError::DuplicateNode(node.key));
        }
    }

    // --- Space tree from `inside` links: child -> parent space. ---
    let mut parent: BTreeMap<EntityKey, EntityKey> = BTreeMap::new();
    for link in links.iter().filter(|link| link.inside) {
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

    // Children grouped by parent space (None = root frame).
    let mut groups: BTreeMap<Option<EntityKey>, Vec<EntityKey>> = BTreeMap::new();
    for key in radii.keys() {
        groups
            .entry(parent.get(key).copied())
            .or_default()
            .push(*key);
    }

    // --- Bottom-up placement so a container's EFFECTIVE radius reflects its
    // packed contents. Process the deepest groups first; a Space then enters its
    // parent's packing already sized to hold everything inside it, which is what
    // keeps neighbouring Spaces' contents from colliding. ---
    let mut eff: BTreeMap<EntityKey, f64> = radii.clone(); // effective radius per node
    let mut local: BTreeMap<EntityKey, [f64; 3]> = BTreeMap::new();

    let mut ordered: Vec<Option<EntityKey>> = groups.keys().copied().collect();
    // Deepest member depth first. Members of a group all share one depth.
    ordered.sort_by(|a, b| {
        let da = a.map(|p| depth[&p] + 1).unwrap_or(0);
        let db = b.map(|p| depth[&p] + 1).unwrap_or(0);
        db.cmp(&da)
    });
    for parent_space in ordered {
        let members = &groups[&parent_space];
        let (placed, bounding) = layout_group(members, &eff, links, params);
        local.extend(placed);
        // A container reserves, in its own (parent) frame, the room its contents
        // occupy one level down: the local bounding radius times the descent scale.
        if let Some(space) = parent_space {
            let contents = bounding * params.scale_per_descent;
            let base = radii[&space];
            eff.insert(space, base.max(contents));
        }
    }

    // --- Compose global positions outward from the root, scaling per descent. ---
    let mut global: BTreeMap<EntityKey, [f64; 3]> = BTreeMap::new();
    // Order by depth so parents resolve before children.
    let mut by_depth: Vec<EntityKey> = radii.keys().copied().collect();
    by_depth.sort_by_key(|key| (depth[key], *key));
    for key in by_depth {
        let d = depth[&key];
        let scale = params.scale_per_descent.powi(d as i32);
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
            scale: params.scale_per_descent.powi(depth[key] as i32),
            // Effective radius (a container's grows to hold its contents), in the
            // global frame.
            radius: eff[key] * params.scale_per_descent.powi(depth[key] as i32),
            // Non-city layouts don't model built height; a neutral default.
            height: 2.0 * eff[key] * params.scale_per_descent.powi(depth[key] as i32),
            space: parent.get(key).copied(),
        })
        .collect();

    // Peer overlaps only: packing guarantees siblings (same Space) do not
    // collide. A node overlapping its own container is EXPECTED (it is inside
    // it), and cross-Space content spacing is a separate concern (container
    // hitbox sizing) — so neither is counted here as a residual failure.
    let residual_overlaps = count_peer_overlaps(&placements);

    // Graph bounding radius: furthest hitbox surface from the graph origin. The
    // macro layer uses it to space whole graphs without collision.
    let bounding = placements
        .iter()
        .map(|placed| norm(placed.position) + placed.radius)
        .fold(0.0_f64, f64::max);

    Ok(GraphOut {
        placements,
        residual: residual_overlaps,
        max_depth,
        bounding,
    })
}

// ---------------------------------------------------------------------------
// Macro layout: separate the graphs, arrange them by layer, route the membranes.
// ---------------------------------------------------------------------------

/// One graph (cluster) as a placement unit: its id, its Mind Protocol layer, and
/// its independently computed (origin-centred) layout.
struct GraphUnit {
    id: String,
    layer: u8,
    out: GraphOut,
}

/// The macro-level placement of one graph: its layer, anchor, and extent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphAnchor {
    pub cluster: String,
    pub layer: u8,
    pub anchor: [f64; 3],
    pub bounding: f64,
    pub node_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClusteredLayout {
    pub placements: Vec<PlacedNode>,
    /// Peer (same-Space) overlaps summed across every graph — should be 0.
    pub residual_overlaps: usize,
    pub max_depth: u32,
    pub graphs: Vec<GraphAnchor>,
    /// Route pairs between bounded graphs (membrane bridges + direct cross links).
    pub routes: usize,
    /// Nodes of the unbounded membrane graph, placed along the routes.
    pub membrane_nodes: usize,
    /// Overlaps between whole-graph hitboxes of DIFFERENT graphs — should be 0,
    /// since layer shells space graphs apart.
    pub cross_graph_overlaps: usize,
}

impl ClusteredLayout {
    pub fn position(&self, key: EntityKey) -> Option<[f64; 3]> {
        self.placements
            .iter()
            .find(|placed| placed.key == key)
            .map(|placed| placed.position)
    }
}

/// Universe-scale layout: separate the graphs FIRST, arrange the BOUNDED graphs
/// by Mind Protocol layer (higher layer ⇒ more central; L4 at the core), lay
/// each out independently, then route them through the special **membrane**
/// graph — an UNBOUNDED graph (no shell, no hitbox) whose links are the routes
/// connecting the bounded graphs. Routes pull linked graphs together with a
/// DELIBERATELY weak spring that never breaks a shell; membrane nodes sit along
/// the routes, between the graphs they connect.
///
/// `clusters` maps a node to its graph id (absent ⇒ the default graph ""),
/// `layers` maps a graph id to its layer (higher = more central; absent ⇒ 0),
/// `membrane` names the membrane graph's cluster id (None ⇒ no membrane; every
/// cross-graph link is then treated as a direct route).
pub fn compute_clustered(
    input: &LayoutInput,
    clusters: &BTreeMap<EntityKey, String>,
    layers: &BTreeMap<String, u8>,
    membrane: Option<&str>,
) -> Result<ClusteredLayout, LayoutError> {
    validate_params(&input.params)?;

    let cluster_of = |key: &EntityKey| clusters.get(key).cloned().unwrap_or_default();
    let is_membrane = |cid: &str| membrane == Some(cid);

    // Partition nodes: bounded graphs vs. the unbounded membrane graph.
    let mut nodes_by_graph: BTreeMap<String, Vec<LayoutNode>> = BTreeMap::new();
    let mut membrane_nodes: Vec<LayoutNode> = Vec::new();
    for node in &input.nodes {
        let cid = cluster_of(&node.key);
        if is_membrane(&cid) {
            membrane_nodes.push(node.clone());
        } else {
            nodes_by_graph.entry(cid).or_default().push(node.clone());
        }
    }

    // Intra-graph links become forces; membrane and cross links become routes.
    let mut links_by_graph: BTreeMap<String, Vec<LayoutLink>> = BTreeMap::new();
    for link in &input.links {
        let cs = cluster_of(&link.source);
        let ct = cluster_of(&link.target);
        if cs == ct && !is_membrane(&cs) {
            links_by_graph.entry(cs).or_default().push(link.clone());
        }
    }

    // Lay each BOUNDED graph out independently (centred on its own origin).
    let mut graphs: Vec<GraphUnit> = Vec::new();
    for (id, nodes) in &nodes_by_graph {
        let links = links_by_graph.get(id).cloned().unwrap_or_default();
        let out = layout_graph(nodes, &links, &input.params)?;
        let layer = layers.get(id).copied().unwrap_or(0);
        graphs.push(GraphUnit {
            id: id.clone(),
            layer,
            out,
        });
    }

    // Membrane adjacency: for each membrane node, the DISTINCT bounded graphs its
    // links touch. Also collect direct cross-graph route pairs (no membrane).
    let mut membrane_adjacency: BTreeMap<EntityKey, BTreeSet<String>> = BTreeMap::new();
    let mut route_pairs: Vec<(String, String)> = Vec::new();
    for link in &input.links {
        let cs = cluster_of(&link.source);
        let ct = cluster_of(&link.target);
        let s_mem = is_membrane(&cs);
        let t_mem = is_membrane(&ct);
        match (s_mem, t_mem) {
            (true, false) => {
                membrane_adjacency.entry(link.source).or_default().insert(ct);
            }
            (false, true) => {
                membrane_adjacency.entry(link.target).or_default().insert(cs);
            }
            (false, false) if cs != ct => route_pairs.push((cs, ct)),
            _ => {}
        }
    }
    // Each membrane node bridges the bounded graphs it touches → pairwise routes.
    for set in membrane_adjacency.values() {
        let touched: Vec<&String> = set.iter().collect();
        for i in 0..touched.len() {
            for j in (i + 1)..touched.len() {
                route_pairs.push((touched[i].clone(), touched[j].clone()));
            }
        }
    }

    // Shell the bounded graphs, then let weak route springs nudge them.
    let anchors = place_layer_shells(&graphs, input.params.layer_gap);
    let anchors = nudge_routes(anchors, &graphs, &route_pairs, input.params.route_attraction);
    let anchor_of: BTreeMap<String, [f64; 3]> = graphs
        .iter()
        .zip(&anchors)
        .map(|(g, anchor)| (g.id.clone(), *anchor))
        .collect();

    // Compose bounded placements: offset each graph by its anchor.
    let mut placements: Vec<PlacedNode> = Vec::new();
    let mut residual = 0usize;
    let mut max_depth = 0u32;
    let mut graph_evidence: Vec<GraphAnchor> = Vec::new();
    for (g, anchor) in graphs.iter().zip(&anchors) {
        residual += g.out.residual;
        max_depth = max_depth.max(g.out.max_depth);
        for placed in &g.out.placements {
            let mut moved = *placed;
            moved.position = add(placed.position, *anchor);
            placements.push(moved);
        }
        graph_evidence.push(GraphAnchor {
            cluster: g.id.clone(),
            layer: g.layer,
            anchor: *anchor,
            bounding: g.out.bounding,
            node_count: g.out.placements.len(),
        });
    }

    // Place membrane nodes along their routes: at the centroid of the anchors of
    // the bounded graphs they connect. Unbounded — no shell, no hitbox reserve.
    for node in &membrane_nodes {
        let position = membrane_adjacency
            .get(&node.key)
            .and_then(|set| {
                let points: Vec<[f64; 3]> =
                    set.iter().filter_map(|c| anchor_of.get(c).copied()).collect();
                if points.is_empty() {
                    None
                } else {
                    let sum = points.iter().fold([0.0; 3], |acc, p| add(acc, *p));
                    Some(scale(sum, 1.0 / points.len() as f64))
                }
            })
            .unwrap_or([0.0, 0.0, 0.0]);
        placements.push(PlacedNode {
            key: node.key,
            position,
            depth: 0,
            scale: 1.0,
            radius: node.radius,
            height: 2.0 * node.radius,
            space: None,
        });
    }

    let cross_graph_overlaps = count_cross_graph_overlaps(&graphs, &anchors);

    Ok(ClusteredLayout {
        placements,
        residual_overlaps: residual,
        max_depth,
        graphs: graph_evidence,
        routes: route_pairs.len(),
        membrane_nodes: membrane_nodes.len(),
        cross_graph_overlaps,
    })
}

/// Places each graph's anchor on a shell chosen by its layer: the highest layer
/// (e.g. L4) sits at the core, lower layers on successively larger rings. A ring
/// is sized to clear the inner shells AND to give its graphs enough arc that
/// their hitboxes do not overlap. Deterministic (graphs ordered by id).
fn place_layer_shells(graphs: &[GraphUnit], gap: f64) -> Vec<[f64; 3]> {
    let n = graphs.len();
    let mut anchors = vec![[0.0_f64; 3]; n];
    if n == 0 {
        return anchors;
    }
    let mut layer_values: Vec<u8> = graphs.iter().map(|g| g.layer).collect();
    layer_values.sort_unstable();
    layer_values.dedup();
    layer_values.reverse(); // highest layer first = most central

    let mut current_r = 0.0_f64;
    for (shell_idx, &lv) in layer_values.iter().enumerate() {
        let mut members: Vec<usize> = (0..n).filter(|&i| graphs[i].layer == lv).collect();
        members.sort_by(|&a, &b| graphs[a].id.cmp(&graphs[b].id));
        let max_b = members
            .iter()
            .map(|&i| graphs[i].out.bounding)
            .fold(0.0_f64, f64::max);

        if shell_idx == 0 && members.len() == 1 {
            anchors[members[0]] = [0.0, 0.0, 0.0];
            current_r = graphs[members[0]].out.bounding;
            continue;
        }

        let sum_b: f64 = members.iter().map(|&i| graphs[i].out.bounding).sum();
        // Ring must clear inner shells and give each graph room on the circle.
        let circumference_need = sum_b / std::f64::consts::PI;
        let ring_r = (current_r + gap + max_b).max(circumference_need + max_b);

        let widths: Vec<f64> = members
            .iter()
            .map(|&i| 2.0 * (graphs[i].out.bounding / ring_r).clamp(0.0, 1.0).asin())
            .collect();
        let used: f64 = widths.iter().sum();
        let slack = (2.0 * std::f64::consts::PI - used).max(0.0);
        let gap_ang = slack / members.len() as f64;

        let mut theta = 0.0_f64;
        for (k, &i) in members.iter().enumerate() {
            theta += widths[k] / 2.0;
            anchors[i] = [ring_r * theta.cos(), 0.0, ring_r * theta.sin()];
            theta += widths[k] / 2.0 + gap_ang;
        }
        current_r = ring_r + max_b;
    }
    anchors
}

/// Weak "membrane" springs: routes pull linked graphs together, but each anchor
/// is re-projected onto its own shell each step, so layers never dissolve. A
/// same-shell clearance keeps graphs from overlapping while they rotate.
fn nudge_routes(
    mut anchors: Vec<[f64; 3]>,
    graphs: &[GraphUnit],
    routes: &[(String, String)],
    attraction: f64,
) -> Vec<[f64; 3]> {
    let n = anchors.len();
    if attraction <= 0.0 || routes.is_empty() || n < 2 {
        return anchors;
    }
    let index: BTreeMap<String, usize> = graphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.id.clone(), i))
        .collect();
    let radius: Vec<f64> = anchors.iter().map(|anchor| norm(*anchor)).collect();

    for _ in 0..40 {
        let mut disp = vec![[0.0_f64; 3]; n];
        // Route attraction (weak).
        for (a, b) in routes {
            let (Some(&ia), Some(&ib)) = (index.get(a), index.get(b)) else {
                continue;
            };
            let pull = scale(sub(anchors[ib], anchors[ia]), attraction * 0.1);
            disp[ia] = add(disp[ia], pull);
            disp[ib] = sub(disp[ib], pull);
        }
        // Clearance so graphs never overlap while rotating.
        for i in 0..n {
            for j in (i + 1)..n {
                let min_sep = graphs[i].out.bounding + graphs[j].out.bounding;
                let d = sub(anchors[i], anchors[j]);
                let dist = norm(d).max(1e-6);
                if dist < min_sep {
                    let push = scale(scale(d, 1.0 / dist), (min_sep - dist) * 0.5);
                    disp[i] = add(disp[i], push);
                    disp[j] = sub(disp[j], push);
                }
            }
        }
        // Apply, re-projecting each anchor onto its own shell radius.
        for i in 0..n {
            if radius[i] < 1e-9 {
                continue; // the central graph stays at the core
            }
            let moved = add(anchors[i], disp[i]);
            let nn = norm(moved);
            anchors[i] = if nn < 1e-9 {
                anchors[i]
            } else {
                scale(moved, radius[i] / nn)
            };
        }
    }
    anchors
}

// ---------------------------------------------------------------------------
// City layout: one continuous field, gapless, shaped by link topology.
// ---------------------------------------------------------------------------

/// Repulsion acts only within this multiple of contact distance. Short-range
/// contact repulsion (NOT long-range inverse-square) keeps the city DENSE and
/// continuous instead of exploding into a sparse round cloud.
const CITY_CONTACT: f64 = 2.2;
/// Central-gravity unit: every node is pulled toward the city centre, scaled by
/// its layer (L4 hardest). This is the "force toward the centre" that makes the
/// fabric continuous — no gaps, no drifting islands.
const CITY_GRAVITY: f64 = 0.02;
/// Volume a weight-1 node occupies (= π/4, so its default footprint radius is
/// 0.5 at the default height). VOLUME IS CONSERVED per node.
const CITY_VOLUME_UNIT: f64 = std::f64::consts::PI * 0.25;
/// The height a node takes when it has room for its full desired footprint.
const CITY_DEFAULT_HEIGHT: f64 = 1.0;
/// Collision-radius floor during the field solve, so the city can pack DENSE
/// (density is precisely what later forces nodes to grow upward).
const CITY_MIN_RADIUS: f64 = 0.35;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CityLayout {
    pub placements: Vec<PlacedNode>,
    /// Hitbox overlaps across the WHOLE city — should be 0 (nothing superposes).
    pub residual_overlaps: usize,
    /// Per-graph centroid + extent (evidence; graphs are districts, not shells).
    pub graphs: Vec<GraphAnchor>,
    pub extent: f64,
    /// Fraction of the city disc actually filled by hitboxes, ×1e6. Higher ⇒ more
    /// continuous (less empty space between graphs).
    pub occupancy_micro: i64,
    /// Tallest built height — a node fully squeezed by its dense locality.
    pub max_height: f64,
}

impl CityLayout {
    pub fn position(&self, key: EntityKey) -> Option<[f64; 3]> {
        self.placements
            .iter()
            .find(|placed| placed.key == key)
            .map(|placed| placed.position)
    }
}

/// Continuous "city" layout: instead of laying each graph out in isolation and
/// spacing bounding circles on layer shells (which leaves gaps and forces round
/// blobs), this runs ONE global force field over every node. Cross-graph and
/// membrane links pull the districts together so the city is continuous with no
/// gaps; the shape of each district — and of the whole — emerges from the link
/// topology, not an imposed circle. A soft radial bias by layer keeps L4 central
/// without hard shells. Packing guarantees nothing superposes.
pub fn compute_city(
    input: &LayoutInput,
    clusters: &BTreeMap<EntityKey, String>,
    layers: &BTreeMap<String, u8>,
    membrane: Option<&str>,
) -> Result<CityLayout, LayoutError> {
    validate_params(&input.params)?;
    let cluster_of = |key: &EntityKey| clusters.get(key).cloned().unwrap_or_default();
    let is_membrane = |cid: &str| membrane == Some(cid);

    let mut radii: BTreeMap<EntityKey, f64> = BTreeMap::new();
    for node in &input.nodes {
        if radii.insert(node.key, node.radius.max(1e-6)).is_some() {
            return Err(LayoutError::DuplicateNode(node.key));
        }
    }
    let keys: Vec<EntityKey> = input.nodes.iter().map(|node| node.key).collect();
    let n = keys.len();
    if n == 0 {
        return Ok(CityLayout {
            placements: Vec::new(),
            residual_overlaps: 0,
            graphs: Vec::new(),
            extent: 0.0,
            occupancy_micro: 0,
            max_height: 0.0,
        });
    }
    let index: BTreeMap<EntityKey, usize> =
        keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();

    // Each node's layer (None ⇒ membrane node: no radial bias, floats where links pull it).
    let node_layer: Vec<Option<u8>> = keys
        .iter()
        .map(|key| {
            let cluster = cluster_of(key);
            if is_membrane(&cluster) {
                None
            } else {
                Some(layers.get(&cluster).copied().unwrap_or(0))
            }
        })
        .collect();
    // Distinct layers, highest first (most central). A node's central-gravity
    // strength scales with how high its layer is — L4 is pulled hardest.
    let mut present: Vec<u8> = node_layer.iter().flatten().copied().collect();
    present.sort_unstable();
    present.dedup();
    present.reverse();
    let shell_of: BTreeMap<u8, usize> =
        present.iter().enumerate().map(|(i, &l)| (l, i)).collect();
    let shells = present.len().max(1) as f64;
    let gravity_of = |layer: Option<u8>| -> f64 {
        match layer {
            // Highest layer (shell 0) → strongest pull → most central.
            Some(l) => CITY_GRAVITY * (shells - shell_of[&l] as f64),
            None => CITY_GRAVITY * 0.5, // membrane floats gently
        }
    };

    // --- Structural weight → conserved volume. Importance = graph degree
    // (referenced-ness); a much-linked node is heavier and claims more volume. ---
    let mut degree = vec![0u32; n];
    for link in &input.links {
        if let (Some(&a), Some(&b)) = (index.get(&link.source), index.get(&link.target)) {
            degree[a] += 1;
            degree[b] += 1;
        }
    }
    let volume: Vec<f64> = (0..n)
        .map(|i| CITY_VOLUME_UNIT * (1.0 + degree[i] as f64))
        .collect();
    // The footprint a node WANTS at the default height (unconstrained).
    let desired_r: Vec<f64> = volume
        .iter()
        .map(|v| (v / (std::f64::consts::PI * CITY_DEFAULT_HEIGHT)).sqrt())
        .collect();
    // Collision radius during the solve: a small floor so the field packs dense.
    let solve_r: Vec<f64> = desired_r
        .iter()
        .map(|r| (0.5 * r).max(CITY_MIN_RADIUS))
        .collect();

    // Deterministic seed: golden-angle, heavier layers seeded nearer the centre.
    let golden = std::f64::consts::PI * (1.0 + 5.0_f64.sqrt());
    let base_seed = (n as f64).sqrt() * CITY_MIN_RADIUS;
    let mut pos: Vec<[f64; 3]> = (0..n)
        .map(|i| {
            let shell = node_layer[i].map(|l| shell_of[&l]).unwrap_or(0) as f64;
            let r = (base_seed * (shell + 1.0)).max(1e-3);
            let a = golden * (i as f64 + 0.5);
            [r * a.cos(), 0.0, r * a.sin()]
        })
        .collect();

    let step_limit = base_seed * 0.5;
    for _ in 0..input.params.iterations {
        let mut disp = vec![[0.0_f64; 3]; n];

        // Short-range CONTACT repulsion only (keeps it dense, not exploded).
        for a in 0..n {
            for b in (a + 1)..n {
                let want = solve_r[a] + solve_r[b];
                let d = sub(pos[a], pos[b]);
                let dist = norm(d).max(1e-6);
                if dist < CITY_CONTACT * want {
                    let mag = input.params.repulsion * (CITY_CONTACT * want - dist);
                    let dir = scale(d, 1.0 / dist);
                    disp[a] = add(disp[a], scale(dir, mag));
                    disp[b] = sub(disp[b], scale(dir, mag));
                }
            }
        }

        // Link attraction shapes the districts by topology (cross-graph/membrane
        // links pull weakly so distinct graphs cohere without merging).
        for link in &input.links {
            let (Some(&a), Some(&b)) = (index.get(&link.source), index.get(&link.target)) else {
                continue;
            };
            let cross = cluster_of(&link.source) != cluster_of(&link.target);
            let strength = if cross {
                input.params.route_attraction
            } else {
                input.params.attraction
            };
            let d = sub(pos[b], pos[a]);
            let dist = norm(d).max(1e-6);
            let dir = scale(d, 1.0 / dist);
            let base = solve_r[a] + solve_r[b];
            let rest = base * (2.0 - link.similarity.clamp(0.0, 1.0));
            let bond = if link.inside { 1.0 } else { link.bond() };
            let mag = strength * bond * (dist - rest);
            disp[a] = add(disp[a], scale(dir, mag));
            disp[b] = sub(disp[b], scale(dir, mag));
        }

        // Central gravity (xz only): every node pulled toward the centre, scaled
        // by its layer. This is the continuous "force toward the centre" — no
        // gaps, no islands — and it makes L4 the dense core.
        for i in 0..n {
            let g = gravity_of(node_layer[i]);
            disp[i][0] -= g * pos[i][0];
            disp[i][2] -= g * pos[i][2];
        }

        for i in 0..n {
            let mut step = scale([disp[i][0], 0.0, disp[i][2]], input.params.damping);
            let sn = norm(step);
            if sn > step_limit {
                step = scale(step, step_limit / sn);
            }
            pos[i] = add(pos[i], step);
            pos[i][1] = 0.0; // nodes rest on the ground plane; height is built up
        }
    }

    let mut posmap: BTreeMap<EntityKey, [f64; 3]> =
        keys.iter().enumerate().map(|(i, k)| (*k, pos[i])).collect();
    let solve_radii: BTreeMap<EntityKey, f64> =
        keys.iter().enumerate().map(|(i, k)| (*k, solve_r[i])).collect();
    pack(&keys, &solve_radii, &mut posmap, input.params.packing_passes);
    center(&keys, &mut posmap);
    let settled: Vec<[f64; 3]> = keys.iter().map(|k| posmap[k]).collect();

    // --- Volume conserved → footprint + height. A node takes the footprint its
    // LOCALITY allows (half the distance to its nearest neighbour, capped at what
    // its weight wants); the rest of its conserved volume becomes HEIGHT. Dense
    // centre ⇒ tall towers; open periphery ⇒ low, wide pavilions. ---
    let placements: Vec<PlacedNode> = (0..n)
        .map(|i| {
            let nearest = (0..n)
                .filter(|&j| j != i)
                .map(|j| norm(sub(settled[i], settled[j])))
                .fold(f64::INFINITY, f64::min);
            let room = if nearest.is_finite() {
                0.5 * nearest
            } else {
                desired_r[i]
            };
            let footprint = room.clamp(CITY_MIN_RADIUS, desired_r[i]).min(room);
            let footprint = footprint.max(1e-3);
            let height = volume[i] / (std::f64::consts::PI * footprint * footprint);
            PlacedNode {
                key: keys[i],
                position: settled[i],
                depth: 0,
                scale: 1.0,
                radius: footprint,
                height,
                space: None,
            }
        })
        .collect();
    let residual_overlaps = count_all_overlaps(&placements);

    let extent = placements
        .iter()
        .map(|p| norm(p.position) + p.radius)
        .fold(0.0_f64, f64::max);
    let node_area: f64 = placements
        .iter()
        .map(|p| std::f64::consts::PI * p.radius * p.radius)
        .sum();
    let city_area = std::f64::consts::PI * extent * extent;
    let occupancy = if city_area > 0.0 {
        (node_area / city_area).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let max_height = placements.iter().map(|p| p.height).fold(0.0_f64, f64::max);

    // District evidence: centroid + extent per graph (not a shell anchor).
    let mut by_cluster: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, key) in keys.iter().enumerate() {
        by_cluster.entry(cluster_of(key)).or_default().push(i);
    }
    let graphs: Vec<GraphAnchor> = by_cluster
        .iter()
        .map(|(cluster, members)| {
            let sum = members
                .iter()
                .fold([0.0; 3], |acc, &i| add(acc, placements[i].position));
            let centroid = scale(sum, 1.0 / members.len() as f64);
            let bounding = members
                .iter()
                .map(|&i| norm(sub(placements[i].position, centroid)) + placements[i].radius)
                .fold(0.0_f64, f64::max);
            GraphAnchor {
                cluster: cluster.clone(),
                layer: layers.get(cluster).copied().unwrap_or(0),
                anchor: centroid,
                bounding,
                node_count: members.len(),
            }
        })
        .collect();

    Ok(CityLayout {
        placements,
        residual_overlaps,
        graphs,
        extent,
        occupancy_micro: (occupancy * 1_000_000.0) as i64,
        max_height,
    })
}

fn count_all_overlaps(placements: &[PlacedNode]) -> usize {
    let mut overlaps = 0;
    for a in 0..placements.len() {
        for b in (a + 1)..placements.len() {
            let min_sep = placements[a].radius + placements[b].radius;
            if norm(sub(placements[a].position, placements[b].position)) + 1e-6 < min_sep {
                overlaps += 1;
            }
        }
    }
    overlaps
}

fn count_cross_graph_overlaps(graphs: &[GraphUnit], anchors: &[[f64; 3]]) -> usize {
    let mut overlaps = 0;
    for i in 0..graphs.len() {
        for j in (i + 1)..graphs.len() {
            let min_sep = graphs[i].out.bounding + graphs[j].out.bounding;
            let dist = norm(sub(anchors[i], anchors[j]));
            if dist + 1e-6 < min_sep {
                overlaps += 1;
            }
        }
    }
    overlaps
}

/// Validates that layout parameters are in their admissible ranges. Public so a
/// graph-native layout authority can validate a materialized policy by the SAME
/// rules the kernel enforces.
pub fn validate_params(params: &LayoutParams) -> Result<(), LayoutError> {
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
) -> (BTreeMap<EntityKey, [f64; 3]>, f64) {
    let n = members.len();
    let mut pos: BTreeMap<EntityKey, [f64; 3]> = BTreeMap::new();
    if n == 0 {
        return (pos, 0.0);
    }
    if n == 1 {
        pos.insert(members[0], [0.0, 0.0, 0.0]);
        return (pos, radii[&members[0]]);
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
            !link.inside && member_set.contains(&link.source) && member_set.contains(&link.target)
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

    // Local bounding radius: furthest node centre plus its own radius.
    let bounding = keys
        .iter()
        .map(|key| norm(pos[key]) + radii[key])
        .fold(0.0_f64, f64::max);
    (pos, bounding)
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
                        [
                            seed.fract() - 0.5,
                            ((a + b) as f64 * 0.3).fract() - 0.5,
                            0.5,
                        ]
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
        keys.iter()
            .map(|k| LayoutNode::new(EntityKey(*k)))
            .collect()
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
        let room = layout
            .placements
            .iter()
            .find(|p| p.key == EntityKey(1))
            .unwrap();
        let desk = layout
            .placements
            .iter()
            .find(|p| p.key == EntityKey(2))
            .unwrap();
        let pen = layout
            .placements
            .iter()
            .find(|p| p.key == EntityKey(3))
            .unwrap();
        assert_eq!(room.depth, 0);
        assert_eq!(desk.depth, 1);
        assert_eq!(pen.depth, 2);
        assert!(
            desk.scale < room.scale,
            "descent must shrink absolute scale"
        );
        assert!(pen.scale < desk.scale, "each descent shrinks further");
        assert!(pen.radius < desk.radius && desk.radius < room.radius);
    }

    #[test]
    fn sibling_spaces_contents_do_not_collide() {
        // root(1) ⊃ {A(2), B(3)}; A ⊃ {4,5,6,7}; B ⊃ {8,9,10,11}. With a large
        // descent scale the contents would spill past a fixed-size container and
        // A's nodes would hit B's — unless the container hitbox is sized to hold
        // its packed contents. Assert zero cross-space collisions.
        let mut links = vec![
            LayoutLink::containment(EntityKey(2), EntityKey(1)),
            LayoutLink::containment(EntityKey(3), EntityKey(1)),
        ];
        let a_kids = [4u128, 5, 6, 7];
        let b_kids = [8u128, 9, 10, 11];
        for k in a_kids {
            links.push(LayoutLink::containment(EntityKey(k), EntityKey(2)));
        }
        for k in b_kids {
            links.push(LayoutLink::containment(EntityKey(k), EntityKey(3)));
        }
        let mut all: Vec<u128> = vec![1, 2, 3];
        all.extend(a_kids);
        all.extend(b_kids);
        let params = LayoutParams {
            scale_per_descent: 0.5, // large descent ⇒ contents nearly fill the parent
            ..LayoutParams::default()
        };
        let layout = compute(&LayoutInput {
            nodes: nodes(&all),
            links,
            params,
        })
        .unwrap();
        let find = |k: u128| {
            *layout
                .placements
                .iter()
                .find(|p| p.key == EntityKey(k))
                .unwrap()
        };
        for &a in &a_kids {
            for &b in &b_kids {
                let (pa, pb) = (find(a), find(b));
                let sep = norm(sub(pa.position, pb.position));
                let min = pa.radius + pb.radius;
                assert!(
                    sep + 1e-9 >= min,
                    "cross-space collision {a}/{b}: sep={sep} < min={min}"
                );
            }
        }
        // And the metric agrees no PEERS overlap anywhere in the tree.
        assert_eq!(layout.residual_overlaps, 0);
    }

    fn clustered_universe() -> (LayoutInput, BTreeMap<EntityKey, String>, BTreeMap<String, u8>) {
        // l4 (1 node) central; two l3 graphs; one l2 graph, outermost.
        let mut clusters: BTreeMap<EntityKey, String> = BTreeMap::new();
        let assign = |clusters: &mut BTreeMap<EntityKey, String>, ks: &[u128], id: &str| {
            for k in ks {
                clusters.insert(EntityKey(*k), id.to_owned());
            }
        };
        assign(&mut clusters, &[1], "l4-core");
        assign(&mut clusters, &[2, 3], "l3-a");
        assign(&mut clusters, &[4, 5], "l3-b");
        assign(&mut clusters, &[6, 7, 8], "l2-org");
        let layers = BTreeMap::from([
            ("l4-core".to_owned(), 4u8),
            ("l3-a".to_owned(), 3u8),
            ("l3-b".to_owned(), 3u8),
            ("l2-org".to_owned(), 2u8),
        ]);
        let input = LayoutInput {
            nodes: nodes(&[1, 2, 3, 4, 5, 6, 7, 8]),
            links: vec![
                LayoutLink::generic(EntityKey(2), EntityKey(3)),
                LayoutLink::generic(EntityKey(6), EntityKey(7)),
            ],
            params: LayoutParams::default(),
        };
        (input, clusters, layers)
    }

    fn anchor_norm(layout: &ClusteredLayout, cluster: &str) -> f64 {
        let a = layout
            .graphs
            .iter()
            .find(|g| g.cluster == cluster)
            .unwrap()
            .anchor;
        norm(a)
    }

    #[test]
    fn clusters_arrange_by_layer_with_l4_central() {
        let (input, clusters, layers) = clustered_universe();
        let layout = compute_clustered(&input, &clusters, &layers, None).unwrap();
        // L4 core sits at the centre; L3 graphs orbit it; L2 is further out.
        assert!(anchor_norm(&layout, "l4-core") < 1e-6, "L4 must be central");
        let l3a = anchor_norm(&layout, "l3-a");
        let l3b = anchor_norm(&layout, "l3-b");
        let l2 = anchor_norm(&layout, "l2-org");
        assert!(l3a > 1e-6 && l3b > 1e-6, "L3 graphs are off-centre");
        assert!(l2 > l3a && l2 > l3b, "L2 sits on a shell beyond L3");
        // Whole graphs never overlap, and peers never overlap.
        assert_eq!(layout.cross_graph_overlaps, 0);
        assert_eq!(layout.residual_overlaps, 0);
        assert_eq!(layout.graphs.len(), 4);
    }

    #[test]
    fn routes_nudge_without_breaking_shells() {
        let (mut input, clusters, layers) = clustered_universe();
        // A route (membrane) between the L4 core and the L2 org.
        input.links.push(LayoutLink::generic(EntityKey(1), EntityKey(6)));

        let with_routes = compute_clustered(&input, &clusters, &layers, None).unwrap();
        let mut no_route_params = input.clone();
        no_route_params.params.route_attraction = 0.0;
        let without = compute_clustered(&no_route_params, &clusters, &layers, None).unwrap();

        // Each graph keeps its shell radius (routes only rotate within a shell).
        for cluster in ["l4-core", "l3-a", "l3-b", "l2-org"] {
            let a = anchor_norm(&with_routes, cluster);
            let b = anchor_norm(&without, cluster);
            assert!(
                (a - b).abs() < 1e-6,
                "route changed shell radius of {cluster}: {a} vs {b}"
            );
        }
        assert_eq!(with_routes.cross_graph_overlaps, 0);
        assert_eq!(with_routes.routes, 1, "the cross-graph link is a route");
    }

    #[test]
    fn unclustered_nodes_form_one_default_graph() {
        let input = LayoutInput {
            nodes: nodes(&[1, 2, 3]),
            links: vec![LayoutLink::generic(EntityKey(1), EntityKey(2))],
            params: LayoutParams::default(),
        };
        let layout =
            compute_clustered(&input, &BTreeMap::new(), &BTreeMap::new(), None).unwrap();
        assert_eq!(layout.graphs.len(), 1, "everything falls into one graph");
        assert_eq!(layout.routes, 0);
        assert_eq!(layout.cross_graph_overlaps, 0);
    }

    #[test]
    fn membrane_graph_is_unbounded_and_routes_between_graphs() {
        // l4-core (central) and l2-org (outer) are bounded; a membrane node
        // bridges them. The membrane graph gets no shell, and its bridging node
        // sits on the route between the two graphs.
        let mut clusters: BTreeMap<EntityKey, String> = BTreeMap::new();
        for k in [1u128, 2] {
            clusters.insert(EntityKey(k), "l4-core".to_owned());
        }
        for k in [6u128, 7, 8] {
            clusters.insert(EntityKey(k), "l2-org".to_owned());
        }
        clusters.insert(EntityKey(100), "membrane".to_owned());
        let layers = BTreeMap::from([("l4-core".to_owned(), 4u8), ("l2-org".to_owned(), 2u8)]);
        let input = LayoutInput {
            nodes: nodes(&[1, 2, 6, 7, 8, 100]),
            links: vec![
                LayoutLink::generic(EntityKey(1), EntityKey(2)),
                LayoutLink::generic(EntityKey(6), EntityKey(7)),
                // membrane node 100 bridges the two bounded graphs
                LayoutLink::generic(EntityKey(100), EntityKey(1)),
                LayoutLink::generic(EntityKey(100), EntityKey(6)),
            ],
            params: LayoutParams::default(),
        };
        let layout = compute_clustered(&input, &clusters, &layers, Some("membrane")).unwrap();

        // The membrane graph is NOT a bounded, shelled graph.
        assert_eq!(layout.graphs.len(), 2, "only bounded graphs get shells");
        assert!(layout.graphs.iter().all(|g| g.cluster != "membrane"));
        assert_eq!(layout.membrane_nodes, 1);
        assert!(layout.routes >= 1, "the membrane bridges l4 and l2");
        assert_eq!(layout.cross_graph_overlaps, 0);

        // The bridging membrane node sits between the two graphs it connects.
        let l2 = layout.graphs.iter().find(|g| g.cluster == "l2-org").unwrap();
        let l2_norm = norm(l2.anchor);
        let m = norm(layout.position(EntityKey(100)).unwrap());
        assert!(
            m > 1e-6 && m < l2_norm,
            "membrane node must sit on the route between graphs: {m} vs {l2_norm}"
        );
    }

    /// Aspect ratio of a point cloud in the xz-plane (principal spread / minor
    /// spread), via the 2×2 covariance eigenvalues. 1.0 = round; larger = elongated.
    fn xz_aspect_ratio(points: &[[f64; 3]]) -> f64 {
        let nf = points.len() as f64;
        let (mut mx, mut mz) = (0.0, 0.0);
        for p in points {
            mx += p[0];
            mz += p[2];
        }
        mx /= nf;
        mz /= nf;
        let (mut sxx, mut szz, mut sxz) = (0.0, 0.0, 0.0);
        for p in points {
            let (dx, dz) = (p[0] - mx, p[2] - mz);
            sxx += dx * dx;
            szz += dz * dz;
            sxz += dx * dz;
        }
        let (sxx, szz, sxz) = (sxx / nf, szz / nf, sxz / nf);
        let tr = sxx + szz;
        let det = sxx * szz - sxz * sxz;
        let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
        let l1 = tr / 2.0 + disc;
        let l2 = (tr / 2.0 - disc).max(1e-9);
        (l1 / l2).sqrt()
    }

    fn city_input(keys: &[u128], links: Vec<LayoutLink>) -> LayoutInput {
        LayoutInput {
            nodes: nodes(keys),
            links,
            params: LayoutParams::default(),
        }
    }

    #[test]
    fn city_has_no_overlaps_and_is_deterministic() {
        let input = city_input(
            &[1, 2, 3, 4, 5, 6],
            vec![
                LayoutLink::generic(EntityKey(1), EntityKey(2)),
                LayoutLink::generic(EntityKey(4), EntityKey(5)),
            ],
        );
        let clusters = BTreeMap::new();
        let layers = BTreeMap::new();
        let a = compute_city(&input, &clusters, &layers, None).unwrap();
        let b = compute_city(&input, &clusters, &layers, None).unwrap();
        assert_eq!(a, b, "city layout is deterministic");
        assert_eq!(a.residual_overlaps, 0, "nothing superposes in the city");
    }

    #[test]
    fn city_keeps_l4_central() {
        let mut clusters: BTreeMap<EntityKey, String> = BTreeMap::new();
        for k in [1u128, 2, 3, 4] {
            clusters.insert(EntityKey(k), "l4-core".to_owned());
        }
        for k in [10u128, 11, 12, 13] {
            clusters.insert(EntityKey(k), "l2-org".to_owned());
        }
        let layers = BTreeMap::from([("l4-core".to_owned(), 4u8), ("l2-org".to_owned(), 2u8)]);
        let input = city_input(&[1, 2, 3, 4, 10, 11, 12, 13], vec![]);
        let city = compute_city(&input, &clusters, &layers, None).unwrap();
        let mean_r = |ks: &[u128]| -> f64 {
            let s: f64 = ks
                .iter()
                .map(|k| {
                    let p = city.position(EntityKey(*k)).unwrap();
                    (p[0] * p[0] + p[2] * p[2]).sqrt()
                })
                .sum();
            s / ks.len() as f64
        };
        assert!(
            mean_r(&[1, 2, 3, 4]) < mean_r(&[10, 11, 12, 13]),
            "L4 district stays nearer the centre than L2"
        );
        assert_eq!(city.residual_overlaps, 0);
    }

    #[test]
    fn weight_becomes_volume_and_density_becomes_height() {
        // A hub referenced by many leaves is heavy; sitting in the dense centre it
        // cannot spread, so its conserved volume becomes HEIGHT (a tower). Volume
        // (π r² h) must exceed a leaf's, and the hub must be taller.
        let links: Vec<LayoutLink> = (2u128..=15)
            .map(|k| LayoutLink::generic(EntityKey(1), EntityKey(k)))
            .collect();
        let city = compute_city(
            &city_input(&(1u128..=15).collect::<Vec<_>>(), links),
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
        )
        .unwrap();
        let node = |k: u128| *city.placements.iter().find(|p| p.key == EntityKey(k)).unwrap();
        let vol = |p: PlacedNode| std::f64::consts::PI * p.radius * p.radius * p.height;
        let hub = node(1);
        let leaf = node(9);
        assert!(
            vol(hub) > vol(leaf) * 1.5,
            "heavy hub carries more volume: hub={} leaf={}",
            vol(hub),
            vol(leaf)
        );
        assert!(
            hub.height > leaf.height,
            "the squeezed heavy hub grows into a tower: hub h={} leaf h={}",
            hub.height,
            leaf.height
        );
        assert_eq!(city.residual_overlaps, 0, "footprints never overlap");
    }

    #[test]
    fn city_shape_follows_topology_not_round() {
        // A chain (path) should be markedly more elongated than a star (hub),
        // proving the global shape follows link topology rather than a circle.
        let chain_links: Vec<LayoutLink> = (1u128..12)
            .map(|k| LayoutLink::generic(EntityKey(k), EntityKey(k + 1)))
            .collect();
        let chain = compute_city(
            &city_input(&(1u128..=12).collect::<Vec<_>>(), chain_links),
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
        )
        .unwrap();
        let star_links: Vec<LayoutLink> = (2u128..=12)
            .map(|k| LayoutLink::generic(EntityKey(1), EntityKey(k)))
            .collect();
        let star = compute_city(
            &city_input(&(1u128..=12).collect::<Vec<_>>(), star_links),
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
        )
        .unwrap();
        let chain_ratio = xz_aspect_ratio(&chain.placements.iter().map(|p| p.position).collect::<Vec<_>>());
        let star_ratio = xz_aspect_ratio(&star.placements.iter().map(|p| p.position).collect::<Vec<_>>());
        assert!(
            chain_ratio > star_ratio,
            "chain must be more elongated than star: chain={chain_ratio} star={star_ratio}"
        );
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
        assert!(
            p1[1] < p2[1],
            "positive-hierarchy source should sit below target"
        );
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
        assert!(
            p1[1] > p2[1],
            "negative-hierarchy source should sit above target"
        );
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
        assert!(
            ds < dw,
            "stronger bond must settle closer: strong={ds} weak={dw}"
        );
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
        assert_eq!(
            link.polarity,
            [0.5, 0.5],
            "unknown predicate is neutrally bonded"
        );
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
        assert!(
            input.links.is_empty(),
            "a link to an absent node is not placeable"
        );
    }

    #[test]
    fn stable_key_is_deterministic_and_distinct() {
        assert_eq!(stable_key("l4-node-type-mapping"), stable_key("l4-node-type-mapping"));
        assert_ne!(stable_key("a"), stable_key("b"));
        assert_ne!(stable_key(""), stable_key("x"));
        // Different ids across a realistic set never collide.
        let ids = ["PART_OF", "DEFINES", "l4-core", "terme-type-protocol", "l4-translation-contract"];
        let keys: BTreeSet<_> = ids.iter().map(|id| stable_key(id)).collect();
        assert_eq!(keys.len(), ids.len());
    }

    #[test]
    fn duplicate_node_is_rejected() {
        let input = LayoutInput {
            nodes: nodes(&[1, 1]),
            links: vec![],
            params: LayoutParams::default(),
        };
        assert_eq!(
            compute(&input),
            Err(LayoutError::DuplicateNode(EntityKey(1)))
        );
    }
}
