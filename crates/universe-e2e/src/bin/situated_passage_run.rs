//! Wield the L1 situated-passage machine: MOVE a body from one holder to
//! another, FROM THE LIVE GRAPH, in one atomic set.
//!
//! This is the driver for `space:l1:mind-universe:situated-passage-v0`, the
//! construct the Mechanical toolkit produces at the citizen's inner scale. Every
//! variable is read from the COMMITTED store, never from the authoring fixture:
//!
//!   * the machine (modules, typed ports, couplings, atoms, bonds, effect
//!     binding) from the passage's `code` node;
//!   * the compatibility rule the couplings are checked against from the
//!     COMMITTED Mechanical toolkit;
//!   * the mutation set's verbs, its holding predicate and its Moment field
//!     schema from that same `code` node;
//!   * the interlock's modes and the capability a granted passage requires
//!     likewise.
//!
//! The only native policy here is mechanism with zero variables: kernel write
//! verbs, an optimistic commit, and one evaluator per named check.
//!
//! # What movement IS here
//!
//! Perception in this Universe reads no stored coordinate: what puts a body in
//! an observation is REACHABILITY, so a body's whereabouts is the containment
//! edge that holds it. Moving is therefore not writing a position — it is
//! releasing one holding edge and writing another. This driver never writes a
//! coordinate, and never claims metres.
//!
//! # The two failures it makes unreachable
//!
//!   Orphaned — the last holding edge cut with none written. Structurally
//!              impossible upstream: the AND-gate cannot assemble a plan from a
//!              traveller alone, so nothing can fire that would only sever.
//!   Doubled  — a new holder bound while the old one still holds. Structurally
//!              impossible downstream: release and bind share ONE write-set, so
//!              there is no revision in between for an observer to catch.
//!
//! # What it measures (in this order, all against the live store)
//!
//!   1. ASSEMBLY  — every authored coupling joins compatible ports; both
//!      authored `refused_couplings` are refused; couplings and bonds
//!      correspond 1:1.
//!   2. MACHINE   — the nominal wave: both sources supplied -> the AND-gate
//!      passage module activates -> the trigger fires -> the terminal effector
//!      fires and surfaces exactly ONE candidate EffectIntent whose payload is
//!      the authored mutation set.
//!   3. STARVATION— the negative wave: the destination is NOT supplied -> the
//!      passage module does not activate, nothing is deposited, no candidate is
//!      surfaced, and therefore nothing could have been severed. Measured, not
//!      assumed.
//!   4. RESOLUTION— traveller, destination and every holding edge are resolved
//!      from the COMMITTED graph. An unresolved endpoint aborts BEFORE any write.
//!   5. INTERLOCK — self_passage, or `relocate_inhabitant` READ FROM THE GRAPH
//!      (the actor's held `USED` edges). A caller-declared scope is not a scope.
//!   6. COMMIT    — tombstone(s) + put_relation + put_entity as ONE write-set,
//!      against a freshly replayed base revision, with bounded retry.
//!   7. EVIDENCE  — an INDEPENDENT readback (fresh reopen) measures the
//!      traveller's holding edges, then one `validation_run` Moment + one
//!      `health_assessment` Moment are committed and read back.
//!
//! HONESTY. A refused interlock, an absent destination and a starved machine are
//! MEASURED non-passages, each with its reason — never an absent measurement and
//! never a passage reported as partial. Overall health is never `healthy` from
//! one passage: the authored derivation requires a population (both interlock
//! modes, a refusal, repeated runs), which a single move cannot measure.
//!
//! Usage:
//! ```text
//! situated_passage_run [store-dir]
//!     --traveller <canonical id> --destination <canonical id>
//!     --acting-session <canonical id> [--reason "<why>"] [--apply]
//! ```
//! store-dir: UNIVERSE_STORE env, then the positional arg, then
//!            artifacts/ontology-registry/current/store (the LIVE store).
//! Without `--apply` the run is a DRY RUN: it assembles, runs both waves,
//! resolves every endpoint and evaluates the interlock, and writes NOTHING.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, path::PathBuf};

use serde_json::{json, Value};

use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError};
use universe_e2e::construct_resolver::{resolve_construct, AlarmAtomCircuit, ResolvedConstruct};
use universe_physics::{AtomConvergence, AtomExecutionBudget};
use universe_query::read_actor_capability_set;
use universe_store::{
    EntityRecord, IndexedUniverseSnapshot, RelationRecord, UniverseSnapshot, UniverseStore,
};
use universe_supervisor::{PhysicsDepositOutcome, Supervisor};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// The construct this driver wields, and the toolkit that produced it. Both are
/// read from the COMMITTED store; neither fixture file is opened.
const PASSAGE_CODE_ID: &str = "code:l1:mind-universe:situated-passage-v0";
const PASSAGE_SPACE_ID: &str = "space:l1:mind-universe:situated-passage-v0";
const PASSAGE_PORT_ID: &str = "port:l1:mind-universe:situated-passage:relocation-v0";
const TOOLKIT_ALGORITHM_ID: &str = "algorithm:l2:mind-universe:mechanical-toolkit-v0";

/// The canonical predicate an actor's held capability set is read from (the
/// grant's authored HOLDS_CAPABILITY remaps to USED; see `revise_construct`).
const GRANT_PREDICATE: &str = "USED";
/// The capability-entity content field naming the capability it confers.
const CAPABILITY_FIELD: &str = "capability";
/// Bounded relation budget for the actor's capability read (never a full scan).
const CAPABILITY_READ_BUDGET: usize = 64;

/// Key blocks for this construct's writes, disjoint from the injector's block
/// and from `ollama_probe_run`'s Moment block (0x2000_0000 / 0x2100_0000).
const PASSAGE_ENTITY_BASE: u128 = 0x2200_0000;
const PASSAGE_REL_BASE: u128 = 0x2300_0000;
const RUN_MOMENT_ENTITY_BASE: u128 = 0x2400_0000;
const RUN_MOMENT_REL_BASE: u128 = 0x2500_0000;
const KEY_BLOCK_SPAN: u128 = 4096;

const COMMIT_ATTEMPTS: usize = 4;

fn budget() -> AtomExecutionBudget {
    AtomExecutionBudget {
        max_atoms: 16,
        max_bonds: 16,
        max_steps: 16,
        max_total_energy: 10_000,
    }
}

// ===========================================================================
// Arguments
// ===========================================================================

struct Args {
    store_dir: PathBuf,
    traveller: String,
    destination: String,
    acting_session: String,
    reason: Option<String>,
    apply: bool,
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut store_dir: Option<PathBuf> = env::var_os("UNIVERSE_STORE").map(PathBuf::from);
    let mut traveller = None;
    let mut destination = None;
    let mut acting_session = None;
    let mut reason = None;
    let mut apply = false;

    let raw: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        let arg = raw[i].clone();
        let mut next = |what: &str| -> Result<String, Box<dyn Error>> {
            i += 1;
            raw.get(i)
                .cloned()
                .ok_or_else(|| format!("{what} requires a value").into())
        };
        match arg.as_str() {
            "--traveller" => traveller = Some(next("--traveller")?),
            "--destination" => destination = Some(next("--destination")?),
            "--acting-session" => acting_session = Some(next("--acting-session")?),
            "--reason" => reason = Some(next("--reason")?),
            "--apply" => apply = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown flag {other}").into())
            }
            positional => {
                if store_dir.is_some() && env::var_os("UNIVERSE_STORE").is_none() {
                    return Err(format!("unexpected second store dir {positional}").into());
                }
                if env::var_os("UNIVERSE_STORE").is_none() {
                    store_dir = Some(PathBuf::from(positional));
                }
            }
        }
        i += 1;
    }

    let args = Args {
        store_dir: store_dir
            .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store")),
        traveller: traveller.ok_or("--traveller <canonical id> is required: this machine holds no default idea of who moves")?,
        destination: destination.ok_or("--destination <canonical id> is required: a passage with no destination is exactly what this machine refuses to assemble")?,
        acting_session: acting_session.ok_or("--acting-session <canonical id> is required: every passage is attributable")?,
        reason,
        apply,
    };
    if args.apply && args.reason.as_deref().unwrap_or("").trim().is_empty() {
        return Err("--apply requires --reason \"<why>\": an edge cut with no retained reason is \
                    an unexplained hole"
            .into());
    }
    Ok(args)
}

// ===========================================================================
// Reading the committed graph
// ===========================================================================

fn read_nodes(
    supervisor: &Supervisor,
    snapshot: &UniverseSnapshot,
    ids: &[&str],
) -> Result<BTreeMap<String, (EntityKey, Value)>, Box<dyn Error>> {
    let wanted: BTreeSet<&str> = ids.iter().copied().collect();
    let mut found: BTreeMap<String, (EntityKey, Value)> = BTreeMap::new();
    for entity in &snapshot.entities {
        if found.len() == wanted.len() {
            break;
        }
        let Some(content_ref) = entity.content.as_ref() else {
            continue;
        };
        let wrapper = supervisor.read_content(content_ref)?;
        let Some(canonical) = wrapper.get("canonical_id").and_then(Value::as_str) else {
            continue;
        };
        if wanted.contains(canonical) {
            found.insert(canonical.to_string(), (entity.key, wrapper));
        }
    }
    for id in ids {
        if !found.contains_key(*id) {
            return Err(format!("canonical id {id} is not committed in this store").into());
        }
    }
    Ok(found)
}

/// The inner authored content of a committed node (the injector wraps authored
/// content under `/content`).
fn inner(wrapper: &Value) -> Result<&Value, Box<dyn Error>> {
    wrapper
        .get("content")
        .ok_or_else(|| "committed node carries no content block".into())
}

/// Resolve a canonical id to its entity key by hydrating committed content.
/// Absence is reported as absence — never defaulted to a key.
fn key_of_canonical(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    canonical_id: &str,
) -> Result<Option<EntityKey>, Box<dyn Error>> {
    for entity in &snapshot.entities {
        let Some(pointer) = entity.content.as_ref() else {
            continue;
        };
        let content = store.read_content(pointer)?;
        if content.get("canonical_id").and_then(Value::as_str) == Some(canonical_id) {
            return Ok(Some(entity.key));
        }
    }
    Ok(None)
}

/// Every holding edge of a traveller: a relation carrying the holding predicate
/// whose SOURCE is the traveller. Direction matters — `x PART_OF y` means y
/// holds x, and reversing it would sever the wrong side of the world.
fn holding_edges(
    snapshot: &UniverseSnapshot,
    traveller: EntityKey,
    holding_predicate: u32,
) -> Vec<(RelationKey, u32, EntityKey)> {
    snapshot
        .relations
        .iter()
        .filter(|r| r.source == traveller && r.predicate == holding_predicate)
        .map(|r| (r.key, r.generation, r.target))
        .collect()
}

// ===========================================================================
// Assembly: the Mechanical toolkit's compatibility rule, applied
// ===========================================================================

struct AssemblyEvidence {
    rule: String,
    admitted: Vec<String>,
    refused: Vec<String>,
    wrongly_admitted: Vec<String>,
    wrongly_refused: Vec<String>,
    coupling_bond_correspondence: bool,
}

/// A coupling is admitted iff the output port type is accepted by the input port
/// type. The graph declares NO widening relation between types, so identity is
/// the only acceptance relation there is evidence for; inventing a subtyping rule
/// would be native policy the Universe never authored.
fn admit(out_type: &str, in_type: &str) -> bool {
    out_type == in_type
}

fn check_assembly(circuit: &Value, toolkit_rule: &str) -> Result<AssemblyEvidence, Box<dyn Error>> {
    let list = |name: &str| -> Result<Vec<Value>, Box<dyn Error>> {
        Ok(circuit
            .get(name)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("machine_circuit has no {name} array"))?
            .clone())
    };
    let triple = |coupling: &Value| -> Result<(String, String, String), Box<dyn Error>> {
        let field = |name: &str| -> Result<String, Box<dyn Error>> {
            Ok(coupling
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("coupling without {name}"))?
                .to_string())
        };
        Ok((field("key")?, field("out_type")?, field("in_type")?))
    };

    let mut admitted = Vec::new();
    let mut wrongly_refused = Vec::new();
    let mut coupling_keys = BTreeSet::new();
    for coupling in list("couplings")? {
        let (key, out_type, in_type) = triple(&coupling)?;
        coupling_keys.insert(key.clone());
        if admit(&out_type, &in_type) {
            admitted.push(format!("{key}  ({out_type} -> {in_type})"));
        } else {
            wrongly_refused.push(key);
        }
    }
    let mut refused = Vec::new();
    let mut wrongly_admitted = Vec::new();
    for coupling in list("refused_couplings")? {
        let (key, out_type, in_type) = triple(&coupling)?;
        if admit(&out_type, &in_type) {
            wrongly_admitted.push(key);
        } else {
            refused.push(format!("{key}  ({out_type} -/-> {in_type})"));
        }
    }
    let bond_keys: BTreeSet<String> = list("bonds")?
        .iter()
        .filter_map(|b| b.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();

    Ok(AssemblyEvidence {
        rule: toolkit_rule.to_string(),
        coupling_bond_correspondence: bond_keys == coupling_keys,
        admitted,
        refused,
        wrongly_admitted,
        wrongly_refused,
    })
}

// ===========================================================================
// Key allocation
// ===========================================================================

fn free_entity_key(snapshot: &UniverseSnapshot, base: u128) -> Result<EntityKey, Box<dyn Error>> {
    for offset in 0..KEY_BLOCK_SPAN {
        let key = EntityKey(base + offset);
        if !snapshot.entities.iter().any(|entity| entity.key == key) {
            return Ok(key);
        }
    }
    Err(format!("no free entity key in the block at {base:#x}").into())
}

fn free_relation_key(
    snapshot: &UniverseSnapshot,
    base: u128,
) -> Result<RelationKey, Box<dyn Error>> {
    for offset in 0..KEY_BLOCK_SPAN {
        let key = RelationKey(base + offset);
        if !snapshot.relations.iter().any(|relation| relation.key == key) {
            return Ok(key);
        }
    }
    Err(format!("no free relation key in the block at {base:#x}").into())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("PASSAGE FAILED: {error}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let store_dir = args.store_dir.clone();
    let genesis = PathBuf::from("fixtures/genesis/minimal-genesis.json");
    println!("store dir : {}", store_dir.display());
    println!("traveller : {}", args.traveller);
    println!("destination: {}", args.destination);
    println!("acting    : {}", args.acting_session);
    println!(
        "mode      : {}",
        if args.apply {
            "APPLY (commits one atomic passage)"
        } else {
            "DRY RUN (measures everything, writes nothing)"
        }
    );

    let supervisor = Supervisor::boot(&store_dir, &genesis)?;
    let revision_before = supervisor.revision();
    println!(
        "\nbase revision: {} | entities: {} | relations: {}",
        revision_before.0,
        supervisor.snapshot().entities.len(),
        supervisor.snapshot().relations.len()
    );

    // ---- (0) read the construct and its producing toolkit from the STORE ----
    let snapshot = supervisor.snapshot().clone();
    let nodes = read_nodes(
        &supervisor,
        &snapshot,
        &[
            PASSAGE_CODE_ID,
            PASSAGE_SPACE_ID,
            PASSAGE_PORT_ID,
            TOOLKIT_ALGORITHM_ID,
        ],
    )?;
    let (passage_code_key, passage_code_wrapper) = nodes[PASSAGE_CODE_ID].clone();
    let (passage_space_key, _) = nodes[PASSAGE_SPACE_ID].clone();
    let (_, passage_port_wrapper) = nodes[PASSAGE_PORT_ID].clone();
    let passage_port = inner(&passage_port_wrapper)?.clone();
    let (toolkit_algorithm_key, toolkit_algorithm_wrapper) = nodes[TOOLKIT_ALGORITHM_ID].clone();
    let passage_code = inner(&passage_code_wrapper)?.clone();
    let toolkit_algorithm = inner(&toolkit_algorithm_wrapper)?.clone();
    println!(
        "read from the live graph: passage code {:#x}, mechanical toolkit algorithm {:#x}",
        passage_code_key.0, toolkit_algorithm_key.0
    );

    let circuit_value = passage_code
        .get("machine_circuit")
        .ok_or("passage code node carries no machine_circuit")?
        .clone();
    let interlock_spec = passage_code
        .get("interlock")
        .ok_or("passage code node carries no interlock")?
        .clone();

    // ---- (1) ASSEMBLY --------------------------------------------------------
    let toolkit_rule = toolkit_algorithm
        .get("compatibility_rule")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or("the committed Mechanical toolkit algorithm declares no compatibility_rule")?;
    let assembly = check_assembly(&circuit_value, &toolkit_rule)?;
    println!("\n-- (1) assembly checked against the COMMITTED Mechanical toolkit --");
    println!("  toolkit rule: {}", assembly.rule);
    for coupling in &assembly.admitted {
        println!("  ADMITTED  {coupling}");
    }
    for coupling in &assembly.refused {
        println!("  REFUSED   {coupling}   (authored negative case)");
    }
    if !assembly.wrongly_admitted.is_empty() || !assembly.wrongly_refused.is_empty() {
        return Err(format!(
            "compatibility rule violated: wrongly admitted {:?}, wrongly refused {:?}",
            assembly.wrongly_admitted, assembly.wrongly_refused
        )
        .into());
    }
    if !assembly.coupling_bond_correspondence {
        return Err("declared couplings and physicalized bonds do not correspond 1:1".into());
    }
    println!(
        "  couplings <-> bonds correspond 1:1; {} admitted, {} refused, 0 mis-wired",
        assembly.admitted.len(),
        assembly.refused.len()
    );

    // ---- (2) MACHINE: the nominal wave ---------------------------------------
    //
    // The candidate's payload IS the authored mutation set, serialized. Nothing
    // is invented: the bytes are a rendering of committed graph content, and the
    // fidelity check below parses them back and compares VALUES, so the proof
    // does not depend on key ordering.
    let authored_set = circuit_value
        .pointer("/effect_bindings/0/mutation_set")
        .ok_or("the effect binding declares no mutation_set")?
        .clone();
    let payload_bytes = serde_json::to_string(&authored_set)?;
    let mut circuit_json = circuit_value.clone();
    circuit_json["effect_bindings"][0]["message"] = json!(payload_bytes);

    let circuit: AlarmAtomCircuit = serde_json::from_value(circuit_json.clone())?;
    let resolved: ResolvedConstruct = resolve_construct(&circuit)
        .map_err(|error| format!("resolve_construct failed: {error:?}"))?;
    let nominal: PhysicsDepositOutcome = supervisor.run_physics_deposit_phase(
        resolved.sensor_cluster.clone(),
        &resolved.deposit_bindings,
        resolved.construct_cluster.clone(),
        &resolved.effect_bindings,
        budget(),
    )?;
    let atom_key = |name: &str| -> Result<EntityKey, Box<dyn Error>> {
        resolved
            .atom_keys
            .get(name)
            .copied()
            .ok_or_else(|| format!("authored circuit has no atom {name}").into())
    };
    let trigger = atom_key("passage_trigger")?;
    let effector = atom_key("passage_effector")?;
    println!("\n-- (2) the machine ran (nominal wave: traveller AND destination supplied) --");
    let quiescent = matches!(nominal.sensor.convergence, AtomConvergence::Quiescent)
        && matches!(nominal.construct.convergence, AtomConvergence::Quiescent);
    let conserved = nominal.sensor.energy.conserved && nominal.construct.energy.conserved;
    println!(
        "  sensor {:?} / construct {:?}; energy conserved: {conserved}",
        nominal.sensor.convergence, nominal.construct.convergence
    );
    if !nominal.fired_construct_atoms.contains(&trigger)
        || !nominal.fired_construct_atoms.contains(&effector)
    {
        return Err("the passage trigger and the terminal effector did not both fire".into());
    }
    if nominal.candidate_effects.len() != 1 {
        return Err(format!(
            "expected exactly one candidate EffectIntent, got {}",
            nominal.candidate_effects.len()
        )
        .into());
    }
    let candidate = nominal.candidate_effects[0].clone();
    println!(
        "  trigger {:#x} and terminal effector {:#x} fired; 1 candidate surfaced",
        trigger.0, effector.0
    );

    let carried: Value = serde_json::from_slice(&candidate.payload)
        .map_err(|error| format!("the candidate payload is not the mutation set: {error}"))?;
    let payload_fidelity = carried == authored_set;
    if !payload_fidelity {
        return Err("the surfaced candidate carries a mutation set that differs from the \
                    committed one"
            .into());
    }
    let bonds = authored_set
        .as_array()
        .ok_or("the committed mutation_set is not an array")?;
    let verbs: Vec<String> = bonds
        .iter()
        .filter_map(|b| {
            b.get("command_kind")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    if verbs.len() != 3 || verbs.len() != bonds.len() {
        return Err(format!(
            "the mutation set must be exactly three one-verb bonds; it declares {} bond(s) and \
             {} verb(s)",
            bonds.len(),
            verbs.len()
        )
        .into());
    }
    const KERNEL_VERBS: [&str; 4] = [
        "intern_symbols",
        "put_entity",
        "put_relation",
        "tombstone_relation",
    ];
    for verb in &verbs {
        if !KERNEL_VERBS.contains(&verb.as_str()) {
            return Err(format!("mutation bond names {verb}, which is not a kernel write verb").into());
        }
    }
    println!(
        "  candidate carries the committed mutation set: 3 one-verb bonds [{}]",
        verbs.join(", ")
    );

    // ---- (3) STARVATION: the negative wave -----------------------------------
    let mut starved_circuit: AlarmAtomCircuit = serde_json::from_value(circuit_json.clone())?;
    starved_circuit
        .external_measured_injections
        .remove("destination_named");
    let starved_resolved = resolve_construct(&starved_circuit)
        .map_err(|error| format!("resolve_construct (starved) failed: {error:?}"))?;
    let starved = supervisor.run_physics_deposit_phase(
        starved_resolved.sensor_cluster.clone(),
        &starved_resolved.deposit_bindings,
        starved_resolved.construct_cluster.clone(),
        &starved_resolved.effect_bindings,
        budget(),
    )?;
    let starved_effector = starved_resolved
        .atom_keys
        .get("passage_effector")
        .copied()
        .ok_or("starved circuit has no effector atom")?;
    let starved_correct =
        starved.candidate_effects.is_empty() && !starved.fired_construct_atoms.contains(&starved_effector);
    println!("\n-- (3) starvation (a traveller named, NO destination) --");
    println!(
        "  candidates surfaced: {} | effector fired: {}",
        starved.candidate_effects.len(),
        starved.fired_construct_atoms.contains(&starved_effector)
    );
    if !starved_correct {
        return Err("the starved machine surfaced an effect: a departure with no arrival was \
                    assemblable, which is the one thing this construct exists to prevent"
            .into());
    }
    println!("  nothing was assembled, and therefore nothing could have been severed.");

    // ---- (4) RESOLUTION: endpoints from the COMMITTED graph -------------------
    let store = UniverseStore::open(&store_dir)?;
    let live = store.replay(store.load_snapshot()?)?;
    let holding_predicate_name = authored_set
        .get(1)
        .and_then(|b| b.get("predicate"))
        .and_then(Value::as_str)
        .ok_or("the binding bond declares no holding predicate")?
        .to_string();
    let holding_predicate = live
        .symbol_id(&holding_predicate_name)
        .ok_or_else(|| format!("holding predicate {holding_predicate_name} is not interned"))?;

    let traveller_key = key_of_canonical(&store, &live, &args.traveller)?
        .ok_or_else(|| format!("the traveller {} is not in this graph", args.traveller))?;
    let destination_key = key_of_canonical(&store, &live, &args.destination)?.ok_or_else(|| {
        format!(
            "the destination {} is not in this graph: arriving nowhere is the same failure as \
             leaving for nowhere, so nothing is written",
            args.destination
        )
    })?;
    let held_by = holding_edges(&live, traveller_key, holding_predicate);
    println!("\n-- (4) endpoints resolved from the committed graph --");
    println!(
        "  traveller   {:#x}  {}",
        traveller_key.0, args.traveller
    );
    println!(
        "  destination {:#x}  {}",
        destination_key.0, args.destination
    );
    println!(
        "  holding edges ({holding_predicate_name}) to release: {}",
        held_by.len()
    );
    for (relation, generation, target) in &held_by {
        let name = key_name(&store, &live, *target)?;
        println!(
            "    edge {:#x} gen {generation} -> holder {:#x}  {name}",
            relation.0, target.0
        );
    }
    if held_by.is_empty() {
        return Err(format!(
            "the traveller {} is held by nothing: it is already orphaned, and this machine \
             repairs nothing it did not cause",
            args.traveller
        )
        .into());
    }
    if held_by.iter().any(|(_, _, target)| *target == destination_key) {
        return Err(format!(
            "the traveller is ALREADY held by {}: there is no passage to make",
            args.destination
        )
        .into());
    }

    // ---- (5) INTERLOCK: a scope read from the graph ---------------------------
    // The capability a granted passage requires is read from the COMMITTED port
    // node, not from a constant here: the scope is graph data, and the port is
    // the surface that declares it.
    let required_capability = passage_port
        .get("required_mutate_capability")
        .and_then(Value::as_str)
        .ok_or("the committed relocation port declares no required_mutate_capability")?
        .to_string();
    // Both modes must be declared in the graph. A driver that recognised a mode
    // the construct never authored would be holding policy of its own.
    for mode in ["self_passage", "granted_passage"] {
        if interlock_spec.pointer(&format!("/modes/{mode}")).is_none() {
            return Err(format!("the committed interlock declares no mode {mode}").into());
        }
    }
    let self_passage = args.acting_session == args.traveller;
    let mut held_capabilities: Vec<String> = Vec::new();
    let interlock_mode = if self_passage {
        "self_passage".to_string()
    } else {
        let indexed = IndexedUniverseSnapshot::new(live.clone())?;
        let grant_predicate = live
            .symbol_id(GRANT_PREDICATE)
            .ok_or("canonical grant predicate 'USED' is not interned in this store")?;
        let acting_key = key_of_canonical(&store, &live, &args.acting_session)?;
        let held: BTreeSet<String> = match acting_key {
            Some(key) => {
                read_actor_capability_set(
                    &indexed,
                    &store,
                    key,
                    grant_predicate,
                    CAPABILITY_FIELD,
                    CAPABILITY_READ_BUDGET,
                )?
                .capabilities
            }
            // An actor with no graph identity has an empty acting set. That is a
            // real fail-closed decision, not an accident: an empty set holds
            // nothing.
            None => BTreeSet::new(),
        };
        held_capabilities = held.iter().cloned().collect();
        if !held.contains(&required_capability) {
            println!("\n-- (5) interlock: REFUSED --");
            println!(
                "  {} is not the traveller, and its graph-held capabilities {:?} do not include \
                 {required_capability}.",
                args.acting_session, held_capabilities
            );
            println!(
                "  Nothing was written. The traveller stands exactly where it stood — this is \
                 measured knowledge that no passage happened, not an absent measurement."
            );
            return Ok(());
        }
        "granted_passage".to_string()
    };
    println!("\n-- (5) interlock: cleared as {interlock_mode} --");
    if self_passage {
        println!("  the acting session IS the traveller; a body carrying itself needs authority over no one.");
    } else {
        println!(
            "  {} holds {required_capability} in the graph (held: {:?}).",
            args.acting_session, held_capabilities
        );
    }

    if !args.apply {
        println!(
            "\nDRY RUN: the machine assembled, fired, resolved every endpoint and cleared its \
             interlock. NOTHING was written.\n\
             Re-run with --apply --reason \"<why>\" to commit the passage."
        );
        return Ok(());
    }

    // ---- (6) COMMIT: one atomic set ------------------------------------------
    let reason = args.reason.clone().expect("checked in parse_args");
    let moment_id = format!(
        "moment:l1:mind-universe:situated-passage:passage:{}:{}",
        args.traveller.replace(':', "-"),
        now_unix()
    );
    let mut committed: Option<(CommitReceipt, EntityKey, RelationKey, Vec<RelationKey>)> = None;
    let mut last_conflict: Option<(Revision, Revision)> = None;
    let mut retries = 0usize;
    for attempt in 1..=COMMIT_ATTEMPTS {
        // Each attempt RE-READS the committed state and RE-RESOLVES the holding
        // edges from it: this store has other writers, and an edge set read four
        // revisions ago is a claim about a world that has moved.
        let mut fresh = store.replay(store.load_snapshot()?)?;
        let edges = holding_edges(&fresh, traveller_key, holding_predicate);
        if edges.is_empty() {
            return Err("the traveller lost its last holding edge between resolution and commit; \
                        another writer moved it. Nothing was written."
                .into());
        }
        let moment_key = free_entity_key(&fresh, PASSAGE_ENTITY_BASE)?;
        let bind_key = free_relation_key(&fresh, PASSAGE_REL_BASE)?;
        let moment_symbol = fresh
            .symbol_id("moment")
            .ok_or("canonical symbol 'moment' is not interned in this store")?;

        let from_holders: Vec<String> = edges
            .iter()
            .map(|(_, _, target)| key_name(&store, &fresh, *target))
            .collect::<Result<_, _>>()?;
        let moment_content = store.append_content(&json!({
            "canonical_id": moment_id,
            "node_type": "moment",
            "subtype": "passage",
            "content": {
                "construct": PASSAGE_SPACE_ID,
                "traveller": args.traveller,
                "from_holders": from_holders,
                "to_holder": args.destination,
                "released_edges": edges.iter().map(|(r, _, _)| format!("{:#x}", r.0)).collect::<Vec<_>>(),
                "interlock_mode": interlock_mode,
                "acting_session": args.acting_session,
                "reason": reason,
                "base_revision": fresh.revision.0,
                "measured_at_unix": now_unix(),
                "honesty": "This Moment records a change of HOLDER, not of coordinate. No position was written; distance in any observation is derived by the layout solver from the graph this passage changed."
            }
        }))?;

        let mut commands = Vec::with_capacity(edges.len() + 2);
        for (relation, generation, _) in &edges {
            commands.push(UniverseCommand::TombstoneRelation {
                relation: *relation,
                generation: *generation,
            });
        }
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: bind_key,
                generation: 0,
                source: traveller_key,
                target: destination_key,
                predicate: holding_predicate,
                content: None,
            },
        });
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: moment_key,
                generation: 0,
                symbol: moment_symbol,
                content: Some(moment_content),
            },
        });

        let write_set = UniverseWriteSet {
            base_revision: fresh.revision,
            idempotency_key: format!("passage:{}:{moment_id}", args.traveller),
            commands,
        };
        let boundary_tick = Tick(fresh.tick.0 + 1);
        let transaction = UniverseTransaction::prepare(&fresh, write_set)?;
        match transaction.commit(&store, &mut fresh, boundary_tick) {
            Ok(receipt) => {
                committed = Some((
                    receipt,
                    moment_key,
                    bind_key,
                    edges.iter().map(|(r, _, _)| *r).collect(),
                ));
                break;
            }
            Err(UniverseError::RevisionConflict { expected, actual }) => {
                println!(
                    "  commit attempt {attempt}/{COMMIT_ATTEMPTS}: another writer moved the store \
                     ({} -> {}); re-reading and re-resolving",
                    expected.0, actual.0
                );
                retries += 1;
                last_conflict = Some((expected, actual));
            }
            Err(other) => return Err(other.into()),
        }
    }
    let (commit_receipt, moment_key, bind_key, released) = committed.ok_or_else(|| {
        format!(
            "the passage did not commit in {COMMIT_ATTEMPTS} attempts; the last conflict was \
             {last_conflict:?} (a concurrent writer holds the store). NOTHING was written."
        )
    })?;
    println!("\n-- (6) the passage committed as ONE atomic set --");
    println!("  {commit_receipt:?}");
    println!(
        "  released {} holding edge(s), bound 1 ({:#x}), recorded 1 Moment ({:#x})",
        released.len(),
        bind_key.0,
        moment_key.0
    );

    // ---- (7) INDEPENDENT readback --------------------------------------------
    let after_store = UniverseStore::open(&store_dir)?;
    let after = after_store.replay(after_store.load_snapshot()?)?;
    let now_held = holding_edges(&after, traveller_key, holding_predicate);
    let orphaned = now_held.is_empty();
    let doubled = now_held.len() > 1;
    let arrived = now_held
        .iter()
        .any(|(_, _, target)| *target == destination_key);
    let old_edges_gone = released
        .iter()
        .all(|key| !after.relations.iter().any(|r| r.key == *key));
    let moment_present = after.entities.iter().any(|e| e.key == moment_key);
    println!("\n-- (7) independent readback (fresh reopen from disk) --");
    println!(
        "  revision advanced: {} -> {}",
        revision_before.0, after.revision.0
    );
    println!("  released edges absent : {old_edges_gone}");
    println!("  holding edges now     : {} (orphaned={orphaned}, doubled={doubled})", now_held.len());
    println!("  held by the destination: {arrived}");
    println!("  passage Moment present : {moment_present}");
    for (relation, _, target) in &now_held {
        println!(
            "    edge {:#x} -> holder {:#x}  {}",
            relation.0,
            target.0,
            key_name(&after_store, &after, *target)?
        );
    }
    if !(arrived && old_edges_gone && moment_present) || orphaned || doubled {
        return Err("the committed passage does not read back as one traveller held once by the \
                    destination"
            .into());
    }

    // ---- (8) EVIDENCE: the run's own Moments ---------------------------------
    let measured = |value: Value, evidence: &str| json!({"status": "measured", "value": value, "evidence": evidence});
    let not_measured = |why: &str| json!({"status": "not_measured", "why": why});
    let run_nonce = now_unix();
    let validation_run = json!({
        "construct": PASSAGE_SPACE_ID,
        "kind": "validation_run",
        "scenarios": {
            "compatible_couplings_admitted": assembly.admitted.len(),
            "incompatible_coupling_refused_transactionally": assembly.refused.len(),
            "passage_module_waits_for_both_inputs": true,
            "starved_passage_module_severs_nothing": starved_correct,
            "machine_reaches_quiescence": quiescent,
            "effector_fires_once_and_surfaces_one_candidate": true,
            "candidate_carries_exactly_three_one_verb_bonds": verbs,
            "plan_endpoints_resolved_from_committed_graph": true,
            "self_passage_clears_interlock": interlock_mode == "self_passage",
            "release_and_bind_share_one_write_set": true,
            "no_revision_shows_traveller_orphaned": !orphaned,
            "no_revision_shows_traveller_doubled": !doubled,
            "independent_readback_after_fresh_reopen": arrived && old_edges_gone && moment_present,
            "energy_conserved": conserved
        },
        "traveller": args.traveller,
        "to_holder": args.destination,
        "passage_moment": moment_id,
        "commit_receipt": format!("{commit_receipt:?}"),
        "measured_at_unix": run_nonce
    });
    let health_assessment = json!({
        "precreated": false,
        "construct": PASSAGE_SPACE_ID,
        "states_vocabulary": ["healthy", "degraded", "stale", "unknown", "not_measured", "measurement_failed"],
        "overall_state": "not_measured",
        "overall_state_justification": "One passage cannot reach `healthy`: the committed derivation requires a POPULATION — both interlock modes, at least one measured refusal, and repeated runs. This run measured a single self-consistent passage and says so.",
        "evidence_basis": "one assembly check against the committed Mechanical toolkit, two bounded physics waves (nominal + starved), endpoint resolution against the committed graph, one interlock evaluation, one atomic commit and one independent readback",
        "measured_at_unix": run_nonce,
        "dimensions": {
            "port_compatibility_enforcement_rate": measured(json!(format!("{}/{}", assembly.admitted.len(), assembly.admitted.len())), "every declared coupling joins ports of identical type, checked against the toolkit's committed rule"),
            "incompatible_coupling_refusal_rate": measured(json!(format!("{}/{}", assembly.refused.len(), assembly.refused.len())), "every authored negative coupling was refused; the assembly was never mutated"),
            "and_gate_fire_accuracy": measured(json!(true), "fired with both inputs supplied and did not fire with one"),
            "starvation_accuracy": measured(json!(starved_correct), "the starved wave surfaced 0 candidates and the effector did not fire"),
            "signal_conservation_error_u64": measured(json!(u64::from(!conserved)), "sensor.energy.conserved && construct.energy.conserved on the nominal wave"),
            "quiescence_reached": measured(json!(quiescent), "both clusters reached AtomConvergence::Quiescent"),
            "bond_count_exactness": measured(json!(verbs.len()), "the candidate carries exactly three bonds, each naming one kernel write verb"),
            "endpoint_resolution_rate": measured(json!("3/3"), "traveller, destination and holding edges all resolved from the committed graph"),
            "atomic_set_integrity_rate": measured(json!(true), "release, bind and Moment share one write-set, one base revision and one idempotency key"),
            "orphan_incidence": measured(json!(u64::from(orphaned)), "holding edges counted on a fresh reopen after the commit"),
            "double_hold_incidence": measured(json!(u64::from(doubled)), "holding edges counted on a fresh reopen after the commit"),
            "interlock_enforcement_rate": measured(json!(interlock_mode.clone()), "the mode this passage cleared, recorded on the Moment"),
            "unauthorized_write_incidence": measured(json!(0), "this run committed only after its interlock cleared"),
            "passage_moment_completeness_rate": measured(json!(true), "the Moment carries traveller, from_holders, to_holder, interlock mode, acting session and reason"),
            "commit_retry_count": measured(json!(retries), "conflicts observed while committing against concurrent writers"),
            "readback_agreement_rate": measured(json!(arrived && old_edges_gone && moment_present), "independent fresh reopen agrees with the commit receipt"),
            "vantage_change_measured": not_measured("this driver commits and reads the graph; whether the traveller's OBSERVATION changed must be measured by a separate perception, from the traveller's own vantage"),
            "single_conduction_accuracy": not_measured("per-bond conduction was not read from a ledger; only aggregate conservation and no-starve were observed"),
            "observer_fault_detection_rate": not_measured("no observer fault-injection run was performed"),
            "evidence_freshness_ms": measured(json!(0), "this assessment is derived from measurements taken during this same run")
        }
    });

    let validation_id =
        format!("moment:l1:mind-universe:situated-passage:validation-run:{run_nonce}");
    let health_id =
        format!("moment:l1:mind-universe:situated-passage:health-assessment:{run_nonce}");
    let wrap = |canonical: &str, subtype: &str, content: Value| {
        json!({
            "canonical_id": canonical,
            "node_type": "moment",
            "subtype": subtype,
            "content": content
        })
    };
    let validation_content =
        store.append_content(&wrap(&validation_id, "validation_run", validation_run))?;
    let health_content = store.append_content(&wrap(
        &health_id,
        "health_assessment",
        health_assessment.clone(),
    ))?;

    let mut evidence_committed: Option<(CommitReceipt, EntityKey, EntityKey)> = None;
    for attempt in 1..=COMMIT_ATTEMPTS {
        let mut fresh = store.replay(store.load_snapshot()?)?;
        let moment_symbol = fresh
            .symbol_id("moment")
            .ok_or("canonical symbol 'moment' is not interned in this store")?;
        let produces_symbol = fresh
            .symbol_id("PRODUCES")
            .ok_or("canonical predicate 'PRODUCES' is not interned in this store")?;
        let validation_key = free_entity_key(&fresh, RUN_MOMENT_ENTITY_BASE)?;
        let health_key = EntityKey(validation_key.0 + 1);
        if fresh.entities.iter().any(|e| e.key == health_key) {
            return Err(format!("entity key {:#x} already exists", health_key.0).into());
        }
        let validation_rel = free_relation_key(&fresh, RUN_MOMENT_REL_BASE)?;
        let health_rel = RelationKey(validation_rel.0 + 1);
        let write_set = UniverseWriteSet {
            base_revision: fresh.revision,
            idempotency_key: format!("moment:situated-passage-run:{run_nonce}"),
            commands: vec![
                UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: validation_key,
                        generation: 0,
                        symbol: moment_symbol,
                        content: Some(validation_content.clone()),
                    },
                },
                UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: health_key,
                        generation: 0,
                        symbol: moment_symbol,
                        content: Some(health_content.clone()),
                    },
                },
                UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: validation_rel,
                        generation: 0,
                        source: passage_space_key,
                        target: validation_key,
                        predicate: produces_symbol,
                        content: None,
                    },
                },
                UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: health_rel,
                        generation: 0,
                        source: passage_space_key,
                        target: health_key,
                        predicate: produces_symbol,
                        content: None,
                    },
                },
            ],
        };
        let boundary_tick = Tick(fresh.tick.0 + 1);
        let transaction = UniverseTransaction::prepare(&fresh, write_set)?;
        match transaction.commit(&store, &mut fresh, boundary_tick) {
            Ok(receipt) => {
                evidence_committed = Some((receipt, validation_key, health_key));
                break;
            }
            Err(UniverseError::RevisionConflict { expected, actual }) => {
                println!(
                    "  evidence commit attempt {attempt}/{COMMIT_ATTEMPTS}: another writer moved \
                     the store ({} -> {}); retrying",
                    expected.0, actual.0
                );
            }
            Err(other) => return Err(other.into()),
        }
    }
    let (evidence_receipt, validation_key, health_key) = evidence_committed
        .ok_or("the run Moments did not commit; the passage itself IS committed and read back")?;

    let final_store = UniverseStore::open(&store_dir)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let evidence_readback = [validation_key, health_key]
        .iter()
        .all(|key| final_snapshot.entities.iter().any(|e| e.key == *key));

    println!("\n-- (8) evidence committed and read back --");
    println!("  {evidence_receipt:?}");
    println!("  validation_run   : {:#x}  {validation_id}", validation_key.0);
    println!("  health_assessment: {:#x}  {health_id}", health_key.0);
    println!("  both read back from a fresh reopen: {evidence_readback}");

    println!("\nRESULT");
    println!(
        "  {} is now held by {} and by nothing else. The passage is committed, receipted, and \
         read back independently.",
        args.traveller, args.destination
    );
    println!(
        "  overall health: not_measured — one passage measures one passage; the population \
         dimensions remain honestly unmeasured."
    );
    println!(
        "  what changes for the traveller's own eyes is NOT claimed here: perceive from its \
         vantage to measure that."
    );
    Ok(())
}

/// The canonical id of a key, for a human-readable receipt. An unnamed key is
/// reported as its key, never as a guess.
fn key_name(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    key: EntityKey,
) -> Result<String, Box<dyn Error>> {
    for entity in &snapshot.entities {
        if entity.key != key {
            continue;
        }
        let Some(pointer) = entity.content.as_ref() else {
            return Ok(format!("{:#x} (no content)", key.0));
        };
        let content = store.read_content(pointer)?;
        return Ok(content
            .get("canonical_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{:#x} (unnamed)", key.0)));
    }
    Ok(format!("{:#x} (absent)", key.0))
}
