//! Run the L1 Ollama inference probe FROM THE LIVE GRAPH, all the way to a real
//! local inference and two committed runtime Moments.
//!
//! This is the driver for `space:l1:mind-universe:ollama-inference-probe-v0`, the
//! construct the Mechanical toolkit produces at the citizen's inner scale. Every
//! variable is read from the COMMITTED store, never from the authoring fixture:
//!
//!   * the machine (modules, typed ports, couplings, atoms, bonds, effect
//!     binding) from the probe's `code` node;
//!   * the compatibility rule the couplings are checked against from the
//!     COMMITTED Mechanical toolkit (`space:l2:mind-universe:mechanical-toolkit-v0`);
//!   * the capability declaration + its limits from the probe's `code` node;
//!   * the transport endpoint, method, content type and timeout likewise;
//!   * the request payload bytes — model, prompt, decoding options — from the
//!     authored effect binding, transported VERBATIM.
//!
//! The only native policy here is mechanism with zero variables: an HTTP/1.1 byte
//! transport, and one evaluator per rig check named by the graph. A rig check the
//! graph names but this driver cannot evaluate is a HARD failure of the run — it
//! is never silently skipped and never reported as passing.
//!
//! What it measures (in this order, all against the live store):
//!   1. ASSEMBLY  — every authored coupling joins compatible ports; both authored
//!      `refused_couplings` are refused; couplings and bonds correspond 1:1.
//!   2. MACHINE   — the nominal wave: both sources supplied -> the AND-gate
//!      request module activates -> the trigger fires -> the terminal effector
//!      fires and surfaces exactly ONE candidate EffectIntent whose payload is
//!      byte-identical to the authored request.
//!   3. STARVATION— the negative wave: the model source is NOT supplied -> the
//!      request module does not activate, nothing is deposited, no candidate is
//!      surfaced. Starvation is measured, not assumed.
//!   4. CAPABILITY— an intent naming an UNDECLARED capability is denied by the
//!      graph-owned registry before any transport.
//!   5. TRANSPORT — the candidate is executed through the declared capability to
//!      the authored endpoint; the EffectReceipt carries the RAW wire response.
//!   6. RIG       — each check the graph names is evaluated against those bytes.
//!   7. EVIDENCE  — one `validation_run` Moment + one `health_assessment` Moment
//!      are committed through the real Commit phase, linked from the construct by
//!      PRODUCES, and read back from a fresh reopen.
//!
//! HONESTY. A transport failure (server down, timeout, non-200) is MEASURED
//! failure evidence, never an empty completion and never `not_measured`. Overall
//! health is never `healthy` from one run: the authored derivation requires a
//! population (availability and determinism rates), which a single call cannot
//! measure. Dimensions this run does not cover are emitted as `not_measured` with
//! the concrete reason.
//!
//! Usage: `ollama_probe_run [store-dir]`
//!   store-dir: UNIVERSE_STORE env, then arg 1, then
//!              artifacts/ontology-registry/current/store (the LIVE store).
//!   The run COMMITS two Moments + two PRODUCES edges into that store.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{env, path::PathBuf};

use serde_json::{json, Value};

use universe_capabilities::{
    CapabilityDeclaration, CapabilityHost, CapabilityRegistry, EffectAdapter,
    EffectExecutionReceipt, EffectIntent, EffectReceipt,
};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError};
use universe_e2e::construct_resolver::{resolve_construct, AlarmAtomCircuit, ResolvedConstruct};
use universe_physics::{AtomConvergence, AtomExecutionBudget};
use universe_store::{EntityRecord, RelationRecord, UniverseSnapshot, UniverseStore};
use universe_supervisor::{PhysicsDepositOutcome, Supervisor};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// The construct this driver runs, and the toolkit that produced it. Both are
/// read from the COMMITTED store; neither fixture file is opened.
const PROBE_CODE_ID: &str = "code:l1:mind-universe:ollama-inference-probe-v0";
const PROBE_SPACE_ID: &str = "space:l1:mind-universe:ollama-inference-probe-v0";
const TOOLKIT_ALGORITHM_ID: &str = "algorithm:l2:mind-universe:mechanical-toolkit-v0";
const TOOLKIT_SPACE_ID: &str = "space:l2:mind-universe:mechanical-toolkit-v0";

/// Key block for this construct's runtime Moments, disjoint from the injector's
/// hashed construct blocks (max 0x1000_0000) and the underground receipt block
/// (0x00E0_0000). Free keys are scanned inside the block, never assumed.
const MOMENT_ENTITY_BASE: u128 = 0x2000_0000;
const MOMENT_REL_BASE: u128 = 0x2100_0000;
const MOMENT_BLOCK_SPAN: u128 = 4096;

fn budget() -> AtomExecutionBudget {
    AtomExecutionBudget {
        max_atoms: 16,
        max_bonds: 16,
        max_steps: 16,
        max_total_energy: 10_000,
    }
}

// ===========================================================================
// Native floor: an HTTP/1.1 byte transport with zero policy.
// ===========================================================================

/// Transports the authored payload bytes verbatim and returns the RAW wire
/// response (status line + headers + body) as the measured result.
///
/// It selects no model, writes no prompt, sets no decoding parameter, parses
/// nothing and retries nothing: host, port, path, method, content type and
/// timeout all come from the construct's `effect_transport` block.
struct HttpByteTransport {
    host: String,
    port: u16,
    path: String,
    method: String,
    content_type: String,
    timeout: Duration,
}

impl HttpByteTransport {
    /// Parses `http://host:port/path`. Only plain HTTP is accepted: a scheme this
    /// transport cannot actually speak is a hard error, never a silent downgrade.
    fn from_endpoint(
        endpoint: &str,
        method: &str,
        content_type: &str,
        timeout_ms: u64,
    ) -> Result<Self, Box<dyn Error>> {
        let rest = endpoint
            .strip_prefix("http://")
            .ok_or_else(|| format!("unsupported endpoint scheme (http:// only): {endpoint}"))?;
        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse::<u16>()?),
            None => (authority.to_string(), 80u16),
        };
        Ok(Self {
            host,
            port,
            path: path.to_string(),
            method: method.to_string(),
            content_type: content_type.to_string(),
            timeout: Duration::from_millis(timeout_ms),
        })
    }
}

impl EffectAdapter for HttpByteTransport {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let authority = format!("{}:{}", self.host, self.port);
        let address = authority
            .to_socket_addrs()
            .map_err(|error| format!("endpoint did not resolve: {error}"))?
            .next()
            .ok_or_else(|| format!("endpoint resolved to no address: {authority}"))?;
        let mut stream = TcpStream::connect_timeout(&address, self.timeout)
            .map_err(|error| format!("connect failed: {error}"))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|error| format!("read timeout not set: {error}"))?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(|error| format!("write timeout not set: {error}"))?;
        let head = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.method,
            self.path,
            authority,
            self.content_type,
            payload.len()
        );
        stream
            .write_all(head.as_bytes())
            .and_then(|()| stream.write_all(payload))
            .and_then(|()| stream.flush())
            .map_err(|error| format!("request write failed: {error}"))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|error| format!("response read failed: {error}"))?;
        if response.is_empty() {
            return Err("server closed the connection with no response bytes".to_string());
        }
        Ok(response)
    }
}

/// Splits a raw HTTP response into (status code, body). Absent/!unparseable
/// pieces are `None` — never defaulted to 200 or to an empty body.
fn split_http(raw: &[u8]) -> (Option<u16>, Option<&[u8]>) {
    let text = String::from_utf8_lossy(raw);
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());
    let body = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| &raw[index + 4..]);
    (status, body)
}

// ===========================================================================
// Bounded, targeted read of the committed graph.
// ===========================================================================

/// Hydrates the content of committed entities until every requested
/// `canonical_id` is found. Bounded by the snapshot's own entity count; a missing
/// id is a hard error, never an invented default.
fn read_nodes(
    supervisor: &Supervisor,
    snapshot: &UniverseSnapshot,
    ids: &[&str],
) -> Result<BTreeMap<String, (EntityKey, Value)>, Box<dyn Error>> {
    let wanted: BTreeSet<&str> = ids.iter().copied().collect();
    let mut found: BTreeMap<String, (EntityKey, Value)> = BTreeMap::new();
    let mut hydrations = 0usize;
    for entity in &snapshot.entities {
        if found.len() == wanted.len() {
            break;
        }
        let Some(content_ref) = entity.content.as_ref() else {
            continue;
        };
        hydrations += 1;
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
            return Err(format!(
                "canonical id {id} is not committed in this store (hydrated {hydrations} nodes)"
            )
            .into());
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

// ===========================================================================
// Assembly: the Mechanical toolkit's compatibility rule, applied.
// ===========================================================================

#[derive(Debug)]
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
    let couplings = circuit
        .get("couplings")
        .and_then(Value::as_array)
        .ok_or("machine_circuit has no couplings array")?;
    let refused_couplings = circuit
        .get("refused_couplings")
        .and_then(Value::as_array)
        .ok_or("machine_circuit has no refused_couplings array")?;
    let bonds = circuit
        .get("bonds")
        .and_then(Value::as_array)
        .ok_or("machine_circuit has no bonds array")?;

    let types = |coupling: &Value| -> Result<(String, String, String), Box<dyn Error>> {
        let key = coupling
            .get("key")
            .and_then(Value::as_str)
            .ok_or("coupling without key")?
            .to_string();
        let out_type = coupling
            .get("out_type")
            .and_then(Value::as_str)
            .ok_or("coupling without out_type")?
            .to_string();
        let in_type = coupling
            .get("in_type")
            .and_then(Value::as_str)
            .ok_or("coupling without in_type")?
            .to_string();
        Ok((key, out_type, in_type))
    };

    let mut admitted = Vec::new();
    let mut wrongly_refused = Vec::new();
    for coupling in couplings {
        let (key, out_type, in_type) = types(coupling)?;
        if admit(&out_type, &in_type) {
            admitted.push(format!("{key} ({out_type} -> {in_type})"));
        } else {
            wrongly_refused.push(format!("{key} ({out_type} -> {in_type})"));
        }
    }
    let mut refused = Vec::new();
    let mut wrongly_admitted = Vec::new();
    for coupling in refused_couplings {
        let (key, out_type, in_type) = types(coupling)?;
        if admit(&out_type, &in_type) {
            wrongly_admitted.push(format!("{key} ({out_type} -> {in_type})"));
        } else {
            refused.push(format!("{key} ({out_type} != {in_type})"));
        }
    }

    // Every admitted coupling must be physicalized by exactly one bond of the
    // same key, and no bond may exist without its declared coupling.
    let coupling_keys: BTreeSet<String> = couplings
        .iter()
        .filter_map(|c| c.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();
    let bond_keys: BTreeSet<String> = bonds
        .iter()
        .filter_map(|b| b.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();

    Ok(AssemblyEvidence {
        rule: toolkit_rule.to_string(),
        admitted,
        refused,
        wrongly_admitted,
        wrongly_refused,
        coupling_bond_correspondence: coupling_keys == bond_keys,
    })
}

// ===========================================================================
// The validation rig: one evaluator per graph-named check.
// ===========================================================================

#[derive(Clone, Debug)]
struct RigResult {
    id: String,
    load_bearing: bool,
    passed: bool,
    evidence: String,
}

/// Evaluates every check the graph names. An id with no evaluator is a hard
/// error: the rig never skips a declared check and never reports it as passing.
fn run_rig(
    rig: &Value,
    requested_model: &str,
    receipt: &EffectExecutionReceipt,
) -> Result<Vec<RigResult>, Box<dyn Error>> {
    let checks = rig
        .get("checks")
        .and_then(Value::as_array)
        .ok_or("validation_rig has no checks array")?;
    let verbs: Vec<String> = rig
        .get("offered_verbs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_lowercase))
                .collect()
        })
        .unwrap_or_default();

    let raw = match &receipt.outcome {
        EffectReceipt::TransportSucceeded { response } => Some(response.clone()),
        EffectReceipt::TransportFailed { .. } => None,
    };
    let (status, body) = match raw.as_ref() {
        Some(bytes) => {
            let (status, body) = split_http(bytes);
            (status, body.map(|b| b.to_vec()))
        }
        None => (None, None),
    };
    let parsed: Option<Value> = body
        .as_ref()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .filter(|value| value.is_object());
    let completion = parsed
        .as_ref()
        .and_then(|v| v.get("response"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let absent = |what: &str| format!("known_absent: {what} (the transport returned no usable bytes)");

    let mut results = Vec::new();
    for check in checks {
        let id = check
            .get("id")
            .and_then(Value::as_str)
            .ok_or("rig check without id")?
            .to_string();
        let load_bearing = check
            .get("load_bearing")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let (passed, evidence) = match id.as_str() {
            "transport_succeeded" => match &receipt.outcome {
                EffectReceipt::TransportSucceeded { response } => {
                    (true, format!("TransportSucceeded, {} raw wire bytes", response.len()))
                }
                EffectReceipt::TransportFailed { reason } => {
                    (false, format!("TransportFailed: {reason}"))
                }
            },
            "http_status_200" => match status {
                Some(code) => (code == 200, format!("status line reported {code}")),
                None => (false, absent("no status line")),
            },
            "body_is_json" => match (&body, &parsed) {
                (Some(bytes), Some(_)) => (true, format!("body parsed as a JSON object ({} bytes)", bytes.len())),
                (Some(bytes), None) => (
                    false,
                    format!("body of {} bytes did not parse as a JSON object", bytes.len()),
                ),
                _ => (false, absent("no body")),
            },
            "model_echo_matches" => match parsed.as_ref().and_then(|v| v.get("model")).and_then(Value::as_str) {
                Some(answered) => (
                    answered == requested_model,
                    format!("requested {requested_model}, answered by {answered}"),
                ),
                None => (false, absent("no response.model")),
            },
            "completion_nonempty" => match parsed.as_ref().and_then(|v| v.get("response")).and_then(Value::as_str) {
                Some(text) => (
                    !text.trim().is_empty(),
                    format!("completion of {} chars", text.chars().count()),
                ),
                None => (false, absent("no response.response")),
            },
            "run_done" => match parsed.as_ref().and_then(|v| v.get("done")).and_then(Value::as_bool) {
                Some(done) => (done, format!("response.done = {done}")),
                None => (false, absent("no response.done")),
            },
            "tokens_generated" => match parsed.as_ref().and_then(|v| v.get("eval_count")).and_then(Value::as_u64) {
                Some(count) => (count > 0, format!("response.eval_count = {count}")),
                None => (false, absent("no response.eval_count")),
            },
            "verb_choice_parseable" => {
                if parsed.is_none() {
                    (false, absent("no completion to parse"))
                } else {
                    let lowered = completion.to_lowercase();
                    let hits: Vec<&String> = verbs
                        .iter()
                        .filter(|verb| {
                            lowered
                                .split(|c: char| !c.is_alphanumeric())
                                .any(|word| word == verb.as_str())
                        })
                        .collect();
                    (
                        hits.len() == 1,
                        format!(
                            "offered {:?}; matched {:?} in completion {:?}",
                            verbs,
                            hits,
                            completion.trim()
                        ),
                    )
                }
            }
            other => {
                return Err(format!(
                    "validation rig names check {other:?} but this driver has no evaluator for it — \
                     the run fails closed rather than skipping or passing a declared check"
                )
                .into())
            }
        };
        results.push(RigResult {
            id,
            load_bearing,
            passed,
            evidence,
        });
    }
    Ok(results)
}

// ===========================================================================
// Commit plumbing.
// ===========================================================================

/// First free key at or after `base`, bounded by `MOMENT_BLOCK_SPAN`.
fn free_entity_key(snapshot: &UniverseSnapshot, base: u128) -> Result<EntityKey, Box<dyn Error>> {
    for offset in 0..MOMENT_BLOCK_SPAN {
        let key = EntityKey(base + offset);
        if !snapshot.entities.iter().any(|entity| entity.key == key) {
            return Ok(key);
        }
    }
    Err(format!("no free entity key in the block at {base:#x}").into())
}

fn free_relation_key(snapshot: &UniverseSnapshot, base: u128) -> Result<RelationKey, Box<dyn Error>> {
    for offset in 0..MOMENT_BLOCK_SPAN {
        let key = RelationKey(base + offset);
        if !snapshot.relations.iter().any(|relation| relation.key == key) {
            return Ok(key);
        }
    }
    Err(format!("no free relation key in the block at {base:#x}").into())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("OLLAMA PROBE RUN FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir = env::var_os("UNIVERSE_STORE")
        .map(PathBuf::from)
        .or_else(|| env::args_os().nth(1).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    let genesis = PathBuf::from("fixtures/genesis/minimal-genesis.json");
    println!("store dir: {}", store_dir.display());

    let mut supervisor = Supervisor::boot(&store_dir, &genesis)?;
    let revision_before = supervisor.revision();
    println!(
        "base revision: {} | entities: {} | relations: {}",
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
            PROBE_CODE_ID,
            PROBE_SPACE_ID,
            TOOLKIT_ALGORITHM_ID,
            TOOLKIT_SPACE_ID,
        ],
    )?;
    let (probe_code_key, probe_code_wrapper) = nodes[PROBE_CODE_ID].clone();
    let (probe_space_key, _) = nodes[PROBE_SPACE_ID].clone();
    let (toolkit_algorithm_key, toolkit_algorithm_wrapper) = nodes[TOOLKIT_ALGORITHM_ID].clone();
    let probe_code = inner(&probe_code_wrapper)?.clone();
    let toolkit_algorithm = inner(&toolkit_algorithm_wrapper)?.clone();
    println!(
        "read from the live graph: probe code {:#x}, mechanical toolkit algorithm {:#x}",
        probe_code_key.0, toolkit_algorithm_key.0
    );

    let circuit_value = probe_code
        .get("machine_circuit")
        .ok_or("probe code node carries no machine_circuit")?
        .clone();
    let transport_spec = probe_code
        .get("effect_transport")
        .ok_or("probe code node carries no effect_transport")?
        .clone();
    let declaration_spec = probe_code
        .get("capability_declaration")
        .ok_or("probe code node carries no capability_declaration")?
        .clone();
    let rig_spec = probe_code
        .get("validation_rig")
        .ok_or("probe code node carries no validation_rig")?
        .clone();

    // ---- (1) ASSEMBLY: the toolkit's own compatibility rule, applied ----------
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
    let circuit: AlarmAtomCircuit = serde_json::from_value(circuit_value.clone())?;
    let resolved: ResolvedConstruct = resolve_construct(&circuit)
        .map_err(|error| format!("resolve_construct failed: {error:?}"))?;
    let nominal: PhysicsDepositOutcome = supervisor.run_physics_deposit_phase(
        resolved.sensor_cluster.clone(),
        &resolved.deposit_bindings,
        resolved.construct_cluster.clone(),
        &resolved.effect_bindings,
        budget(),
    )?;
    let key_of = |name: &str| -> Result<EntityKey, Box<dyn Error>> {
        resolved
            .atom_keys
            .get(name)
            .copied()
            .ok_or_else(|| format!("authored circuit has no atom {name}").into())
    };
    let effector = key_of("inference_effector")?;
    let probe_trigger = key_of("inference_probe")?;
    println!("\n-- (2) the machine ran (nominal wave: both sources supplied) --");
    println!(
        "  sensor {:?} / construct {:?}; energy conserved: {} / {}",
        nominal.sensor.convergence,
        nominal.construct.convergence,
        nominal.sensor.energy.conserved,
        nominal.construct.energy.conserved
    );
    if !nominal.fired_construct_atoms.contains(&probe_trigger)
        || !nominal.fired_construct_atoms.contains(&effector)
    {
        return Err("the trigger and the terminal effector did not both fire".into());
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
        probe_trigger.0, effector.0
    );

    // The candidate must carry the AUTHORED request bytes, unchanged.
    let authored_message = circuit_value
        .pointer("/effect_bindings/0/message")
        .and_then(Value::as_str)
        .ok_or("effect binding has no message")?
        .to_string();
    let payload_fidelity = candidate.payload == authored_message.as_bytes();
    if !payload_fidelity {
        return Err("the surfaced candidate payload differs from the authored request bytes".into());
    }
    let requested_model = serde_json::from_str::<Value>(&authored_message)
        .ok()
        .and_then(|value| value.get("model").and_then(Value::as_str).map(str::to_string))
        .ok_or("the authored request payload names no model")?;
    println!(
        "  candidate payload is byte-identical to the authored request ({} bytes, model {requested_model})",
        candidate.payload.len()
    );

    // ---- (3) STARVATION: the negative wave -----------------------------------
    let mut starved_circuit: AlarmAtomCircuit = serde_json::from_value(circuit_value.clone())?;
    starved_circuit
        .external_measured_injections
        .remove("model_selected");
    let starved_resolved = resolve_construct(&starved_circuit)
        .map_err(|error| format!("resolve_construct (starved) failed: {error:?}"))?;
    let starved = supervisor.run_physics_deposit_phase(
        starved_resolved.sensor_cluster.clone(),
        &starved_resolved.deposit_bindings,
        starved_resolved.construct_cluster.clone(),
        &starved_resolved.effect_bindings,
        budget(),
    )?;
    let starved_correct = starved.candidate_effects.is_empty()
        && !starved.fired_construct_atoms.contains(&effector);
    println!("\n-- (3) starvation (negative wave: the model source is NOT supplied) --");
    println!(
        "  candidates surfaced: {} | effector fired: {}",
        starved.candidate_effects.len(),
        starved.fired_construct_atoms.contains(&effector)
    );
    if !starved_correct {
        return Err("the AND-gate request module produced a request from one supplied input".into());
    }
    println!("  the AND gate starved: no request assembled, nothing transported");

    // ---- (4) CAPABILITY: the graph-owned registry, enforced -------------------
    let declaration: CapabilityDeclaration = serde_json::from_value(declaration_spec.clone())?;
    let declared_capability = declaration.capability.clone();
    let mut declarations = BTreeMap::new();
    declarations.insert(declared_capability.clone(), declaration.clone());
    let registry = CapabilityRegistry {
        version: format!("graph:{PROBE_CODE_ID}"),
        declarations,
    };
    let endpoint = transport_spec
        .get("endpoint")
        .and_then(Value::as_str)
        .ok_or("effect_transport declares no endpoint")?;
    let method = transport_spec
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("POST");
    let content_type = transport_spec
        .get("content_type")
        .and_then(Value::as_str)
        .unwrap_or("application/json");
    let timeout_ms = transport_spec
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .ok_or("effect_transport declares no timeout_ms")?;
    let mut host = CapabilityHost::default().with_registry(registry);
    host.register(
        declared_capability.clone(),
        Box::new(HttpByteTransport::from_endpoint(
            endpoint,
            method,
            content_type,
            timeout_ms,
        )?),
    );
    // A capability the graph never declared must be denied BEFORE any transport.
    let undeclared = EffectIntent {
        capability: "infer.undeclared-probe".to_string(),
        idempotency_key: format!("ollama-probe:undeclared:{}", now_ms()),
        payload: candidate.payload.clone(),
        deadline_tick: candidate.deadline_tick,
        causal_ancestry: candidate.causal_ancestry.clone(),
    };
    let undeclared_denied = matches!(
        host.execute_measured(supervisor.tick(), &undeclared),
        Err(UniverseError::CapabilityDenied(_))
    );
    println!("\n-- (4) the graph-owned capability registry --");
    println!(
        "  declared {declared_capability} (max_payload_bytes {:?}, max_causal_depth {:?}); undeclared capability denied before transport: {undeclared_denied}",
        declaration.max_payload_bytes, declaration.max_causal_depth
    );
    if !undeclared_denied {
        return Err("an undeclared capability was not denied by the graph-owned registry".into());
    }

    // ---- (5) TRANSPORT: the real local inference -----------------------------
    println!("\n-- (5) transport: POST {endpoint} (timeout {timeout_ms} ms) --");
    let measured_at_ms = now_ms();
    let started = Instant::now();
    let exec_receipt = host.execute_measured(supervisor.tick(), &candidate)?;
    let latency_ms = started.elapsed().as_millis();
    supervisor.observe_transport_receipt(
        exec_receipt.capability.clone(),
        exec_receipt.idempotency_key.clone(),
        &exec_receipt.outcome,
    );
    let transport_succeeded = matches!(exec_receipt.outcome, EffectReceipt::TransportSucceeded { .. });
    println!(
        "  transport_attempted: {} | outcome: {} | latency {latency_ms} ms",
        exec_receipt.transport_attempted,
        if transport_succeeded { "TransportSucceeded" } else { "TransportFailed" }
    );

    // ---- (6) RIG: every check the graph names --------------------------------
    let rig = run_rig(&rig_spec, &requested_model, &exec_receipt)?;
    println!("\n-- (6) validation rig ({} checks, all evaluated) --", rig.len());
    for result in &rig {
        println!(
            "  {} {:<24} {}",
            if result.passed { "PASS" } else { "FAIL" },
            result.id,
            result.evidence
        );
    }
    let failures: Vec<&RigResult> = rig.iter().filter(|r| !r.passed).collect();
    let load_bearing_failures: Vec<&RigResult> =
        failures.iter().copied().filter(|r| r.load_bearing).collect();

    // ---- (7) EVIDENCE: two Moments committed and read back -------------------
    let run_nonce = format!("{}", measured_at_ms);
    let quiescent = nominal.sensor.convergence == AtomConvergence::Quiescent
        && nominal.construct.convergence == AtomConvergence::Quiescent;
    let energy_conserved = nominal.sensor.energy.conserved && nominal.construct.energy.conserved;
    let conservation_error: i64 = if energy_conserved { 0 } else { -1 };
    let outcome_label = if transport_succeeded {
        "TransportSucceeded"
    } else {
        "TransportFailed"
    };
    let transport_failure_note = if transport_succeeded {
        "the transport succeeded this run, so the failure-honesty path was not exercised"
    } else {
        "exercised: the transport failed and every dependent check is recorded as measured failure with known_absent evidence"
    };
    let rig_results: Vec<Value> = rig
        .iter()
        .map(|r| {
            let result = if r.passed { "measured_pass" } else { "measured_fail" };
            json!({
                "check": r.id,
                "load_bearing": r.load_bearing,
                "result": result,
                "evidence": r.evidence
            })
        })
        .collect();

    let validation_run = json!({
        "precreated": false,
        "runner": "ollama_probe_run — one assembly check, two physics waves, one authorized transport",
        "construct": PROBE_SPACE_ID,
        "produced_by_toolkit": TOOLKIT_SPACE_ID,
        "read_from": "the COMMITTED live store (machine, capability declaration, transport and rig all hydrated from graph content; no fixture file was opened)",
        "measured_at_unix_ms": measured_at_ms,
        "scenarios_exercised": {
            "compatible_couplings_admitted": assembly.admitted,
            "incompatible_coupling_refused_transactionally": assembly.refused,
            "request_module_waits_for_both_inputs": true,
            "starved_request_module_assembles_nothing": starved_correct,
            "machine_reaches_quiescence": quiescent,
            "effector_fires_once_and_surfaces_one_candidate": nominal.candidate_effects.len() == 1,
            "candidate_carries_authored_payload_unchanged": payload_fidelity,
            "undeclared_capability_denied": undeclared_denied,
            "transport_returns_effect_receipt": exec_receipt.transport_attempted,
            "energy_conserved": energy_conserved
        },
        "scenarios_not_run": [
            { "id": "oversized_payload_fails_before_transport", "why": "the authored payload is within the declared limit; no oversized variant was issued this run" },
            { "id": "rig_check_without_evaluator_fails_the_run", "why": "every check the graph names has an evaluator; the fail-closed branch was not exercised" },
            { "id": "transport_failure_recorded_as_failure_not_absence", "why": transport_failure_note }
        ],
        "effect_receipt": {
            "capability": exec_receipt.capability,
            "idempotency_key": exec_receipt.idempotency_key,
            "transport_attempted": exec_receipt.transport_attempted,
            "outcome": outcome_label,
            "endpoint": endpoint,
            "latency_ms": latency_ms
        },
        "rig_results": rig_results,
        "physics_evidence": {
            "sensor_convergence": format!("{:?}", nominal.sensor.convergence),
            "construct_convergence": format!("{:?}", nominal.construct.convergence),
            "fired_construct_atoms": nominal.fired_construct_atoms.iter().map(|k| format!("{:#x}", k.0)).collect::<Vec<_>>(),
            "terminal_starved": nominal.construct.terminal_starved.iter().map(|k| format!("{:#x}", k.0)).collect::<Vec<_>>(),
            "starved_wave_candidates": starved.candidate_effects.len()
        }
    });

    let not_measured = |why: &str| json!({ "status": "not_measured", "why": why });
    let measured = |value: Value, evidence: &str| json!({ "status": "measured", "value": value, "evidence": evidence });
    let overall_state = if failures.is_empty() { "not_measured" } else { "degraded" };
    let overall_justification = if failures.is_empty() {
        "Every load-bearing rig check passed on THIS run, but the authored derivation reserves `healthy` for a POPULATION of runs: availability over time and determinism are not measurable from a single call. The honest overall state is therefore not_measured, never healthy."
            .to_string()
    } else {
        format!(
            "Fresh evidence exists and {} check(s) failed ({} of them load-bearing): {}. The state is degraded on measured failure — never not_measured, which would hide known failure evidence.",
            failures.len(),
            load_bearing_failures.len(),
            failures.iter().map(|r| r.id.clone()).collect::<Vec<_>>().join(", ")
        )
    };
    let transport_failure_honesty = if transport_succeeded {
        not_measured("the transport succeeded this run, so the failure-honesty path was not exercised")
    } else {
        measured(
            json!(true),
            "the failure was recorded as measured failure with known_absent evidence on every dependent check, never as an empty completion",
        )
    };
    let rig_result = |id: &str| rig.iter().find(|r| r.id == id).map(|r| r.passed);
    let dimension_from_rig = |id: &str, evidence: &str| match rig_result(id) {
        Some(passed) => measured(json!(passed), evidence),
        None => not_measured("the rig did not name this check"),
    };

    let health_assessment = json!({
        "precreated": false,
        "construct": PROBE_SPACE_ID,
        "states_vocabulary": ["healthy", "degraded", "stale", "unknown", "not_measured", "measurement_failed"],
        "overall_state": overall_state,
        "overall_state_justification": overall_justification,
        "evidence_basis": "one assembly check against the committed Mechanical toolkit, two bounded physics waves (nominal + starved), one denied undeclared capability, one authorized transport to the local inference server, and every rig check the graph names",
        "measured_at_unix_ms": measured_at_ms,
        "dimensions": {
            "port_compatibility_enforcement_rate": measured(
                json!(format!("{}/{}", assembly.admitted.len(), assembly.admitted.len())),
                "every declared coupling joins ports of identical type, checked against the toolkit's committed rule"),
            "incompatible_coupling_refusal_rate": measured(
                json!(format!("{}/{}", assembly.refused.len(), assembly.refused.len())),
                "every authored negative coupling was refused; the assembly was never mutated"),
            "and_gate_fire_accuracy": measured(
                json!(true),
                "fired with both inputs supplied (nominal wave) and did not fire with one (starved wave)"),
            "starvation_accuracy": measured(
                json!(starved_correct),
                "the starved wave surfaced 0 candidates and the effector did not fire"),
            "signal_conservation_error_u64": measured(
                json!(conservation_error),
                "sensor.energy.conserved && construct.energy.conserved on the nominal wave"),
            "quiescence_reached": measured(json!(quiescent), "both clusters reached AtomConvergence::Quiescent"),
            "candidate_payload_fidelity": measured(
                json!(payload_fidelity),
                "the surfaced candidate payload is byte-identical to the authored request"),
            "capability_declaration_enforced": measured(
                json!(undeclared_denied),
                "an undeclared capability was denied by the graph-owned registry before any transport"),
            "effect_receipt_backed_rate": measured(
                json!(exec_receipt.transport_attempted),
                "the candidate was executed through the declared capability and returned an EffectExecutionReceipt"),
            "transport_latency_ms": measured(json!(latency_ms as u64), "wall clock around execute_measured (includes model load on a cold server)"),
            "response_wellformed_rate": dimension_from_rig("body_is_json", "rig check body_is_json"),
            "model_echo_match_rate": dimension_from_rig("model_echo_matches", "rig check model_echo_matches"),
            "completion_nonempty_rate": dimension_from_rig("completion_nonempty", "rig check completion_nonempty"),
            "tokens_generated": dimension_from_rig("tokens_generated", "rig check tokens_generated"),
            "verb_choice_parseable_rate": dimension_from_rig("verb_choice_parseable", "rig check verb_choice_parseable"),
            "rig_check_coverage": measured(
                json!(format!("{}/{}", rig.len(), rig.len())),
                "every check named by the graph was evaluated; an unknown check id fails the run"),
            "transport_failure_honesty_rate": transport_failure_honesty,
            "single_conduction_accuracy": not_measured(
                "per-bond conduction was not read from a ledger; only aggregate conservation and no-starve were observed"),
            "availability_over_time_rate": not_measured(
                "one call measures one moment; an availability rate needs a population of runs over time"),
            "determinism_rate": not_measured(
                "one call cannot measure run-to-run stability, and the heartbeat is explicitly non-deterministic"),
            "observer_fault_detection_rate": not_measured(
                "no observer fault-injection run was performed"),
            "evidence_freshness_ms": measured(json!(0), "this assessment is derived from measurements taken during this same run")
        }
    });

    // Commit both Moments + their PRODUCES edges as ONE transaction.
    //
    // The commit re-reads the store from disk immediately before preparing, and
    // commits against THAT snapshot: this store has other writers (the MCP
    // observer/injector session), so the supervisor's boot-time snapshot may be
    // stale by now. Re-reading is the honest fix — never widening the base
    // revision or retrying blind over someone else's committed state.
    let store = UniverseStore::open(&store_dir)?;
    let validation_id = format!(
        "moment:l1:mind-universe:ollama-inference-probe:validation-run:{run_nonce}"
    );
    let health_id = format!(
        "moment:l1:mind-universe:ollama-inference-probe:health-assessment:{run_nonce}"
    );
    let wrap = |canonical: &str, subtype: &str, content: Value| {
        json!({
            "canonical_id": canonical,
            "node_type": "moment",
            "subtype": subtype,
            "content": content
        })
    };
    let validation_content = store.append_content(&wrap(
        &validation_id,
        "validation_run",
        validation_run.clone(),
    ))?;
    let health_content = store.append_content(&wrap(
        &health_id,
        "health_assessment",
        health_assessment.clone(),
    ))?;

    // Optimistic commit against the CURRENT committed state, with bounded
    // retries. This store has another writer (the MCP observer/injector
    // session), so the base revision can move between reading it and committing.
    // Each attempt RE-READS the committed state and re-derives its symbols and
    // free keys from it: the base revision is never widened, and no other
    // writer's committed state is overwritten or replayed over.
    const COMMIT_ATTEMPTS: usize = 4;
    let mut committed: Option<(CommitReceipt, EntityKey, EntityKey, u32)> = None;
    let mut last_conflict: Option<(Revision, Revision)> = None;
    for attempt in 1..=COMMIT_ATTEMPTS {
        let mut live = store.replay(store.load_snapshot()?)?;
        let moment_symbol = live
            .symbol_id("moment")
            .ok_or("canonical symbol 'moment' is not interned in this store")?;
        let produces_symbol = live
            .symbol_id("PRODUCES")
            .ok_or("canonical predicate 'PRODUCES' is not interned in this store")?;
        let validation_key = free_entity_key(&live, MOMENT_ENTITY_BASE)?;
        let health_key = EntityKey(validation_key.0 + 1);
        if live.entities.iter().any(|entity| entity.key == health_key) {
            return Err(format!("entity key {:#x} already exists", health_key.0).into());
        }
        let validation_rel = free_relation_key(&live, MOMENT_REL_BASE)?;
        let health_rel = RelationKey(validation_rel.0 + 1);
        let write_set = UniverseWriteSet {
            base_revision: live.revision,
            idempotency_key: format!("moment:ollama-probe-run:{run_nonce}"),
            causal_ancestry: vec![
                "ollama-probe:request-assembled".to_string(),
                exec_receipt.idempotency_key.clone(),
                format!("construct:{PROBE_SPACE_ID}:run"),
            ],
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
                        source: probe_space_key,
                        target: validation_key,
                        predicate: produces_symbol,
                        content: None,
                    },
                },
                UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: health_rel,
                        generation: 0,
                        source: probe_space_key,
                        target: health_key,
                        predicate: produces_symbol,
                        content: None,
                    },
                },
            ],
        };
        let boundary_tick = Tick(live.tick.0 + 1);
        let transaction = UniverseTransaction::prepare(&live, write_set)?;
        match transaction.commit(&store, &mut live, boundary_tick) {
            Ok(receipt) => {
                committed = Some((receipt, validation_key, health_key, produces_symbol));
                break;
            }
            Err(UniverseError::RevisionConflict { expected, actual }) => {
                println!(
                    "  commit attempt {attempt}/{COMMIT_ATTEMPTS}: another writer moved the store \
                     ({} -> {}); re-reading the committed state and retrying",
                    expected.0, actual.0
                );
                last_conflict = Some((expected, actual));
            }
            Err(other) => return Err(other.into()),
        }
    }
    let (commit_receipt, validation_key, health_key, produces_symbol) =
        committed.ok_or_else(|| {
            format!(
                "the run Moments did not commit in {COMMIT_ATTEMPTS} attempts; \
                 the last conflict was {last_conflict:?} (a concurrent writer holds the store)"
            )
        })?;

    // INDEPENDENT readback: a fresh reopen from disk, not the handle we wrote with.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    let revision_after: Revision = after.revision;
    for (key, id) in [(validation_key, &validation_id), (health_key, &health_id)] {
        let entity = after
            .entities
            .iter()
            .find(|entity| entity.key == key)
            .ok_or_else(|| format!("Moment {id} absent on independent readback"))?;
        let wrapper = fresh.read_content(
            entity
                .content
                .as_ref()
                .ok_or_else(|| format!("Moment {id} has no content on readback"))?,
        )?;
        if wrapper.get("canonical_id").and_then(Value::as_str) != Some(id.as_str()) {
            return Err(format!("canonical_id mismatch on readback for {:#x}", key.0).into());
        }
    }
    let produces_present = after
        .relations
        .iter()
        .filter(|relation| {
            relation.predicate == produces_symbol
                && relation.source == probe_space_key
                && (relation.target == validation_key || relation.target == health_key)
        })
        .count();

    println!("\n-- (7) evidence committed and read back --");
    println!("  commit receipt   : {commit_receipt:?}");
    println!(
        "  revision advanced: {} -> {}",
        revision_before.0, revision_after.0
    );
    println!("  validation_run   : {:#x}  {validation_id}", validation_key.0);
    println!("  health_assessment: {:#x}  {health_id}", health_key.0);
    println!("  PRODUCES edges from the construct read back: {produces_present}/2");
    if produces_present != 2 {
        return Err("the PRODUCES edges from the construct did not read back".into());
    }

    println!("\n-- health assessment (committed) --");
    println!("{}", serde_json::to_string_pretty(&health_assessment)?);

    let measured_dimensions = health_assessment["dimensions"]
        .as_object()
        .map(|dims| {
            dims.values()
                .filter(|v| v.get("status").and_then(Value::as_str) == Some("measured"))
                .count()
        })
        .unwrap_or(0);
    let unmeasured_dimensions = health_assessment["dimensions"]
        .as_object()
        .map(|dims| {
            dims.values()
                .filter(|v| v.get("status").and_then(Value::as_str) == Some("not_measured"))
                .count()
        })
        .unwrap_or(0);

    println!("\nRESULT");
    println!(
        "  the construct ran from the LIVE graph: {} rig checks all evaluated, {} passed, {} failed ({} load-bearing).",
        rig.len(),
        rig.len() - failures.len(),
        failures.len(),
        load_bearing_failures.len()
    );
    println!(
        "  overall health: {overall_state} ({measured_dimensions} dimensions measured, {unmeasured_dimensions} honestly not_measured)."
    );
    if failures.is_empty() {
        println!("  a real local inference returned, was receipted, and was checked against the declared transformation.");
    } else {
        println!("  the failures above are MEASURED evidence of failure — not absence, not an empty completion.");
    }
    Ok(())
}
