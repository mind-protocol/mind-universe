//! READ-ONLY ConstructChrome EMITTER: one frozen `ConstructChrome` JSON object
//! per live construct, over the LIVE store.
//!
//! `construct_map` answers, for ONE construct, "what is it for, is it real yet,
//! and what would make it more real?" — deriving, by independent readback, a
//! four-axis qualification (correctness / trust / health / epistemic), an ordered
//! typed NEEDS list, and the moments it PRODUCES. `construct_chrome` runs that
//! SAME derivation across EVERY construct in the store and projects each onto the
//! FROZEN `ConstructChrome` contract the renderer consumes:
//!
//! ```jsonc
//! { "construct", "name", "lifecycle", "axes": {correctness,trust,health,epistemic},
//!   "fog", "liveness", "needs_count", "top_need", "receipts": [{kind,id,fresh}] }
//! ```
//!
//! It opens the LIVE store READ-ONLY (`UniverseStore::open` + `replay` +
//! `read_content`; it NEVER writes, NEVER appends an event, NEVER commits a
//! `UniverseTransaction`), enumerates every construct (a `space` node whose
//! committed content carries `contractKind: "construct"`, excluding the shared
//! validity TEMPLATE), and for EACH runs the construct_map member-enumeration,
//! loop-binding, `classify_own` epistemic classification, determinability
//! propagation, `compute_needs`, and a PRODUCES-edge receipt scan (all replicated
//! verbatim so construct_map.rs is untouched).
//!
//! The load-bearing HONESTY RULE, enforced by a hard assertion before printing:
//! a not_measured / unknown axis is emitted as its LITERAL not_measured/unknown
//! string — never coerced to a confident 'good' value. `fog=true` whenever the
//! construct carries no fresh runtime evidence (its own self-verifying loop never
//! ran); `liveness=pulsing` ONLY when a fresh runtime moment (validation_run /
//! health_assessment among the construct's own runtime_moment_subtypes) is present
//! — otherwise `cold`. Today every construct is written-not-run: every one is
//! cold, foggy, and its trust/health/epistemic axes are honestly not_measured;
//! only the moments a construct actually PRODUCES (e.g. Underground's
//! change_ground effect receipts) appear in `receipts`.
//!
//! Usage: `construct_chrome [store-dir]`
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

use serde_json::{json, Value};
use universe_core::EntityKey;
use universe_store::UniverseStore;

const TEMPLATE_ID: &str = "space:l2:mind-universe:construct-validity-v0";

fn main() {
    if let Err(error) = run() {
        eprintln!("CONSTRUCT_CHROME FAILED: {error}");
        std::process::exit(1);
    }
}

// ===========================================================================
// Epistemic lattice + loop-template model — replicated from construct_map.rs
// (verbatim; construct_map.rs is left untouched).
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

/// A moment the construct PRODUCES (via a PRODUCES edge), read back independently.
struct Receipt {
    kind: String,
    id: String,
    fresh: bool,
}

/// The full construct_map derivation for one construct, projected into the fields
/// the frozen ConstructChrome contract needs.
struct Analyzed {
    root_id: String,
    name: String,
    // qualification axes (already honesty-mapped)
    correctness: &'static str,
    trust: &'static str,
    health: &'static str,
    epistemic: &'static str,
    lifecycle: &'static str,
    fog: bool,
    pulsing: bool,
    needs: Vec<Need>,
    receipts: Vec<Receipt>,
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
    //     contractKind == "construct", EXCLUDING the shared validity template.
    //     Independent readback of the LIVE store — no self-reported registry.
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
    let mut analyzed: Vec<Analyzed> = Vec::new();
    for root_id in &construct_ids {
        analyzed.push(analyze_construct(root_id, &snapshot, &idx, &template));
    }

    // --- Project into the frozen ConstructChrome array ------------------------
    let chrome: Vec<Value> = analyzed.iter().map(to_chrome).collect();

    // --- Enforce the honesty rule BEFORE printing -----------------------------
    // Every not_measured/unknown axis must be emitted as its literal token, and a
    // cold construct may never carry a confident 'good' axis value. This is not a
    // display nicety — it is asserted, so a regression that laundered a fog axis
    // into a good color would hard-fail the emitter (still read-only; no mutation).
    assert_honesty(&analyzed);

    // --- Emit --------------------------------------------------------------
    println!("{}", serde_json::to_string_pretty(&Value::Array(chrome))?);
    println!(
        "HONESTY OK: {} construct(s); every not_measured/unknown axis emitted as its literal string \
(never coerced to a good value); no cold construct carries a confident-good axis; \
liveness=pulsing only when a fresh runtime moment exists (0 today -> all cold).",
        analyzed.len()
    );
    Ok(())
}

/// Map one Analyzed onto the frozen ConstructChrome JSON object.
fn to_chrome(a: &Analyzed) -> Value {
    let top_need = a.needs.first().map(|n| {
        json!({
            "kind": n.blocker_kind,
            "target": n.target,
            "action": n.action,
        })
    });
    let receipts: Vec<Value> = a
        .receipts
        .iter()
        .map(|r| json!({ "kind": r.kind, "id": r.id, "fresh": r.fresh }))
        .collect();
    json!({
        "construct": a.root_id,
        "name": a.name,
        "lifecycle": a.lifecycle,
        "axes": {
            "correctness": a.correctness,
            "trust": a.trust,
            "health": a.health,
            "epistemic": a.epistemic,
        },
        "fog": a.fog,
        "liveness": if a.pulsing { "pulsing" } else { "cold" },
        "needs_count": a.needs.len(),
        "top_need": top_need,
        "receipts": receipts,
    })
}

/// The honesty gate. Panics (read-only; nothing was mutated) if any construct
/// laundered an unmeasured axis into a confident value.
fn assert_honesty(analyzed: &[Analyzed]) {
    let good_epistemic = |s: &str| matches!(s, "observed" | "measured");
    for a in analyzed {
        // A cold construct (no fresh runtime moment) has produced no fresh
        // evidence of its functioning: its runtime axes MUST stay honest.
        if !a.pulsing {
            assert!(
                a.health != "healthy",
                "{}: cold construct claims health=healthy without a fresh health_assessment",
                a.root_id
            );
            assert!(
                a.trust != "strong",
                "{}: cold construct claims trust=strong without fresh verification",
                a.root_id
            );
            assert!(
                !good_epistemic(a.epistemic),
                "{}: cold construct claims epistemic={} without a fresh runtime moment",
                a.root_id,
                a.epistemic
            );
            // fog must be raised when there is no fresh runtime evidence.
            assert!(
                a.fog,
                "{}: cold construct not flagged fog=true despite no fresh runtime evidence",
                a.root_id
            );
        }
        // liveness=pulsing is reserved for a genuinely-run construct.
        if a.pulsing {
            assert!(
                !a.fog,
                "{}: pulsing (running) construct must not be foggy",
                a.root_id
            );
        }
    }
}

/// Run the construct_map member-enumeration + binding + determinability pipeline
/// for one construct root and return its ConstructChrome-relevant projection.
fn analyze_construct(
    root_id: &str,
    snapshot: &universe_store::UniverseSnapshot,
    idx: &StoreIndex,
    template: &LoopTemplate,
) -> Analyzed {
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

    // Honesty fields (implementation self-report — NOT evidence).
    let mut honesty = Honesty::default();
    if let Some(imp) = members.iter().find(|m| m.subtype == "implementation") {
        honesty.graph_status = str_field(&imp.inner, "graph_status");
        honesty.wiring_status = str_field(&imp.inner, "wiring_status");
        honesty.runtime_status = str_field(&imp.inner, "runtime_status");
        honesty.verification_status = str_field(&imp.inner, "verification_status");
    }
    // Independent readback: the entities ARE present, so a self-reported
    // graph_status="not_written" is STALE. The graph-write is observed.
    let graph_written_observed = idx.id_to_key.contains_key(root_id);

    let runtime_moment_subtypes: BTreeSet<String> = idx
        .wrappers_by_id
        .get(root_id)
        .and_then(|w| w.get("content"))
        .and_then(|c| c.get("runtime_moment_subtypes"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    // A fresh runtime moment = a member whose subtype is one this construct
    // PRODUCES by running its verify loop (validation_run / health_assessment).
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

    // NEEDS (the shared, typed, ordered repair ladder).
    let needs = compute_needs(template, &states, &index, &required, &det, &part_to_member, &members);

    // RECEIPTS — moments this construct PRODUCES, via PRODUCES edges from the
    // construct root or any of its members, read back independently. Today this
    // is empty for every construct except the ones that ran a real effect (e.g.
    // Underground's change_ground EffectReceipt Moments).
    let receipts = collect_receipts(root_id, &member_keys, snapshot, idx);

    // --- Project onto the four ConstructChrome axes + lifecycle/fog/liveness ---
    let (correctness, trust, health, epistemic) = qualify_axes(
        &states,
        &det,
        &honesty,
        runtime_moment_present,
    );
    let lifecycle = lifecycle_of(
        graph_written_observed,
        &honesty,
        runtime_moment_present,
        trust,
    );
    // fog = no fresh runtime evidence of the construct's own functioning. Its
    // structure is observed (independent readback), but health/trust/epistemic
    // stay not_measured until the self-verifying loop actually runs.
    let fog = !runtime_moment_present;
    let pulsing = runtime_moment_present;

    let name = members
        .iter()
        .find(|m| m.canonical_id == *root_id)
        .and_then(|m| m.inner.get("name").and_then(Value::as_str))
        .unwrap_or(root_id)
        .to_string();

    Analyzed {
        root_id: root_id.to_string(),
        name,
        correctness,
        trust,
        health,
        epistemic,
        lifecycle,
        fog,
        pulsing,
        needs,
        receipts,
    }
}

/// The four ConstructChrome axes, honesty-mapped from the construct_map state.
///
/// * correctness is a STRUCTURAL judgement, assessable from independent readback
///   of the graph (never foggy): logical_error > incomplete (a role definition
///   absent) > correct (whole loop confirmed) > chained (definitions present and
///   structurally chained, but verification is runtime-pending).
/// * trust / health / epistemic are RUNTIME judgements: without a fresh runtime
///   moment there is nothing to trust, no health to derive, and the construct's
///   central claim (its objective) is not_measured. We NEVER emit a confident
///   'good' value (strong / healthy / observed|measured) from a self-report.
fn qualify_axes(
    states: &[PartState],
    det: &BTreeMap<String, Det>,
    honesty: &Honesty,
    runtime_moment_present: bool,
) -> (&'static str, &'static str, &'static str, &'static str) {
    // correctness ----------------------------------------------------------
    let any_logical_error = states.iter().any(|s| s.det == Det::LogicalError);
    // a required ROLE definition absent = the loop is missing a piece.
    let missing_role = states
        .iter()
        .any(|s| s.band == Band::Role && s.own != Epistemic::Observed);
    let all_confirmed = !states.is_empty()
        && states
            .iter()
            .all(|s| det.get(&s.part).copied().unwrap_or(Det::NotDeterminable) == Det::Confirmed);
    let correctness = if states.is_empty() {
        "unknown"
    } else if any_logical_error {
        "logical_error"
    } else if missing_role {
        "incomplete"
    } else if all_confirmed {
        "correct"
    } else {
        // every role definition is present and forward-chained, but the loop
        // only closes through a fresh runtime moment that has not happened.
        "chained"
    };

    // trust ----------------------------------------------------------------
    // Only a fresh, passing verification earns 'strong'/'adequate'. A construct
    // that has never run has an unmeasured trust — self-report is not evidence.
    let verified_fresh = runtime_moment_present
        && matches!(
            honesty.verification_status.as_str(),
            "independently_verified" | "verified"
        );
    let trust = if verified_fresh {
        "strong"
    } else if runtime_moment_present {
        "adequate"
    } else {
        "not_measured"
    };

    // health ---------------------------------------------------------------
    // Health is DERIVED from a fresh metric vector via a health_assessment
    // moment. Absent that, it is not_measured — never silently 'healthy'.
    let health = if runtime_moment_present {
        // A runtime moment exists but this read path does not re-derive the
        // verdict; report it as measured-but-unresolved rather than a good color.
        "unknown"
    } else {
        "not_measured"
    };

    // epistemic ------------------------------------------------------------
    // The construct's STRUCTURE is observed, but its central claim (does the
    // loop actually close / objective satisfied) is only observed once a fresh
    // runtime moment exists. Cold -> not_measured (the honest fog token).
    let epistemic = if runtime_moment_present { "observed" } else { "not_measured" };

    (correctness, trust, health, epistemic)
}

/// The lifecycle stage, honestly. `not_written` never appears here (enumeration
/// already proved graph presence); `wired` is a self-reported stage label (not an
/// evidence axis); `running`/`verified` require a fresh runtime moment.
fn lifecycle_of(
    graph_written_observed: bool,
    honesty: &Honesty,
    runtime_moment_present: bool,
    trust: &str,
) -> &'static str {
    if !graph_written_observed {
        "not_written"
    } else if runtime_moment_present {
        if trust == "strong" {
            "verified"
        } else {
            "running"
        }
    } else if honesty.wiring_status == "wired" {
        "wired"
    } else {
        "written"
    }
}

/// Scan PRODUCES edges out of the construct (root or member) and read back each
/// produced moment independently. `fresh` = the moment is present now via
/// independent readback (a real committed moment, not a stale self-claim).
fn collect_receipts(
    _root_id: &str,
    member_keys: &BTreeSet<EntityKey>,
    snapshot: &universe_store::UniverseSnapshot,
    idx: &StoreIndex,
) -> Vec<Receipt> {
    let Some(produces) = snapshot.symbol_id("PRODUCES") else {
        return Vec::new();
    };
    let mut out: Vec<Receipt> = Vec::new();
    for r in &snapshot.relations {
        if r.predicate != produces {
            continue;
        }
        if !member_keys.contains(&r.source) {
            continue;
        }
        // The produced target must be a readable entity (present in the store).
        let Some(w) = idx.key_to_wrapper.get(&r.target) else { continue };
        let id = w.get("canonical_id").and_then(Value::as_str).unwrap_or("").to_string();
        // A receipt is a MOMENT (a runtime/effect moment), never a structural
        // loop role that merely sits on the tail of an intra-loop PRODUCES edge
        // (e.g. observability_algorithm PRODUCES metric — a definitional wire, not
        // a receipt). Keep only moment entities: node_type=="moment" or a
        // `moment:` canonical id. This drops the `metric:` role, keeps Underground's
        // `moment:...:change-ground` EffectReceipts.
        let is_moment = w.get("node_type").and_then(Value::as_str) == Some("moment")
            || id.starts_with("moment:");
        if !is_moment {
            continue;
        }
        // kind: prefer the moment's declared subtype, else its effect name, else
        // its node_type. (change_ground receipts carry content.effect.)
        let kind = w
            .get("subtype")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                w.get("content")
                    .and_then(|c| c.get("effect"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| w.get("node_type").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "moment".to_string());
        out.push(Receipt { kind, id, fresh: true });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
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
// NEED derivation — replicated from construct_map.rs verbatim.
// ===========================================================================
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
