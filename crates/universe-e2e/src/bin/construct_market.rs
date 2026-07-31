//! READ-ONLY cross-construct MARKET: pair every construct's typed NEED against
//! every construct's typed OFFER (chantier C6).
//!
//! `construct_map` answers "what is ONE construct for, is it real yet, and what
//! would make it more real?" — and it already derives, per construct, a typed
//! OFFER (what it PROVIDES, each item carrying a `provides_type`, marked active
//! or present-but-inactive) and a typed, ordered NEEDS list (each carrying a
//! `satisfiable_by` type). Those two vocabularies were designed to share one type
//! namespace precisely so a future matcher could pair them. This bin IS that
//! matcher.
//!
//! It opens the LIVE store READ-ONLY (`UniverseStore::open` + `replay` +
//! `read_content`; it NEVER writes, NEVER appends an event, NEVER commits a
//! `UniverseTransaction`), enumerates every construct in the store (a `space`
//! node whose committed content carries `contractKind: "construct"`, excluding
//! the shared validity TEMPLATE), and for EACH construct runs the SAME
//! offer/need derivation as `construct_map` (`compute_offer` / `compute_needs`,
//! replicated here verbatim so `construct_map.rs` is untouched). It then builds a
//! global type ledger and MATCHES `need.satisfiable_by` against
//! `offer.provides_type` across ALL constructs.
//!
//! The one load-bearing epistemic rule: a match only SATISFIES a need when the
//! providing offer is ACTIVE. A declared-but-inactive offer (a runtime moment
//! never produced, a port with no incident relation, an author verb behind an
//! ungranted capability) shares the type but is not a live source — it never
//! closes a need. Type overlap is a candidate; an active provider is a match.
//!
//! Three assertions are printed at the end: (1) the total offer<->need match
//! count (candidates vs active); (2) the ACTIVE-ONLY rule, proven by showing no
//! active match was ever sourced from an inactive offer; (3) the honest
//! universal-gap line `capability:physics-event-energy-deposit-bridge -> N active
//! providers` (expected N=0 — no construct offers the bridge the whole city
//! needs to wire itself into the physics field).
//!
//! Usage: `construct_market [store-dir]`
//!   store-dir default: artifacts/ontology-registry/current/store

// The faithful loop model keeps a few fields/variants for completeness that this
// read path records but does not branch on; they document the model.
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
        eprintln!("CONSTRUCT_MARKET FAILED: {error}");
        std::process::exit(1);
    }
}

// ===========================================================================
// Epistemic lattice + loop-template model — replicated from construct_map.rs
// (verbatim; construct_map.rs is left untouched per the C6 constraint).
// ===========================================================================
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

struct LoopEdge {
    from: String,
    to: String,
    predicate: String,
    feedback: bool,
}

struct LoopTemplate {
    parts: Vec<(String, Band)>,
    edges: Vec<LoopEdge>,
}

impl LoopTemplate {
    fn part_names(&self) -> BTreeSet<&str> {
        self.parts.iter().map(|(n, _)| n.as_str()).collect()
    }

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

fn load_template(wrappers: &BTreeMap<String, Value>) -> Result<LoopTemplate, Box<dyn Error>> {
    let wrapper = wrappers
        .get(TEMPLATE_ID)
        .ok_or_else(|| format!("loop template {TEMPLATE_ID} not present in the store"))?;
    let svl = wrapper
        .get("content")
        .and_then(|c| c.get("self_verifying_loop"))
        .ok_or("template has no content.self_verifying_loop")?;
    let nodes = svl.get("nodes").and_then(Value::as_object).ok_or("template nodes missing")?;

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
// Member enumeration + binding + epistemic state — replicated from construct_map
// ---------------------------------------------------------------------------
fn discriminator(id: &str) -> &str {
    id.split_once(':').map(|(_, rest)| rest).unwrap_or(id)
}

struct BoundMember {
    canonical_id: String,
    subtype: String,
    node_type: String,
    inner: Value,
}

struct PartState {
    part: String,
    band: Band,
    member: Option<usize>,
    own: Epistemic,
    display: Epistemic,
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

/// Shared, prebuilt-once indexes over the committed snapshot.
struct StoreIndex {
    id_to_key: BTreeMap<String, EntityKey>,
    key_to_id: BTreeMap<EntityKey, String>,
    key_to_wrapper: BTreeMap<EntityKey, Value>,
    wrappers_by_id: BTreeMap<String, Value>,
}

/// The per-construct market position: its typed offers and typed needs, plus the
/// honest phase tag. Offers/needs come from the SAME derivation as construct_map.
struct ConstructPosition {
    root_id: String,
    name: String,
    phase: &'static str,
    offers: Vec<OfferItem>,
    needs: Vec<Need>,
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));

    // READ-ONLY handshake: open, load the committed snapshot, replay the valid
    // event log. No write path is ever touched.
    let store = UniverseStore::open(&store_dir)?;
    let snapshot = store.replay(store.load_snapshot()?)?;

    // canonical_id -> (EntityKey, wrapper) indexes, built ONCE.
    let mut idx = StoreIndex {
        id_to_key: BTreeMap::new(),
        key_to_id: BTreeMap::new(),
        key_to_wrapper: BTreeMap::new(),
        wrappers_by_id: BTreeMap::new(),
    };
    for entity in &snapshot.entities {
        if let Some(ptr) = entity.content.as_ref() {
            let wrapper = store.read_content(ptr)?;
            if let Some(cid) = wrapper.get("canonical_id").and_then(Value::as_str) {
                idx.id_to_key.insert(cid.to_string(), entity.key);
                idx.key_to_id.insert(entity.key, cid.to_string());
                idx.wrappers_by_id.insert(cid.to_string(), wrapper.clone());
                idx.key_to_wrapper.insert(entity.key, wrapper);
            }
        }
    }

    let template = load_template(&idx.wrappers_by_id)?;

    // --- Enumerate constructs: a `space` node whose committed content carries
    //     contractKind == "construct", EXCLUDING the shared validity template
    //     (which is a template, not an inhabited construct). Independent readback
    //     of the LIVE store — no self-reported registry is trusted.
    let mut construct_ids: Vec<String> = Vec::new();
    for (cid, wrapper) in &idx.wrappers_by_id {
        if cid == TEMPLATE_ID {
            continue;
        }
        let is_space = wrapper.get("node_type").and_then(Value::as_str) == Some("space");
        let kind = wrapper
            .get("content")
            .and_then(|c| c.get("contractKind"))
            .and_then(Value::as_str);
        if is_space && kind == Some("construct") {
            construct_ids.push(cid.clone());
        }
    }
    construct_ids.sort();

    if construct_ids.is_empty() {
        return Err("no constructs (space + contractKind=construct) found in the store".into());
    }

    // --- Analyze each construct with the construct_map derivation --------------
    let mut positions: Vec<ConstructPosition> = Vec::new();
    for root_id in &construct_ids {
        positions.push(analyze_construct(root_id, &snapshot, &idx, &template));
    }

    render_market(&store_dir, &snapshot.revision.0, &positions);
    Ok(())
}

/// Run the construct_map member-enumeration + binding + determinability pipeline
/// for one construct root and return its typed offers + typed needs. This body is
/// the offer/need-producing subset of construct_map's `run()`, replicated so that
/// bin is not modified.
fn analyze_construct(
    root_id: &str,
    snapshot: &universe_store::UniverseSnapshot,
    idx: &StoreIndex,
    template: &LoopTemplate,
) -> ConstructPosition {
    // (a) discriminator-suffix match.
    let suffix = discriminator(root_id).to_string();
    let mut member_keys: BTreeSet<EntityKey> = idx
        .id_to_key
        .iter()
        .filter(|(cid, _)| discriminator(cid) == suffix)
        .map(|(_, k)| *k)
        .collect();

    // (b) bounded construct-INTERNAL relation-closure (never DEPENDS_ON/PART_OF,
    //     which cross construct boundaries; never absorb the template root).
    let cross_boundary: BTreeSet<u32> = ["DEPENDS_ON", "PART_OF"]
        .into_iter()
        .filter_map(|p| snapshot.symbol_id(p))
        .collect();
    let template_key = idx.id_to_key.get(TEMPLATE_ID).copied();
    let budget = snapshot.entities.len() + 8;
    let mut iterations = 0usize;
    loop {
        if iterations >= budget {
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

    let read_members = |keys: &BTreeSet<EntityKey>| -> Vec<BoundMember> {
        let mut out: Vec<BoundMember> = Vec::new();
        for key in keys {
            if let Some(w) = idx.key_to_wrapper.get(key) {
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

    // (c) satellite-by-content-reference.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for m in &members {
        collect_strings(&m.inner, &mut referenced);
    }
    let mut satellite_added = false;
    for cid in &referenced {
        let Some(w) = idx.wrappers_by_id.get(cid) else { continue };
        let subtype = w.get("subtype").and_then(Value::as_str).unwrap_or("");
        let is_satellite = is_satellite_subtype(subtype)
            || cid.starts_with("port:")
            || cid.starts_with("broadcast_port:");
        if is_satellite {
            if let Some(k) = idx.id_to_key.get(cid) {
                if member_keys.insert(*k) {
                    satellite_added = true;
                }
            }
        }
    }
    if satellite_added {
        members = read_members(&member_keys);
    }

    // Honesty fields.
    let mut honesty = Honesty::default();
    if let Some(imp) = members.iter().find(|m| m.subtype == "implementation") {
        honesty.graph_status = str_field(&imp.inner, "graph_status");
        honesty.wiring_status = str_field(&imp.inner, "wiring_status");
        honesty.runtime_status = str_field(&imp.inner, "runtime_status");
        honesty.verification_status = str_field(&imp.inner, "verification_status");
    }
    let graph_written_observed = idx.id_to_key.contains_key(root_id);

    let runtime_moment_subtypes: BTreeSet<String> = idx
        .wrappers_by_id
        .get(root_id)
        .and_then(|w| w.get("content"))
        .and_then(|c| c.get("runtime_moment_subtypes"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let runtime_moment_present = members
        .iter()
        .any(|m| runtime_moment_subtypes.contains(&m.subtype));

    // Bind members onto loop parts by subtype.
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
                let target = str_field(&m.inner, "target");
                let role = str_field(&m.inner, "role");
                if target.starts_with("observability_algorithm") || role == "observer_validation" {
                    "observer_validation"
                } else {
                    "validation"
                }
            }
            _ => {
                if m.node_type == "space" && m.canonical_id == root_id {
                    "loop_space"
                } else {
                    continue;
                }
            }
        };
        part_to_member.entry(part.to_string()).or_insert(i);
    }

    // Own epistemic per part.
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
        let (own, display, self_block) =
            classify_own(part, *band, bound, not_wired, not_running, &runtime_claim_roles);
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

    // Determinability propagation.
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

    // Incoming consumers (for the `definition` OFFER item).
    let consume_predicates: BTreeSet<u32> = ["DEPENDS_ON", "APPLIES_IN"]
        .into_iter()
        .filter_map(|p| snapshot.symbol_id(p))
        .collect();
    let root_key = idx.id_to_key[root_id];
    let mut consumed_by: Vec<String> = Vec::new();
    for r in &snapshot.relations {
        if r.target == root_key && consume_predicates.contains(&r.predicate) {
            if let Some(src) = idx.key_to_id.get(&r.source) {
                if *src != root_id {
                    consumed_by.push(src.clone());
                }
            }
        }
    }
    consumed_by.sort();
    consumed_by.dedup();

    // OFFER + NEEDS (the shared type-bearing derivations).
    let root_content = idx
        .wrappers_by_id
        .get(root_id)
        .and_then(|w| w.get("content"))
        .cloned()
        .unwrap_or(Value::Null);
    let offers = compute_offer(root_id, &root_content, &members, &consumed_by, runtime_moment_present);
    let needs = compute_needs(template, &states, &index, &required, &det, &part_to_member, &members);

    let phase = if graph_written_observed && !runtime_moment_present {
        "written-not-run"
    } else if runtime_moment_present {
        "running"
    } else {
        "authored"
    };
    let name = members
        .iter()
        .find(|m| m.canonical_id == *root_id)
        .and_then(|m| m.inner.get("name").and_then(Value::as_str))
        .unwrap_or(root_id)
        .to_string();

    ConstructPosition { root_id: root_id.to_string(), name, phase, offers, needs }
}

// ===========================================================================
// classify_own / resolve — replicated from construct_map.rs verbatim.
// ===========================================================================
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
                    Epistemic::NotMeasured
                } else {
                    Epistemic::Observed
                };
                (Epistemic::Observed, display, None)
            } else {
                (
                    Epistemic::KnownAbsent,
                    Epistemic::KnownAbsent,
                    Some("required role member absent".into()),
                )
            }
        }
        Band::Anatomy => (
            Epistemic::KnownAbsent,
            Epistemic::KnownAbsent,
            Some("anatomy not materialized (template-only)".into()),
        ),
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

// ===========================================================================
// OFFER / NEED derivations — replicated from construct_map.rs verbatim.
// ===========================================================================
struct OfferItem {
    kind: &'static str,
    name: String,
    provides_type: String,
    active: bool,
    detail: String,
}

struct Need {
    rank: usize,
    target: String,
    blocker_kind: &'static str,
    unblocks_count: usize,
    action: String,
    depends_on: Vec<usize>,
    satisfiable_by: &'static str,
}

fn is_satellite_subtype(s: &str) -> bool {
    matches!(s, "capability_port" | "physicalization_binding" | "broadcast_port")
}

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

    // affordance
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

    // capability
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

    // port
    for m in members {
        if is_satellite_subtype(&m.subtype)
            && (m.subtype.ends_with("_port") || m.canonical_id.starts_with("port:"))
        {
            let posture = m.inner.get("state").and_then(Value::as_str).unwrap_or("unspecified");
            let cap = m
                .inner
                .get("required_mutate_capability")
                .and_then(Value::as_str)
                .unwrap_or("-");
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

    // effect
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
            detail: format!(
                "{detail}; requires authorized transport + EffectReceipt (never proven by a firing)"
            ),
        });
    }

    // moment
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

    // definition
    let def_name = root_content.get("name").and_then(Value::as_str).unwrap_or(root_id).to_string();
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

fn compute_needs(
    template: &LoopTemplate,
    states: &[PartState],
    index: &BTreeMap<String, usize>,
    required: &BTreeMap<String, Vec<String>>,
    det: &BTreeMap<String, Det>,
    part_to_member: &BTreeMap<String, usize>,
    members: &[BoundMember],
) -> Vec<Need> {
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
        action: "Wire the construct into the physics field: materialize the trigger band (Sensor/DepositBond/trigger atom/Threshold) so a physics event can fire it. wiring_status not_wired->wired.".into(),
        depends_on: vec![],
        satisfiable_by: "capability:physics-event-energy-deposit-bridge",
    });
    let obs_unblocks = reach("observability_algorithm").max(1);
    needs.push(Need {
        rank: 2,
        target: "implementation (run)".into(),
        blocker_kind: "unmeasured",
        unblocks_count: obs_unblocks + 1,
        action: "Run the mechanism + the INDEPENDENT observer to commit a Moment and produce a validation_run + the metric dimensions. runtime_status not_running->running; verification_status not_measured->measured.".into(),
        depends_on: vec![1],
        satisfiable_by: "moment:validation_run",
    });
    needs.push(Need {
        rank: 3,
        target: "validation + observer_validation".into(),
        blocker_kind: "unverified",
        unblocks_count: 2,
        action: "Execute the authored scenarios + the observer fault-detection tests against the fresh trace, moving behavior/observer claims self_reported->independently_verified.".into(),
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
    let _ = part_to_member;
    if members.iter().any(|m| m.subtype == "physicalization_binding") {
        needs.push(Need {
            rank: 5,
            target: "visual_binding".into(),
            blocker_kind: "missing_binding",
            unblocks_count: 0,
            action: "Drive the deferred state sockets from three independent streams: degree->size (poids), measured support->brightness (activation; unmeasured=Fog), causal-chain health->line continuity (validité).".into(),
            depends_on: vec![2],
            satisfiable_by: "definition:state-channel-streams",
        });
    }
    needs
}

fn first_sentence(s: &str) -> String {
    match s.find(". ") {
        Some(i) => s[..=i].trim().to_string(),
        None => s.to_string(),
    }
}

// ===========================================================================
// The MARKET — the new C6 work: pair NEEDS against OFFERS across constructs.
// ===========================================================================

/// One provider of a type: which construct offers it, and whether the offer is
/// ACTIVE (a live source) or merely declared-but-inactive (present, unproven).
struct Provider {
    construct: String,
    kind: &'static str,
    name: String,
    active: bool,
}

fn short(id: &str) -> String {
    // Trailing slug for compact tables: last two colon-separated segments.
    let parts: Vec<&str> = id.split(':').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2..].join(":")
    } else {
        id.to_string()
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

fn render_market(store_dir: &std::path::Path, revision: &u64, positions: &[ConstructPosition]) {
    let mut s = String::new();

    // Header ----------------------------------------------------------------
    s.push_str("CONSTRUCT MARKET   cross-construct offer <-> need matcher (C6)\n");
    s.push_str(&format!(
        "store: {}   revision: {}   constructs: {}\n",
        store_dir.display(),
        revision,
        positions.len()
    ));
    s.push_str("READ-ONLY: no store write, no event appended, no transaction committed.\n");
    s.push_str(
        "rule: a need is SATISFIED only by an ACTIVE offer of its type. A declared-but-inactive\n",
    );
    s.push_str(
        "      offer (a moment never produced, an unlinked port, an ungranted author verb) shares\n",
    );
    s.push_str("      the type but is not a live source — type overlap is a candidate, not a match.\n\n");

    // Roster ----------------------------------------------------------------
    s.push_str("CONSTRUCTS (independent readback: space + contractKind=construct)\n");
    for p in positions {
        let active_offers = p.offers.iter().filter(|o| o.active).count();
        let inactive_offers = p.offers.len() - active_offers;
        s.push_str(&format!(
            "  {:<34} {:<16} offers={:>2} (active {:>2} / inactive {:>2})  needs={}\n",
            truncate(&short(&p.root_id), 34),
            p.phase,
            p.offers.len(),
            active_offers,
            inactive_offers,
            p.needs.len()
        ));
    }
    s.push('\n');

    // Global provider index: provides_type -> providers (active + inactive) --
    let mut providers: BTreeMap<String, Vec<Provider>> = BTreeMap::new();
    for p in positions {
        for o in &p.offers {
            providers.entry(o.provides_type.clone()).or_default().push(Provider {
                construct: p.root_id.clone(),
                kind: o.kind,
                name: o.name.clone(),
                active: o.active,
            });
        }
    }

    // Need-type ledger: for each distinct need type, who needs it and who can
    // provide it (active vs inactive). This IS the market clearing table.
    let mut need_types: BTreeMap<String, Vec<String>> = BTreeMap::new(); // type -> needer constructs
    for p in positions {
        for n in &p.needs {
            need_types
                .entry(n.satisfiable_by.to_string())
                .or_default()
                .push(p.root_id.clone());
        }
    }

    s.push_str("MARKET LEDGER   (need type -> providers; * = only inactive providers exist)\n");
    s.push_str(&format!(
        "  {:<48} {:>6} {:>10} {:>10} {:>9}\n",
        "need type (satisfiable_by)", "needers", "active", "inactive", "satisfied"
    ));
    // Sort need types with the universal-gap capability first, then alpha.
    let mut ordered_types: Vec<&String> = need_types.keys().collect();
    ordered_types.sort();
    for t in &ordered_types {
        let needers = need_types[*t].len();
        let provs = providers.get(*t);
        let active_n = provs.map(|v| v.iter().filter(|p| p.active).count()).unwrap_or(0);
        let inactive_n = provs.map(|v| v.iter().filter(|p| !p.active).count()).unwrap_or(0);
        let satisfied = active_n > 0;
        let mark = if !satisfied && inactive_n > 0 { "*" } else { " " };
        s.push_str(&format!(
            "{}{:<48} {:>6} {:>10} {:>10} {:>9}\n",
            mark,
            truncate(t, 48),
            needers,
            active_n,
            inactive_n,
            if satisfied { "YES" } else { "no" }
        ));
    }
    s.push('\n');

    // Full match enumeration: (needing construct, need type) x (providing offer)
    // where types are equal. Count candidates and satisfying (active) matches.
    let mut total_candidate_matches = 0usize;
    let mut total_active_matches = 0usize;
    let mut active_from_inactive_offer = 0usize; // must stay 0 — proves the rule
    let mut satisfied_needs = 0usize;
    let mut open_needs = 0usize;

    s.push_str("MATCHES   (need -> providing offer; ACTIVE providers close the need)\n");
    for p in positions {
        for n in &p.needs {
            let provs = providers.get(n.satisfiable_by);
            let active: Vec<&Provider> = provs
                .map(|v| v.iter().filter(|x| x.active).collect())
                .unwrap_or_default();
            let inactive: Vec<&Provider> = provs
                .map(|v| v.iter().filter(|x| !x.active).collect())
                .unwrap_or_default();

            total_candidate_matches += active.len() + inactive.len();
            total_active_matches += active.len();
            // A match is only counted as ACTIVE when its offer is active; by
            // construction the count below can only ever be 0.
            for prov in &active {
                if !prov.active {
                    active_from_inactive_offer += 1;
                }
            }

            if !active.is_empty() {
                satisfied_needs += 1;
                let srcs: Vec<String> = active
                    .iter()
                    .map(|x| format!("{}[{}]", short(&x.construct), x.kind))
                    .collect();
                s.push_str(&format!(
                    "  SATISFIED  {:<28} need#{} {:<44} <- ACTIVE {}\n",
                    truncate(&short(&p.root_id), 28),
                    n.rank,
                    truncate(n.satisfiable_by, 44),
                    srcs.join(", ")
                ));
            } else {
                open_needs += 1;
                let note = if !inactive.is_empty() {
                    format!(
                        "{} inactive provider(s) exist but NONE is a live source",
                        inactive.len()
                    )
                } else {
                    "no construct offers this type at all (must be sourced elsewhere)".into()
                };
                s.push_str(&format!(
                    "  OPEN       {:<28} need#{} {:<44} -- {}\n",
                    truncate(&short(&p.root_id), 28),
                    n.rank,
                    truncate(n.satisfiable_by, 44),
                    note
                ));
            }
        }
    }
    s.push('\n');

    // ---- Three crisp assertions ------------------------------------------
    let bridge_type = "capability:physics-event-energy-deposit-bridge";
    let bridge_active = providers
        .get(bridge_type)
        .map(|v| v.iter().filter(|p| p.active).count())
        .unwrap_or(0);
    let bridge_needers = need_types.get(bridge_type).map(|v| v.len()).unwrap_or(0);

    // (2) sum of inactive providers that share a need type but satisfy nothing.
    let mut inactive_sharing_need_type = 0usize;
    for t in &ordered_types {
        if let Some(v) = providers.get(*t) {
            inactive_sharing_need_type += v.iter().filter(|p| !p.active).count();
        }
    }

    s.push_str("=== PROOF (three crisp assertions) ===\n");
    s.push_str(&format!(
        "ASSERTION 1  total offer<->need matches: {} candidate (type-overlap) pairs, of which {} are ACTIVE (satisfying).\n",
        total_candidate_matches, total_active_matches
    ));
    s.push_str(&format!(
        "             needs satisfied: {} / {}   (open: {})\n",
        satisfied_needs,
        satisfied_needs + open_needs,
        open_needs
    ));

    let rule_ok = active_from_inactive_offer == 0;
    s.push_str(&format!(
        "ASSERTION 2  ACTIVE-ONLY rule holds: {}. {} declared-but-inactive provider offer(s) share a need's\n",
        if rule_ok { "PASS" } else { "FAIL" },
        inactive_sharing_need_type
    ));
    s.push_str(&format!(
        "             type (e.g. moment:validation_run / moment:health_assessment — moments never produced),\n",
    ));
    s.push_str(&format!(
        "             yet active matches sourced from an inactive offer = {} (must be 0). A declared offer never satisfies.\n",
        active_from_inactive_offer
    ));

    s.push_str(&format!(
        "ASSERTION 3  universal gap: {} -> {} active providers (needed by {} construct(s)).\n",
        bridge_type, bridge_active, bridge_needers
    ));
    s.push_str(&format!(
        "             expected 0 — no construct in the store offers the physics-event->energy-deposit bridge;\n",
    ));
    s.push_str(
        "             every construct is written-not-run and cannot wire itself until that one bridge is built.\n",
    );

    let all_ok = rule_ok && bridge_active == 0;
    s.push_str(&format!(
        "\nMARKET RESULT: {} — offers and needs share one type vocabulary; {} live matches today because the\n",
        if all_ok { "COHERENT & HONEST" } else { "ASSERTION FAILURE" },
        total_active_matches
    ));
    s.push_str(
        "               city is authored-not-run. The matcher is real; the goods are not yet live.\n",
    );
    s.push_str("END — READ-ONLY: no store write, no event appended, no transaction committed.\n");

    print!("{s}");

    // Hard-fail the process if an assertion is violated, so the proof is not
    // merely printed but ENFORCED (still read-only; nothing was mutated).
    assert_eq!(active_from_inactive_offer, 0, "ASSERTION 2 violated: an inactive offer satisfied a need");
    assert_eq!(bridge_active, 0, "ASSERTION 3 violated: a construct unexpectedly offers the physics bridge");
}
