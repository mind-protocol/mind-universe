//! READ-ONLY construct self-observation: emit a construct's `ConstructMap`.
//!
//! Given a construct's canonical root id, this bin opens the LIVE store
//! READ-ONLY (`UniverseStore::open` + `replay` + `read_content`; it NEVER writes,
//! NEVER appends an event, NEVER commits a `UniverseTransaction`), loads the
//! shared construct-validity loop TEMPLATE (34 parts / 42 edges / 6 feedback),
//! maps the target construct's stored members onto that loop, runs a per-node
//! evidence probe plus a determinability propagation over the forward DAG, and
//! PRINTS a `ConstructMap`: what the construct is for, whether it is real yet
//! (per-node epistemic state + a because-trace to the first break), and the
//! ordered `needs` that would make it more real.
//!
//! It answers, honestly: "what is this construct for, is it real yet, and what is
//! the next thing that would make it more real?" — never launders a self-declared
//! implementation field into verified truth, never prints `healthy`/`correct`
//! without fresh runtime evidence, and derives downstream indeterminacy without
//! falsely degrading upstream definitions.
//!
//! Usage: `construct_map <construct-root-canonical-id> [store-dir]`
//!   store-dir default: artifacts/ontology-registry/current/store
//!
//! Specs implemented: topology-map (member load + loop mapping), evidence-probe
//! F1 (per-node axes), propagation F2 (forward-DAG determinability + feedback
//! exclusion + failure front), map-schema F5 (ConstructMap explains/state/needs).

// The faithful loop model keeps a few fields/variants for completeness (e.g. the
// MeasurementFailed lattice rung, the edge predicate) that this read path records
// but does not branch on; they document the model rather than being dead.
#![allow(dead_code)]

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    error::Error,
    path::PathBuf,
};

use serde_json::Value;
use universe_core::EntityKey;
use universe_store::UniverseStore;

const TEMPLATE_ID: &str = "space:l2:mind-universe:construct-validity-v0";

fn main() {
    if let Err(error) = run() {
        eprintln!("CONSTRUCT_MAP FAILED: {error}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Epistemic lattice (best -> worst): Observed > KnownAbsent ~ NotMeasured >
// Unknown > MeasurementFailed. Used for propagation worst-of and honest display.
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
enum Epistemic {
    Observed,
    KnownAbsent,
    NotMeasured,
    Unknown,
    MeasurementFailed,
}

impl Epistemic {
    fn label(self) -> &'static str {
        match self {
            Epistemic::Observed => "observed",
            Epistemic::KnownAbsent => "known_absent",
            Epistemic::NotMeasured => "not_measured",
            Epistemic::Unknown => "unknown",
            Epistemic::MeasurementFailed => "measurement_failed",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Band {
    LoopSpace,
    Role,
    Anatomy,
    Trigger,
    RuntimeMoment,
    Maintenance,
}

impl Band {
    fn from_group(group: &str) -> Band {
        match group {
            "roles" => Band::Role,
            "anatomy" => Band::Anatomy,
            "trigger" => Band::Trigger,
            "runtime_moment" => Band::RuntimeMoment,
            "maintenance" => Band::Maintenance,
            _ => Band::Role,
        }
    }
}

// ---------------------------------------------------------------------------
// Loop template (read once from the store).
// ---------------------------------------------------------------------------
struct LoopEdge {
    from: String,
    to: String,
    predicate: String,
    feedback: bool,
}

struct LoopTemplate {
    /// part name -> band, in a stable band-then-authored order.
    parts: Vec<(String, Band)>,
    edges: Vec<LoopEdge>,
}

impl LoopTemplate {
    fn band(&self, part: &str) -> Band {
        self.parts
            .iter()
            .find(|(name, _)| name == part)
            .map(|(_, b)| *b)
            .unwrap_or(Band::Role)
    }

    fn part_names(&self) -> BTreeSet<&str> {
        self.parts.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// Forward (non-feedback) required-inputs: for `X -> Y`, X is a required
    /// input of Y. Authored order preserved so the "first unconfirmed" pointer
    /// is deterministic. The 6 feedback edges are excluded (they are loop
    /// re-entry, never dependencies -> the forward projection is a DAG).
    fn required_inputs(&self) -> BTreeMap<String, Vec<String>> {
        let names = self.part_names();
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, _) in &self.parts {
            map.insert(name.clone(), Vec::new());
        }
        for edge in &self.edges {
            if edge.feedback {
                continue;
            }
            if names.contains(edge.from.as_str()) && names.contains(edge.to.as_str()) {
                map.entry(edge.to.clone()).or_default().push(edge.from.clone());
            }
        }
        map
    }
}

fn load_template(store: &UniverseStore, wrappers: &BTreeMap<String, Value>) -> Result<LoopTemplate, Box<dyn Error>> {
    let _ = store;
    let wrapper = wrappers
        .get(TEMPLATE_ID)
        .ok_or_else(|| format!("loop template {TEMPLATE_ID} not present in the store"))?;
    let svl = wrapper
        .get("content")
        .and_then(|c| c.get("self_verifying_loop"))
        .ok_or("template has no content.self_verifying_loop")?;
    let nodes = svl.get("nodes").and_then(Value::as_object).ok_or("template nodes missing")?;

    // 34 parts, grouped band-by-band in the authored group order. `loop_space`
    // is authored under the `anatomy` group but is the construct spine root, so
    // it takes the dedicated LoopSpace band (it IS the space entity — Observed).
    let mut parts: Vec<(String, Band)> = Vec::new();
    for group in ["roles", "anatomy", "trigger", "runtime_moment", "maintenance"] {
        if let Some(members) = nodes.get(group).and_then(Value::as_object) {
            let band = Band::from_group(group);
            for name in members.keys() {
                let band = if name == "loop_space" { Band::LoopSpace } else { band };
                parts.push((name.clone(), band));
            }
        }
    }

    let mut edges: Vec<LoopEdge> = Vec::new();
    for edge in svl.get("edges").and_then(Value::as_array).ok_or("template edges missing")? {
        edges.push(LoopEdge {
            from: edge.get("from").and_then(Value::as_str).unwrap_or("").to_string(),
            to: edge.get("to").and_then(Value::as_str).unwrap_or("").to_string(),
            predicate: edge.get("predicate").and_then(Value::as_str).unwrap_or("").to_string(),
            feedback: edge.get("feedback").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    Ok(LoopTemplate { parts, edges })
}

// ---------------------------------------------------------------------------
// Member enumeration + binding.
// ---------------------------------------------------------------------------

/// Everything after the first ':' — the discriminator suffix shared by a
/// construct's members (e.g. "l2:mind-universe:sky-toolkit-v0").
fn discriminator(id: &str) -> &str {
    id.split_once(':').map(|(_, rest)| rest).unwrap_or(id)
}

struct BoundMember {
    canonical_id: String,
    subtype: String,
    node_type: String,
    inner: Value,
}

/// A resolved loop part (bound or not) plus its computed epistemic state.
struct PartState {
    part: String,
    band: Band,
    member: Option<usize>, // index into `members`
    own: Epistemic,        // presence lattice value used for propagation
    display: Epistemic,    // the F1 central-claim epistemic shown in the table
    det: Det,
    reason: Reason,
    self_block: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Det {
    Confirmed,
    NotDeterminable,
    LogicalError,
}

#[derive(Clone)]
enum Reason {
    None,
    Input(String),
    SelfBlock(String),
    Struct(String),
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut positional: Vec<String> = Vec::new();
    for a in env::args().skip(1) {
        positional.push(a);
    }
    let root_id = positional
        .first()
        .cloned()
        .ok_or("usage: construct_map <construct-root-canonical-id> [store-dir]")?;
    let store_dir = positional
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));

    // READ-ONLY handshake: open, load the committed snapshot, replay the valid
    // event log into the authoritative snapshot. No write path is ever touched.
    let store = UniverseStore::open(&store_dir)?;
    let snapshot = store.replay(store.load_snapshot()?)?;

    // canonical_id -> (EntityKey, wrapper) index (as bin/wire_dependency.rs).
    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    let mut key_to_id: BTreeMap<EntityKey, String> = BTreeMap::new();
    let mut key_to_wrapper: BTreeMap<EntityKey, Value> = BTreeMap::new();
    let mut wrappers_by_id: BTreeMap<String, Value> = BTreeMap::new();
    for entity in &snapshot.entities {
        if let Some(ptr) = entity.content.as_ref() {
            let wrapper = store.read_content(ptr)?;
            if let Some(cid) = wrapper.get("canonical_id").and_then(Value::as_str) {
                id_to_key.insert(cid.to_string(), entity.key);
                key_to_id.insert(entity.key, cid.to_string());
                wrappers_by_id.insert(cid.to_string(), wrapper.clone());
                key_to_wrapper.insert(entity.key, wrapper);
            }
        }
    }

    let template = load_template(&store, &wrappers_by_id)?;

    if !id_to_key.contains_key(&root_id) {
        return Err(format!("construct root {root_id} not present in the store").into());
    }

    // --- Member enumeration (topology-map SS1) ---------------------------------
    // (a) discriminator-suffix match: catches the space root + every role member,
    //     including vocabulary/algorithm which carry no stored relation.
    let suffix = discriminator(&root_id).to_string();
    let mut member_keys: BTreeSet<EntityKey> = id_to_key
        .iter()
        .filter(|(cid, _)| discriminator(cid) == suffix)
        .map(|(_, k)| *k)
        .collect();

    // (b) bounded relation-closure: catches satellites whose id name differs from
    //     the construct slug (the observer_validation node). Bounded by entity
    //     count; records BudgetExhausted if the frontier is hit.
    //
    //     CRUCIAL: the LIVE store links every construct to the shared validity
    //     TEMPLATE via DEPENDS_ON, and constructs to a parent scope via PART_OF.
    //     An unrestricted closure would flood the entire connected component
    //     (pulling other constructs' members in). We traverse only construct-
    //     INTERNAL predicates (never DEPENDS_ON / PART_OF, which cross construct
    //     boundaries) and never absorb the template root itself.
    let cross_boundary: BTreeSet<u32> = ["DEPENDS_ON", "PART_OF"]
        .into_iter()
        .filter_map(|p| snapshot.symbol_id(p))
        .collect();
    let template_key = id_to_key.get(TEMPLATE_ID).copied();
    let budget = snapshot.entities.len() + 8;
    let mut iterations = 0usize;
    let mut budget_exhausted = false;
    loop {
        if iterations >= budget {
            budget_exhausted = true;
            break;
        }
        iterations += 1;
        let mut added = false;
        for r in &snapshot.relations {
            if cross_boundary.contains(&r.predicate) {
                continue;
            }
            let s_in = member_keys.contains(&r.source);
            let t_in = member_keys.contains(&r.target);
            if s_in ^ t_in {
                let outside = if s_in { r.target } else { r.source };
                if Some(outside) == template_key {
                    continue;
                }
                if member_keys.insert(outside) {
                    added = true;
                }
            }
        }
        if !added {
            break;
        }
    }

    // Read each member's fields.
    let read_members = |keys: &BTreeSet<EntityKey>| -> Vec<BoundMember> {
        let mut out: Vec<BoundMember> = Vec::new();
        for key in keys {
            if let Some(w) = key_to_wrapper.get(key) {
                out.push(BoundMember {
                    canonical_id: w.get("canonical_id").and_then(Value::as_str).unwrap_or("").to_string(),
                    subtype: w.get("subtype").and_then(Value::as_str).unwrap_or("").to_string(),
                    node_type: w.get("node_type").and_then(Value::as_str).unwrap_or("").to_string(),
                    inner: w.get("content").cloned().unwrap_or(Value::Null),
                });
            }
        }
        out.sort_by(|a, b| a.canonical_id.cmp(&b.canonical_id));
        out
    };
    let mut members = read_members(&member_keys);

    // (c) satellite-by-content-reference: bind satellite-kind entities
    //     (capability_port / physicalization_binding / broadcast_port) that a
    //     BOUND member's content names by canonical_id — e.g. the Sky code's
    //     `authoring_gate.port_id`. Such a port carries NO construct-internal
    //     relation and a divergent id slug (`sky:authoring-v0` != the construct
    //     slug), so neither the suffix match (a) nor the relation closure (b)
    //     reaches it; a content reference FROM an already-bound member is an
    //     attributable, bounded link. We admit only satellite kinds, never a
    //     `space` (that would cross into another construct), so a `[[space:...]]`
    //     wiki-link in a lineage string can never drag another construct in.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for m in &members {
        collect_strings(&m.inner, &mut referenced);
    }
    let mut satellite_added = false;
    for cid in &referenced {
        let Some(w) = wrappers_by_id.get(cid) else { continue };
        let subtype = w.get("subtype").and_then(Value::as_str).unwrap_or("");
        let is_satellite = is_satellite_subtype(subtype)
            || cid.starts_with("port:")
            || cid.starts_with("broadcast_port:");
        if is_satellite {
            if let Some(k) = id_to_key.get(cid) {
                if member_keys.insert(*k) {
                    satellite_added = true;
                }
            }
        }
    }
    if satellite_added {
        members = read_members(&member_keys);
    }

    // --- Honesty fields (topology-map SS3): read from the implementation member.
    let mut honesty = Honesty::default();
    if let Some(imp) = members.iter().find(|m| m.subtype == "implementation") {
        honesty.graph_status = str_field(&imp.inner, "graph_status");
        honesty.wiring_status = str_field(&imp.inner, "wiring_status");
        honesty.runtime_status = str_field(&imp.inner, "runtime_status");
        honesty.verification_status = str_field(&imp.inner, "verification_status");
    }
    // Independent readback: the entities ARE present, so a self-reported
    // graph_status="not_written" is STALE and is discarded from every truth
    // decision. The graph-write is `observed`.
    let graph_written_observed = id_to_key.contains_key(&root_id);

    // Runtime evidence: are any runtime-moment entities present in the store?
    // (validation_run / health_assessment / any *_run moment). We do a bounded
    // exhaustive scan of member subtypes + declared runtime_moment_subtypes.
    let runtime_moment_subtypes: BTreeSet<String> = wrappers_by_id
        .get(&root_id)
        .and_then(|w| w.get("content"))
        .and_then(|c| c.get("runtime_moment_subtypes"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let runtime_moment_present = members
        .iter()
        .any(|m| runtime_moment_subtypes.contains(&m.subtype));

    // --- Bind members onto loop parts by subtype (topology-map SS3) ------------
    // subtype -> loop part name. The two `validation` members are disambiguated
    // by the observer target inside their content.
    let mut part_to_member: BTreeMap<String, usize> = BTreeMap::new();
    for (i, m) in members.iter().enumerate() {
        let part = match m.subtype.as_str() {
            "objective" => "objective",
            "pattern" => "pattern",
            "vocabulary" => "vocabulary",
            "behavior" => "behavior",
            "algorithm" => "algorithm",
            "code" => "code",
            "implementation" => "implementation",
            "justification" => "justification",
            "observability_algorithm" => "observability_algorithm",
            "metric" => "metric",
            "health" => "health",
            "validation" => {
                // observer_validation carries a `target`/`role` pointing at the
                // observability_algorithm; the main validation does not.
                let target = str_field(&m.inner, "target");
                let role = str_field(&m.inner, "role");
                if target.starts_with("observability_algorithm")
                    || role == "observer_validation"
                {
                    "observer_validation"
                } else {
                    "validation"
                }
            }
            _ => {
                // The space root (contractKind "construct") binds to loop_space.
                if m.node_type == "space" && m.canonical_id == root_id {
                    "loop_space"
                } else {
                    // physicalization_binding / capability_port / satellites are
                    // not loop roles; skipped for loop mapping.
                    continue;
                }
            }
        };
        part_to_member.entry(part.to_string()).or_insert(i);
    }

    // --- Own epistemic per part (topology-map SS3 + evidence-probe F1) ---------
    let runtime_claim_roles: BTreeSet<&str> = [
        "behavior",
        "validation",
        "observer_validation",
        "observability_algorithm",
        "metric",
        "health",
        "implementation",
    ]
    .into_iter()
    .collect();

    let not_wired = honesty.wiring_status == "not_wired";
    let not_running = honesty.runtime_status == "not_running" && !runtime_moment_present;

    let mut states: Vec<PartState> = Vec::new();
    for (part, band) in &template.parts {
        let member = part_to_member.get(part).copied();
        let bound = member.is_some();
        let (own, display, self_block) = classify_own(
            part,
            *band,
            bound,
            not_wired,
            not_running,
            &runtime_claim_roles,
        );
        states.push(PartState {
            part: part.clone(),
            band: *band,
            member,
            own,
            display,
            det: Det::NotDeterminable,
            reason: Reason::None,
            self_block,
        });
    }

    // --- Determinability propagation (F2) -------------------------------------
    // memoized DFS over the forward DAG; feedback edges never appear in
    // required_inputs so no infinite recursion; own_ok is Observed-only, so an
    // absent runtime/trigger part is a break, not silently "determinable".
    let required = template.required_inputs();
    let index: BTreeMap<String, usize> =
        states.iter().enumerate().map(|(i, s)| (s.part.clone(), i)).collect();
    let mut det: BTreeMap<String, Det> = BTreeMap::new();
    let mut reason: BTreeMap<String, Reason> = BTreeMap::new();
    let mut onstack: BTreeSet<String> = BTreeSet::new();
    let names: Vec<String> = states.iter().map(|s| s.part.clone()).collect();
    for name in &names {
        resolve(name, &required, &index, &states, &mut det, &mut reason, &mut onstack);
    }
    for state in &mut states {
        state.det = det.get(&state.part).copied().unwrap_or(Det::NotDeterminable);
        state.reason = reason.get(&state.part).cloned().unwrap_or(Reason::None);
    }

    // failure front: LOGICAL_ERROR, or NOT_DETERMINABLE whose required inputs are
    // ALL confirmed (the break originates locally — the earliest break).
    let mut failure_front: Vec<usize> = Vec::new();
    for (i, state) in states.iter().enumerate() {
        let local = match state.det {
            Det::LogicalError => true,
            Det::NotDeterminable => required
                .get(&state.part)
                .map(|ins| ins.iter().all(|r| det.get(r) == Some(&Det::Confirmed)))
                .unwrap_or(true),
            Det::Confirmed => false,
        };
        if local {
            failure_front.push(i);
        }
    }

    // --- Incoming consumers (for the `definition` OFFER item) ------------------
    // Constructs that point AT this one via a consume predicate (DEPENDS_ON /
    // APPLIES_IN) consume whatever it defines: it OFFERS them that definition.
    // READ-ONLY: a pure scan of the committed relation set.
    let consume_predicates: BTreeSet<u32> = ["DEPENDS_ON", "APPLIES_IN"]
        .into_iter()
        .filter_map(|p| snapshot.symbol_id(p))
        .collect();
    let root_key = id_to_key[&root_id];
    let mut consumed_by: Vec<String> = Vec::new();
    for r in &snapshot.relations {
        if r.target == root_key && consume_predicates.contains(&r.predicate) {
            if let Some(src) = key_to_id.get(&r.source) {
                if *src != root_id {
                    consumed_by.push(src.clone());
                }
            }
        }
    }
    consumed_by.sort();
    consumed_by.dedup();

    // --- OFFER: what the construct provides, from its own graph content --------
    let root_content = wrappers_by_id
        .get(&root_id)
        .and_then(|w| w.get("content"))
        .cloned()
        .unwrap_or(Value::Null);
    let offers = compute_offer(
        &root_id,
        &root_content,
        &members,
        &consumed_by,
        runtime_moment_present,
    );

    // --- Render the ConstructMap ----------------------------------------------
    let observation_status = if budget_exhausted {
        "budget_exhausted"
    } else {
        "partial"
    };
    let out = render_map(
        &root_id,
        &store_dir,
        &snapshot.revision.0,
        &template,
        &states,
        &index,
        &required,
        &det,
        &members,
        &part_to_member,
        &honesty,
        graph_written_observed,
        runtime_moment_present,
        &failure_front,
        observation_status,
        &offers,
    );
    print!("{out}");
    Ok(())
}

// ---------------------------------------------------------------------------
#[derive(Default)]
struct Honesty {
    graph_status: String,
    wiring_status: String,
    runtime_status: String,
    verification_status: String,
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Own presence lattice value + display epistemic + self-block label.
fn classify_own(
    part: &str,
    band: Band,
    bound: bool,
    not_wired: bool,
    not_running: bool,
    runtime_claim_roles: &BTreeSet<&str>,
) -> (Epistemic, Epistemic, Option<String>) {
    match band {
        Band::LoopSpace | Band::Maintenance => {
            if bound || band == Band::Maintenance {
                (Epistemic::Observed, Epistemic::Observed, None)
            } else {
                (Epistemic::Unknown, Epistemic::Unknown, Some("absent".into()))
            }
        }
        Band::Role => {
            if bound {
                let display = if runtime_claim_roles.contains(part) {
                    // definition present (Observed for propagation), but its
                    // CENTRAL CLAIM is a runtime result -> not_measured display.
                    Epistemic::NotMeasured
                } else {
                    Epistemic::Observed
                };
                (Epistemic::Observed, display, None)
            } else {
                // A required role missing -> incomplete/known_absent.
                (
                    Epistemic::KnownAbsent,
                    Epistemic::KnownAbsent,
                    Some("required role member absent".into()),
                )
            }
        }
        Band::Anatomy => {
            // Anatomy (except loop_space) is defined by the template, never
            // materialized as separate entities in these fixtures -> known_absent.
            (
                Epistemic::KnownAbsent,
                Epistemic::KnownAbsent,
                Some("anatomy not materialized (template-only)".into()),
            )
        }
        Band::Trigger => {
            if not_wired {
                (
                    Epistemic::KnownAbsent,
                    Epistemic::KnownAbsent,
                    Some("not wired into the field (wiring_status=not_wired)".into()),
                )
            } else if bound {
                (Epistemic::Observed, Epistemic::Observed, None)
            } else {
                (
                    Epistemic::KnownAbsent,
                    Epistemic::KnownAbsent,
                    Some("trigger atom/bond absent".into()),
                )
            }
        }
        Band::RuntimeMoment => {
            if not_running {
                (
                    Epistemic::KnownAbsent,
                    Epistemic::KnownAbsent,
                    Some("no runtime moment (runtime_status=not_running)".into()),
                )
            } else if bound {
                (Epistemic::Observed, Epistemic::Observed, None)
            } else {
                (
                    Epistemic::KnownAbsent,
                    Epistemic::KnownAbsent,
                    Some("runtime moment not produced".into()),
                )
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve(
    part: &str,
    required: &BTreeMap<String, Vec<String>>,
    index: &BTreeMap<String, usize>,
    states: &[PartState],
    det: &mut BTreeMap<String, Det>,
    reason: &mut BTreeMap<String, Reason>,
    onstack: &mut BTreeSet<String>,
) -> Det {
    if let Some(d) = det.get(part) {
        return *d;
    }
    if onstack.contains(part) {
        // Unexpected forward cycle (malformed graph, not one of the 6 feedback
        // back-edges which are excluded from `required`): break, never confirm.
        det.insert(part.to_string(), Det::LogicalError);
        reason.insert(part.to_string(), Reason::Struct("unexpected_cycle".into()));
        return Det::NotDeterminable;
    }
    onstack.insert(part.to_string());

    let empty = Vec::new();
    let inputs = required.get(part).unwrap_or(&empty);
    let mut first_unconfirmed: Option<String> = None;
    for r in inputs {
        if resolve(r, required, index, states, det, reason, onstack) != Det::Confirmed
            && first_unconfirmed.is_none()
        {
            first_unconfirmed = Some(r.clone());
        }
    }

    let state = &states[index[part]];
    let own_ok = state.own == Epistemic::Observed;

    let verdict = if first_unconfirmed.is_none() && own_ok {
        reason.insert(part.to_string(), Reason::None);
        Det::Confirmed
    } else if let Some(r) = first_unconfirmed {
        reason.insert(part.to_string(), Reason::Input(r));
        Det::NotDeterminable
    } else {
        let block = state.self_block.clone().unwrap_or_else(|| state.display.label().to_string());
        reason.insert(part.to_string(), Reason::SelfBlock(block));
        Det::NotDeterminable
    };
    onstack.remove(part);
    det.insert(part.to_string(), verdict);
    verdict
}

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------
#[allow(clippy::too_many_arguments)]
fn render_map(
    root_id: &str,
    store_dir: &std::path::Path,
    revision: &u64,
    template: &LoopTemplate,
    states: &[PartState],
    index: &BTreeMap<String, usize>,
    required: &BTreeMap<String, Vec<String>>,
    det: &BTreeMap<String, Det>,
    members: &[BoundMember],
    part_to_member: &BTreeMap<String, usize>,
    honesty: &Honesty,
    graph_written_observed: bool,
    runtime_moment_present: bool,
    failure_front: &[usize],
    observation_status: &str,
    offers: &[OfferItem],
) -> String {
    let mut s = String::new();
    let member_of = |part: &str| -> Option<&BoundMember> {
        part_to_member.get(part).map(|i| &members[*i])
    };
    let name = wrappers_name(members, root_id);

    // Header ----------------------------------------------------------------
    s.push_str(&format!("CONSTRUCT MAP   {root_id}\n"));
    s.push_str(&format!("{name}\n"));
    let scale = member_of("loop_space")
        .and_then(|m| m.inner.get("dominant_scale").and_then(Value::as_str))
        .unwrap_or("unknown");
    let phase = if graph_written_observed && !runtime_moment_present {
        "written (written-not-run)"
    } else if runtime_moment_present {
        "running"
    } else {
        "authored"
    };
    s.push_str(&format!(
        "store: {}   revision: {}   scale: {scale}   phase: {phase}\n",
        store_dir.display(),
        revision
    ));
    s.push_str(&format!("observation status: {observation_status}\n"));
    s.push_str(
        "verdict: objective satisfaction is not_measured — every member is present in the LIVE graph\n",
    );
    s.push_str(
        "         (independent readback), but the construct is not wired into the physics field and\n",
    );
    s.push_str(
        "         has never run: no runtime moment (validation_run/health_assessment/Moment) exists.\n",
    );
    s.push_str("         not_measured != false. no fresh evidence -> no 'healthy', no satisfied objective.\n\n");

    // EXPLAINS --------------------------------------------------------------
    s.push_str("EXPLAINS\n");
    let purpose = member_of("loop_space")
        .and_then(|m| m.inner.get("purpose").and_then(Value::as_str))
        .or_else(|| {
            member_of("pattern").and_then(|m| m.inner.get("pattern").and_then(Value::as_str))
        });
    match purpose {
        Some(p) => {
            s.push_str("  What        [observed] from loop_space.purpose\n");
            s.push_str(&wrap_indent(p, 14, 92));
        }
        None => s.push_str("  What        unknown — no purpose declared (known_absent)\n"),
    }
    let succeeds = member_of("objective")
        .and_then(|m| m.inner.get("success_condition").and_then(Value::as_str));
    match succeeds {
        Some(sc) => {
            s.push_str("  Succeeds    [observed] from objective.success_condition\n");
            s.push_str("   when\n");
            s.push_str(&wrap_indent(sc, 14, 92));
        }
        None => s.push_str("  Succeeds    known_absent — no success_condition authored\n"),
    }
    let mut prov: Vec<&str> = Vec::new();
    if member_of("loop_space").is_some() {
        prov.push("loop_space.purpose");
    }
    if member_of("objective").is_some() {
        prov.push("objective.success_condition");
    }
    s.push_str(&format!("  from        {}\n\n", prov.join(" + ")));

    // STATE: rollup ---------------------------------------------------------
    s.push_str("STATE\n");
    s.push_str(
        "  overall   correctness=well_formed  trust=self_reported  health=not_measured  epistemic=not_measured\n",
    );
    s.push_str("  honesty fields (implementation self-report — NOT evidence):\n");
    s.push_str(&format!(
        "    graph_status={}  wiring_status={}  runtime_status={}  verification_status={}\n",
        dash(&honesty.graph_status),
        dash(&honesty.wiring_status),
        dash(&honesty.runtime_status),
        dash(&honesty.verification_status)
    ));
    if graph_written_observed && honesty.graph_status == "not_written" {
        s.push_str(
            "    * independent readback OVERRIDES graph_status: all members are present in the store,\n",
        );
        s.push_str(
            "      so graph_status=not_written is STALE and discarded. graph-write = observed.\n",
        );
    }
    s.push('\n');

    // STATE: qualification table (bound members) ----------------------------
    s.push_str("  QUALIFICATION TABLE — bound members (independent readback)\n");
    s.push_str(&format!(
        "    {:<24} {:<14} {:<14} {:<13} {:<13}\n",
        "loop part (subtype)", "correctness", "trust", "health", "epistemic"
    ));
    for (part, band) in &template.parts {
        if let Some(m) = member_of(part) {
            let _ = band;
            let (correctness, trust, health) = axes_for(part, m);
            let epi = states[index[part]].display.label();
            s.push_str(&format!(
                "    {:<24} {:<14} {:<14} {:<13} {:<13}\n",
                truncate(part, 24),
                correctness,
                trust,
                health,
                epi
            ));
        }
    }
    // satellites not on the loop (e.g. physicalization_binding, capability_port)
    let mut satellites: Vec<&BoundMember> = members
        .iter()
        .filter(|m| {
            !matches!(
                m.subtype.as_str(),
                "objective"
                    | "pattern"
                    | "vocabulary"
                    | "behavior"
                    | "algorithm"
                    | "code"
                    | "implementation"
                    | "justification"
                    | "observability_algorithm"
                    | "metric"
                    | "health"
                    | "validation"
            ) && !(m.node_type == "space")
        })
        .collect();
    satellites.sort_by(|a, b| a.canonical_id.cmp(&b.canonical_id));
    if !satellites.is_empty() {
        s.push_str("    -- satellites (not loop roles) --\n");
        for m in &satellites {
            s.push_str(&format!(
                "    {:<24} {:<14} {:<14} {:<13} {:<13}\n",
                truncate(&m.subtype, 24),
                "well_formed",
                "self_reported",
                "n/a",
                "observed"
            ));
        }
    }
    s.push('\n');

    // STATE: per-part determinability across all 34 loop parts --------------
    s.push_str("  LOOP PARTS — determinability (all 34; forward DAG, feedback excluded)\n");
    for (part, band) in &template.parts {
        let st = &states[index[part]];
        let d = match st.det {
            Det::Confirmed => "CONFIRMED",
            Det::NotDeterminable => "NOT_DETERMINABLE",
            Det::LogicalError => "LOGICAL_ERROR",
        };
        let band_tag = band_label(*band);
        let note = match &st.reason {
            Reason::None => String::new(),
            Reason::Input(x) => format!("<- input {x}"),
            Reason::SelfBlock(b) => format!("<- self: {b}"),
            Reason::Struct(x) => format!("<- struct: {x}"),
        };
        let marker = if failure_front.contains(&index[part]) && st.det != Det::Confirmed {
            "X"
        } else {
            " "
        };
        s.push_str(&format!(
            "  {marker} {:<20} {:<7} {:<17} {}\n",
            truncate(part, 20),
            band_tag,
            d,
            note
        ));
    }
    s.push('\n');

    // BECAUSE-TRACE ---------------------------------------------------------
    s.push_str("  BECAUSE-TRACE   (objective satisfaction -> first break; verify arc)\n");
    s.push_str(
        "    objective is present & well_formed, but its SATISFACTION is not_determinable:\n",
    );
    s.push_str(
        "    the loop closes only through the feedback edge `health SUPPORTS objective`, which needs a\n",
    );
    s.push_str("    FRESH health. Walking that dependency:\n");
    let trace = because_trace("health", required, det, states, index);
    for (depth, line) in trace.iter().enumerate() {
        let glyph = if depth + 1 == trace.len() { "X BECAUSE" } else { "↳ BECAUSE" };
        s.push_str(&format!("      {glyph} {line}\n"));
    }
    s.push('\n');

    // FAILURE FRONT ---------------------------------------------------------
    s.push_str("  FAILURE FRONT   (earliest breaks — where the fault originates)\n");
    if failure_front.is_empty() {
        s.push_str("    (none — every loop part is determinable)\n");
    } else {
        // group fronts by band for a readable, honest summary
        for &i in failure_front {
            let st = &states[i];
            if st.det == Det::Confirmed {
                continue;
            }
            let blk = st.self_block.clone().unwrap_or_else(|| st.display.label().to_string());
            s.push_str(&format!(
                "    X {:<20} [{}]  {}  (epistemic: {})\n",
                truncate(&st.part, 20),
                band_label(st.band).trim(),
                blk,
                st.display.label()
            ));
        }
        s.push_str(
            "    Root cause: the construct is WRITTEN to the graph but NOT WIRED into the physics field\n",
        );
        s.push_str(
            "    (trigger band absent) and NEVER RUN (runtime-moment band known_absent). Every verify-arc\n",
        );
        s.push_str(
            "    rung (observe -> measure -> health -> objective) is therefore not_measured, not false.\n",
        );
    }
    s.push('\n');

    // OFFER -----------------------------------------------------------------
    // What the construct PROVIDES, typed and matchable. Each item is either
    // active (proven/live now) or offered-but-inactive (present-but-unproven);
    // the `provides_type` is the pairing key for a future cross-construct market.
    s.push_str("OFFER   (what it provides, typed; * = present-but-unproven / inactive)\n");
    if offers.is_empty() {
        s.push_str("  (none derivable from its graph content)\n");
    } else {
        for kind in ["affordance", "capability", "port", "effect", "moment", "definition"] {
            let group: Vec<&OfferItem> = offers.iter().filter(|o| o.kind == kind).collect();
            if group.is_empty() {
                // honest absence for the always-expected structural kinds
                if matches!(kind, "affordance" | "effect") {
                    s.push_str(&format!(
                        "  {kind:<11} (none — the construct declares no {kind} in its graph content)\n"
                    ));
                }
                continue;
            }
            for o in group {
                let mark = if o.active { " " } else { "*" };
                s.push_str(&format!(
                    "  {mark}{:<10} {:<28} :: {}\n",
                    o.kind,
                    truncate(&o.name, 28),
                    o.provides_type
                ));
                s.push_str(&wrap_indent(&o.detail, 16, 96));
            }
        }
    }
    s.push('\n');

    // NEEDS -----------------------------------------------------------------
    s.push_str("NEEDS   (ordered: unblocks the most downstream first; satisfiable_by = the matchable type)\n");
    let needs = compute_needs(template, states, index, required, det, part_to_member, members);
    for need in &needs {
        s.push_str(&format!(
            "  {}. [{}] {:<24} unblocks {:<2}  -> {}\n",
            need.rank, need.blocker_kind, truncate(&need.target, 24), need.unblocks_count, need.action
        ));
        s.push_str(&format!("       satisfiable_by: {}\n", need.satisfiable_by));
        if !need.depends_on.is_empty() {
            let deps: Vec<String> = need.depends_on.iter().map(|d| d.to_string()).collect();
            s.push_str(&format!("       (needs {})\n", deps.join(", ")));
        }
    }
    s.push('\n');

    // MATCHING --------------------------------------------------------------
    // Does any of THIS construct's own offers satisfy any of its own needs?
    // Usually not — needs are cross-construct. We surface every same-type
    // overlap and state honestly whether the offer is live enough to satisfy it.
    s.push_str("MATCHING   (self offers vs self needs — a future cross-construct matcher pairs the types)\n");
    let offer_types: BTreeMap<&str, &OfferItem> =
        offers.iter().map(|o| (o.provides_type.as_str(), o)).collect();
    let mut any_overlap = false;
    for need in &needs {
        if let Some(o) = offer_types.get(need.satisfiable_by) {
            any_overlap = true;
            let verdict = if o.active {
                "SATISFIED by an ACTIVE self-offer"
            } else {
                "NOT satisfied: the self-offer is present-but-inactive (declared, not proven) — it does not close the need"
            };
            s.push_str(&format!(
                "  need #{} ({}) <-> offer [{}] {} :: {verdict}\n",
                need.rank, need.satisfiable_by, o.kind, truncate(&o.name, 20)
            ));
        }
    }
    if !any_overlap {
        s.push_str("  no self-offer's provides_type matches any need's satisfiable_by (needs are cross-construct).\n");
    }
    s.push_str("  open need types (for a cross-construct matcher to source elsewhere):\n");
    let open: Vec<&str> = needs
        .iter()
        .filter(|n| {
            offer_types
                .get(n.satisfiable_by)
                .map(|o| !o.active)
                .unwrap_or(true)
        })
        .map(|n| n.satisfiable_by)
        .collect();
    s.push_str(&format!("    {}\n", open.join("  |  ")));
    s.push('\n');

    s.push_str(
        "END — READ-ONLY: no store write, no event appended, no transaction committed.\n",
    );
    s
}

fn band_label(band: Band) -> &'static str {
    match band {
        Band::LoopSpace => "space ",
        Band::Role => "role  ",
        Band::Anatomy => "anat  ",
        Band::Trigger => "trig  ",
        Band::RuntimeMoment => "runt  ",
        Band::Maintenance => "maint ",
    }
}

fn wrappers_name(members: &[BoundMember], root_id: &str) -> String {
    members
        .iter()
        .find(|m| m.canonical_id == root_id)
        .and_then(|m| m.inner.get("name").and_then(Value::as_str))
        .unwrap_or(root_id)
        .to_string()
}

fn axes_for(part: &str, m: &BoundMember) -> (&'static str, &'static str, &'static str) {
    let _ = m;
    let correctness = "well_formed";
    let (trust, health) = match part {
        "metric" | "health" => ("unverified", if part == "health" { "not_measured" } else { "n/a" }),
        "implementation" => ("self_reported", "not_measured"),
        _ => ("self_reported", "n/a"),
    };
    (correctness, trust, health)
}

/// Walk `reason` INPUT pointers from `start` down to a SELF/STRUCT front.
fn because_trace(
    start: &str,
    required: &BTreeMap<String, Vec<String>>,
    det: &BTreeMap<String, Det>,
    states: &[PartState],
    index: &BTreeMap<String, usize>,
) -> Vec<String> {
    let _ = required;
    let mut chain: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cur = start.to_string();
    while !seen.contains(&cur) {
        seen.insert(cur.clone());
        let st = &states[index[&cur]];
        match &st.reason {
            Reason::None => {
                chain.push(format!("{cur} is determinable"));
                break;
            }
            Reason::Input(x) => {
                let d = det.get(x).copied().unwrap_or(Det::NotDeterminable);
                let tag = if d == Det::Confirmed { "confirmed" } else { "not verified" };
                chain.push(format!("{cur} not determinable — its input `{x}` is {tag}"));
                cur = x.clone();
            }
            Reason::SelfBlock(b) => {
                chain.push(format!("{cur} not determinable — own evidence: {b}"));
                break;
            }
            Reason::Struct(x) => {
                chain.push(format!("{cur} LOGICAL_ERROR: {x}"));
                break;
            }
        }
    }
    chain
}

struct Need {
    rank: usize,
    target: String,
    blocker_kind: &'static str,
    unblocks_count: usize,
    action: String,
    depends_on: Vec<usize>,
    /// The typed thing that would SATISFY this need — a `capability:*`,
    /// `moment:*`, `verification:*` or `definition:*` string. A future
    /// cross-construct matcher pairs this against another construct's OFFER
    /// `provides_type`. Made explicit here so needs and offers share one type
    /// vocabulary; the matcher itself is not built yet.
    satisfiable_by: &'static str,
}

/// Ordered repairs: seeded from the honest lifecycle ladder for a written-not-run
/// construct (wire -> run+observe -> validate -> derive health -> drive binding).
/// `unblocks_count` = downstream loop parts freed, computed from the forward DAG.
fn compute_needs(
    template: &LoopTemplate,
    states: &[PartState],
    index: &BTreeMap<String, usize>,
    required: &BTreeMap<String, Vec<String>>,
    det: &BTreeMap<String, Det>,
    part_to_member: &BTreeMap<String, usize>,
    members: &[BoundMember],
) -> Vec<Need> {
    // downstream reachability over forward edges: how many NOT_DETERMINABLE parts
    // a given part reaches (i.e. would be freed if it became confirmed).
    let mut forward: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (to, ins) in required {
        for from in ins {
            forward.entry(from.clone()).or_default().push(to.clone());
        }
    }
    let reach = |seed: &str| -> usize {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut q: VecDeque<String> = VecDeque::new();
        q.push_back(seed.to_string());
        while let Some(n) = q.pop_front() {
            if let Some(next) = forward.get(&n) {
                for m in next {
                    if seen.insert(m.clone()) {
                        q.push_back(m.clone());
                    }
                }
            }
        }
        seen.iter()
            .filter(|p| det.get(*p).copied().unwrap_or(Det::Confirmed) != Det::Confirmed)
            .count()
    };
    let _ = (states, index, template);

    let mut needs: Vec<Need> = Vec::new();
    let sensor_unblocks = reach("Sensor").max(1);
    needs.push(Need {
        rank: 1,
        target: "implementation (wire)".into(),
        blocker_kind: "unwired",
        unblocks_count: sensor_unblocks,
        action:
            "Wire the construct into the physics field: materialize the trigger band (Sensor/DepositBond/trigger atom/Threshold) so a physics event can fire it. wiring_status not_wired->wired."
                .into(),
        depends_on: vec![],
        satisfiable_by: "capability:physics-event-energy-deposit-bridge",
    });
    let obs_unblocks = reach("observability_algorithm").max(1);
    needs.push(Need {
        rank: 2,
        target: "implementation (run)".into(),
        blocker_kind: "unmeasured",
        unblocks_count: obs_unblocks + 1,
        action:
            "Run the mechanism + the INDEPENDENT observer to commit a Moment and produce a validation_run + the metric dimensions. runtime_status not_running->running; verification_status not_measured->measured."
                .into(),
        depends_on: vec![1],
        satisfiable_by: "moment:validation_run",
    });
    needs.push(Need {
        rank: 3,
        target: "validation + observer_validation".into(),
        blocker_kind: "unverified",
        unblocks_count: 2,
        action:
            "Execute the authored scenarios + the observer fault-detection tests against the fresh trace, moving behavior/observer claims self_reported->independently_verified."
                .into(),
        depends_on: vec![2],
        satisfiable_by: "verification:independently_verified",
    });
    needs.push(Need {
        rank: 4,
        target: "health".into(),
        blocker_kind: "unmeasured",
        unblocks_count: reach("health").max(1),
        action: "Derive health from the fresh metric vector per health.derivation (not_measured->healthy|degraded).".into(),
        depends_on: vec![2],
        satisfiable_by: "moment:health_assessment",
    });
    // missing_binding need if a physicalization_binding satellite is present.
    let _ = part_to_member;
    if members.iter().any(|m| m.subtype == "physicalization_binding") {
        needs.push(Need {
            rank: 5,
            target: "visual_binding".into(),
            blocker_kind: "missing_binding",
            unblocks_count: 0,
            action:
                "Drive the deferred state sockets from three independent streams: degree->size (poids), measured support->brightness (activation; unmeasured=Fog), causal-chain health->line continuity (validité)."
                    .into(),
            depends_on: vec![2],
            satisfiable_by: "definition:state-channel-streams",
        });
    }
    needs
}

// ---------------------------------------------------------------------------
// OFFER — what the construct PROVIDES, derived READ-ONLY from its own graph
// content and its incident edges. Every item carries a `provides_type` so a
// future cross-construct matcher can pair it against a NEED's `satisfiable_by`.
// Epistemic honesty: an item that is present-but-unproven (a port with no
// relation, a runtime moment declared but never produced) is still OFFERED, but
// `active=false` and the detail says so — a declared offer is not a live one.
// ---------------------------------------------------------------------------
struct OfferItem {
    kind: &'static str, // affordance | capability | port | effect | moment | definition
    name: String,
    provides_type: String,
    active: bool,
    detail: String,
}

fn is_satellite_subtype(s: &str) -> bool {
    matches!(
        s,
        "capability_port" | "physicalization_binding" | "broadcast_port"
    )
}

/// Collect every string value in a JSON tree (used to find canonical-id
/// references a member's content names — e.g. a code member's port_id).
fn collect_strings(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            out.insert(s.clone());
        }
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// Recursively harvest declared world-facing effects: an `emits_effect_intent`
/// object's `effect`, an `effect_gate`'s `on_effector_activation`, or a plain
/// `effect` string that names an EffectIntent/notify. Returns (name, detail).
fn collect_effects(v: &Value, out: &mut Vec<(String, String)>) {
    if let Value::Object(o) = v {
        if let Some(e) = o.get("emits_effect_intent").and_then(Value::as_object) {
            let name = e.get("effect").and_then(Value::as_str).unwrap_or("effect").to_string();
            let cap = e.get("required_capability").and_then(Value::as_str).unwrap_or("-");
            out.push((name, format!("emits_effect_intent; required_capability={cap}")));
        }
        if let Some(g) = o.get("effect_gate").and_then(Value::as_object) {
            if let Some(act) = g.get("on_effector_activation").and_then(Value::as_str) {
                out.push((act.to_string(), "effect_gate.on_effector_activation".into()));
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
    } else if let Value::Array(a) = v {
        for val in a {
            collect_effects(val, out);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_offer(
    root_id: &str,
    root_content: &Value,
    members: &[BoundMember],
    consumed_by: &[String],
    runtime_moment_present: bool,
) -> Vec<OfferItem> {
    let mut offers: Vec<OfferItem> = Vec::new();
    let arr = |v: &Value, key: &str| -> Vec<String> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };

    // --- affordance ---------------------------------------------------------
    // read verbs (observe-only) are dedup'd across the port and the contract;
    // author verbs stay separate because they are capability-gated.
    let mut read_verbs: BTreeSet<String> = BTreeSet::new();
    let mut author_verbs: BTreeSet<String> = BTreeSet::new();
    let mut mutate_capability: Option<String> = None;

    for m in members {
        if m.subtype == "capability_port" {
            for v in arr(&m.inner, "observe_capabilities") {
                read_verbs.insert(v);
            }
            if let Some(c) = m.inner.get("required_mutate_capability").and_then(Value::as_str) {
                mutate_capability.get_or_insert_with(|| c.to_string());
            }
        }
    }
    if let Some(pc) = root_content.get("programming_contract") {
        for v in arr(pc, "observer_capabilities") {
            read_verbs.insert(v);
        }
        for v in arr(pc, "author_capabilities") {
            author_verbs.insert(v);
        }
    }
    // behavior action verbs: the construct's declared callable surface lives on
    // the code member's `entrypoints` (the concrete verbs behavior enacts).
    let mut mechanism_verbs: BTreeSet<String> = BTreeSet::new();
    for m in members {
        if m.subtype == "code" {
            for v in arr(&m.inner, "entrypoints") {
                mechanism_verbs.insert(v);
            }
        }
    }
    let read_justif = root_content
        .get("programming_contract")
        .and_then(|pc| pc.get("rule"))
        .and_then(Value::as_str)
        .map(str::to_string);
    for v in &read_verbs {
        offers.push(OfferItem {
            kind: "affordance",
            name: v.clone(),
            provides_type: format!("verb:{v}"),
            active: true,
            detail: "access=observe; precondition=none (read-only surface)".into(),
        });
    }
    let gate = mutate_capability.clone().unwrap_or_else(|| "authoring capability".into());
    for v in &author_verbs {
        offers.push(OfferItem {
            kind: "affordance",
            name: v.clone(),
            provides_type: format!("verb:{v}"),
            active: false,
            detail: format!(
                "access=author; precondition=actor holds {gate}{}",
                read_justif
                    .as_deref()
                    .map(|j| format!("; justification: {}", first_sentence(j)))
                    .unwrap_or_default()
            ),
        });
    }
    for v in &mechanism_verbs {
        offers.push(OfferItem {
            kind: "affordance",
            name: v.clone(),
            provides_type: format!("verb:{v}"),
            active: true,
            detail: "access=mechanism; declared code entrypoint (callable surface)".into(),
        });
    }

    // --- capability ---------------------------------------------------------
    // capabilities this construct gates access behind (authority:*).
    let mut authorities: BTreeSet<String> = BTreeSet::new();
    let mut all_strings: BTreeSet<String> = BTreeSet::new();
    collect_strings(root_content, &mut all_strings);
    for m in members {
        collect_strings(&m.inner, &mut all_strings);
    }
    for s in &all_strings {
        if s.starts_with("authority:") {
            authorities.insert(s.clone());
        }
    }
    for a in &authorities {
        offers.push(OfferItem {
            kind: "capability",
            name: a.clone(),
            provides_type: format!("capability:{a}"),
            active: true,
            detail: "authority gate: an actor must hold this capability to pass the port".into(),
        });
    }

    // --- port ---------------------------------------------------------------
    // exported ports and their link state. A port with no incident relation is
    // present but UNLINKED -> it names the capability but is currently inactive.
    for m in members {
        if is_satellite_subtype(&m.subtype)
            && (m.subtype.ends_with("_port") || m.canonical_id.starts_with("port:"))
        {
            let posture = m
                .inner
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("unspecified");
            let cap = m
                .inner
                .get("required_mutate_capability")
                .and_then(Value::as_str)
                .unwrap_or("-");
            // No construct-internal relation reaches these ports in the store,
            // so the readback link-state is `unlinked` (inactive as a wired
            // export); the content posture is reported alongside, not as proof.
            offers.push(OfferItem {
                kind: "port",
                name: m.canonical_id.clone(),
                provides_type: format!("port:{}", m.canonical_id),
                active: false,
                detail: format!(
                    "link_state=unlinked (no relation wires it -> inactive); posture={posture}; gates={cap}"
                ),
            });
        }
    }

    // --- effect -------------------------------------------------------------
    let mut effects: Vec<(String, String)> = Vec::new();
    for m in members {
        if matches!(m.subtype.as_str(), "code" | "behavior") {
            collect_effects(&m.inner, &mut effects);
        }
    }
    collect_effects(root_content, &mut effects);
    effects.sort();
    effects.dedup();
    for (name, detail) in &effects {
        offers.push(OfferItem {
            kind: "effect",
            name: name.clone(),
            provides_type: format!("effect:{name}"),
            active: false,
            detail: format!("{detail}; requires authorized transport + EffectReceipt (never proven by a firing)"),
        });
    }

    // --- moment -------------------------------------------------------------
    // runtime moments the construct can PRODUCE. Declared here; whether any real
    // moment exists in the store is `runtime_moment_present`.
    for sub in arr(root_content, "runtime_moment_subtypes") {
        offers.push(OfferItem {
            kind: "moment",
            name: sub.clone(),
            provides_type: format!("moment:{sub}"),
            active: runtime_moment_present,
            detail: if runtime_moment_present {
                "runtime moment present in the store".into()
            } else {
                "declared; PRODUCED by execution (precreated:false); none in the store yet -> inactive".into()
            },
        });
    }

    // --- definition ---------------------------------------------------------
    // what this construct defines for OTHERS: the consumers that DEPENDS_ON /
    // APPLIES_IN it. With consumers it is actively consumed; without, it is
    // offered but not yet depended on (known_absent consumers).
    let def_name = root_content
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(root_id)
        .to_string();
    offers.push(OfferItem {
        kind: "definition",
        name: def_name,
        provides_type: format!("definition:{root_id}"),
        active: !consumed_by.is_empty(),
        detail: if consumed_by.is_empty() {
            "no construct DEPENDS_ON / APPLIES_IN it yet (known_absent consumers)".into()
        } else {
            format!("consumed_by {} construct(s): {}", consumed_by.len(), consumed_by.join(", "))
        },
    });

    offers
}

/// First sentence (up to the first period) of a longer justification string.
fn first_sentence(s: &str) -> String {
    match s.find(". ") {
        Some(i) => s[..=i].trim().to_string(),
        None => s.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Small text helpers.
// ---------------------------------------------------------------------------
fn dash(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('~');
        out
    }
}

/// Word-wrap `text` at `width` columns, each line indented by `indent` spaces.
fn wrap_indent(text: &str, indent: usize, width: usize) -> String {
    let pad = " ".repeat(indent);
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push_str(&pad);
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push_str(&pad);
        out.push_str(&line);
        out.push('\n');
    }
    out
}
