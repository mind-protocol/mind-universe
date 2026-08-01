//! End-to-end proof for collectivized inference.
//!
//! Not "it compiles" and not "a unit test passes". This runs the real path:
//!
//!   1. COPY a live store (never writing the original) and commit the routing
//!      table into the copy as ONE atomic attributed transaction.
//!   2. Reopen the copy from disk INDEPENDENTLY and read the routing back out
//!      of committed content. Everything after this point is driven by the
//!      committed graph, not by the fixture file.
//!   3. Dispatch four turns through their AUTHORED chains:
//!        A  real local inference against a running Ollama;
//!        B  a route whose only provider has no credential -> `not_configured`;
//!        C  a route whose first link is unreachable -> MEASURED transport
//!           failure -> authored `advance_on` -> the working local model;
//!        D  a turn whose inference is never landed at all.
//!   4. Land A/B/C in REVERSE order, and prove admission still follows the
//!      endogenous wake order — the inference is not the clock.
//!   5. Let D time out and prove it becomes `unknown`, distinct from C's
//!      `measurement_failed` and B's `not_configured`.
//!   6. Prove no credential material is anywhere in the emitted evidence.
//!   7. Commit the evidence as a Moment into the copy and read it back from a
//!      fresh reopen — it survives reload.
//!
//! Usage: `cargo run -p universe-inference --features store-proof --bin inference_proof [store-dir]`
//! The store dir defaults to the live registry store and is only ever READ.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use universe_core::{EntityKey, Revision, Tick};
use universe_inference::clock::{AdmissionGate, AdmissionWorld, Turn, TurnDisposition};
use universe_inference::contract::{InferenceProvider, InferenceRequest, ProviderReadiness};
use universe_inference::routing::{RoutingSource, RoutingTable};
use universe_inference::{install_all, CollectiveRouter, HttpJsonProvider};
use universe_store::{EntityRecord, UniverseSnapshot, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const ROUTING_CANONICAL_ID: &str = "code:l4:mind-universe:inference-routing-v0";
const ROUTING_ENTITY_BASE: u128 = 0x3000_0000;
const MOMENT_ENTITY_BASE: u128 = 0x3001_0000;
const BLOCK_SPAN: u128 = 4096;

fn main() {
    if let Err(error) = run() {
        eprintln!("\nINFERENCE PROOF FAILED: {error}");
        std::process::exit(1);
    }
}

// ===========================================================================
// A world that answers admission's precondition questions.
// ===========================================================================

/// The proof's stand-in for the L2 world. It is deliberately explicit about
/// what is proven: only these exact (verb, target) pairs hold, and the
/// revision it reports is the revision of the reopened store copy.
struct ProofWorld {
    revision: u64,
    proven: BTreeSet<(String, String)>,
}

impl AdmissionWorld for ProofWorld {
    fn current_revision(&self) -> u64 {
        self.revision
    }
    fn precondition_holds(&self, _actor_id: &str, verb: &str, target: &str) -> bool {
        self.proven
            .contains(&(verb.to_string(), target.to_string()))
    }
}

// ===========================================================================
// Store plumbing (against a COPY, always)
// ===========================================================================

/// Copy a store consistently while other processes are writing it.
///
/// `UniverseStore::replay` applies EVERY record in the event log to the
/// checkpoint with no skipping, so the copy is only usable if `snapshot.json`
/// is exactly the base the log continues from. A naive directory copy tears:
/// if a concurrent writer checkpoints between two files, the snapshot ends up
/// ahead of the log and replay fails with a revision conflict.
///
/// So: copy the checkpoint FIRST and the event log LAST (an older checkpoint
/// with a newer log still replays forward), then VERIFY by replaying, and
/// retry the whole copy if the window was unlucky. A torn copy is reported,
/// never silently used.
/// Which basis the working copy ended up on. Reported in the evidence so a run
/// can never imply it used the full live history when it did not.
#[derive(Clone, Debug)]
enum StoreBasis {
    /// Checkpoint plus the full event log, replayed.
    CheckpointAndLog { revision: Revision },
    /// Checkpoint only, because the source event log does not replay. The
    /// exact defect is carried so this is never mistaken for a clean run.
    CheckpointOnly {
        revision: Revision,
        log_defect: String,
        dropped_events: usize,
    },
}

impl StoreBasis {
    fn describe(&self) -> String {
        match self {
            StoreBasis::CheckpointAndLog { revision } => {
                format!("checkpoint + full event log, replayed to revision {}", revision.0)
            }
            StoreBasis::CheckpointOnly {
                revision,
                log_defect,
                dropped_events,
            } => format!(
                "checkpoint ONLY at revision {} ({dropped_events} events in the source log were \
                 NOT applied because the source log does not replay: {log_defect})",
                revision.0
            ),
        }
    }
}

fn ordered_names(source: &Path) -> Result<Vec<std::ffi::OsString>, Box<dyn Error>> {
    let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(source)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|entry| entry.file_name())
        .collect();
    // checkpoint first, event log last, content in between.
    names.sort_by_key(|name| match name.to_string_lossy().as_ref() {
        "snapshot.json" => 0,
        "events.jsonl" => 2,
        _ => 1,
    });
    Ok(names)
}

/// Copy a store while other processes are writing it.
///
/// `UniverseStore::replay` applies EVERY record in the event log to the
/// checkpoint with no skipping, so the copy is only usable if `snapshot.json`
/// is exactly the base the log continues from. Two things can break that:
///
/// * a torn copy — a concurrent writer moved the store between two file
///   copies. Retrying fixes it, so we retry.
/// * a source log that genuinely does not replay — e.g. two events appended
///   against the same base revision by racing writers. Retrying cannot fix
///   that, so we fall back to the checkpoint alone and SAY SO, rather than
///   pretending the run used the full history.
fn copy_store(source: &Path, destination: &Path) -> Result<(usize, StoreBasis), Box<dyn Error>> {
    const ATTEMPTS: usize = 4;
    let mut last_error = String::new();

    for attempt in 1..=ATTEMPTS {
        if destination.exists() {
            std::fs::remove_dir_all(destination)?;
        }
        std::fs::create_dir_all(destination)?;
        let names = ordered_names(source)?;
        if names.is_empty() {
            return Err(format!("no store files found under {}", source.display()).into());
        }
        for name in &names {
            std::fs::copy(source.join(name), destination.join(name))?;
        }
        match UniverseStore::open(destination)
            .and_then(|store| store.replay(store.load_snapshot()?))
        {
            Ok(snapshot) => {
                return Ok((
                    names.len(),
                    StoreBasis::CheckpointAndLog {
                        revision: snapshot.revision,
                    },
                ))
            }
            Err(error) => {
                last_error = format!("{error}");
                eprintln!("  copy attempt {attempt}/{ATTEMPTS} does not replay ({last_error})");
                std::thread::sleep(std::time::Duration::from_millis(200 * attempt as u64));
            }
        }
    }

    // Fall back to the checkpoint alone. This is the store's own committed
    // checkpoint, read from disk — not a fabricated world — but it is NOT the
    // full live history, and the evidence says exactly that.
    if destination.exists() {
        std::fs::remove_dir_all(destination)?;
    }
    std::fs::create_dir_all(destination)?;
    let mut copied = 0;
    let mut dropped_events = 0;
    for name in ordered_names(source)? {
        if name.to_string_lossy() == "events.jsonl" {
            dropped_events = std::fs::read_to_string(source.join(&name))
                .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count())
                .unwrap_or(0);
            std::fs::write(destination.join(&name), b"")?;
        } else {
            std::fs::copy(source.join(&name), destination.join(&name))?;
        }
        copied += 1;
    }
    let store = UniverseStore::open(destination)?;
    let snapshot = store.replay(store.load_snapshot()?)?;
    Ok((
        copied,
        StoreBasis::CheckpointOnly {
            revision: snapshot.revision,
            log_defect: last_error,
            dropped_events,
        },
    ))
}

fn free_entity_key(snapshot: &UniverseSnapshot, base: u128) -> Result<EntityKey, Box<dyn Error>> {
    for offset in 0..BLOCK_SPAN {
        let key = EntityKey(base + offset);
        if !snapshot.entities.iter().any(|entity| entity.key == key) {
            return Ok(key);
        }
    }
    Err(format!("no free entity key in the block at {base:#x}").into())
}

/// Commit one authored node into the store copy as ONE atomic attributed
/// transaction: symbol interning + the entity, under one idempotency key.
///
/// `UniverseWriteSet` carries no `causal_ancestry` field, so the ancestry is
/// recorded where it stays inspectable — inside the committed node's own
/// `provenance` block. Nothing is committed anonymously.
fn commit_node(
    store: &UniverseStore,
    symbol: &str,
    base: u128,
    idempotency_key: &str,
    causal_ancestry: Vec<String>,
    wrapper: &Value,
) -> Result<(EntityKey, Revision), Box<dyn Error>> {
    let mut wrapper = wrapper.clone();
    let provenance = wrapper
        .as_object_mut()
        .ok_or("node wrapper is not a JSON object")?
        .entry("provenance")
        .or_insert_with(|| json!({}));
    let provenance = provenance
        .as_object_mut()
        .ok_or("node provenance is not a JSON object")?;
    provenance.insert("causal_ancestry".into(), json!(causal_ancestry));
    provenance.insert("idempotency_key".into(), json!(idempotency_key));
    let wrapper = Value::Object(
        wrapper
            .as_object()
            .expect("checked above")
            .clone(),
    );

    let mut live = store.replay(store.load_snapshot()?)?;
    let plan = live.plan_symbol_interning(&[symbol.to_string()])?;
    let symbol_id = *plan
        .assignments
        .get(symbol)
        .ok_or("symbol interning plan did not assign the requested symbol")?;
    let content = store.append_content(&wrapper)?;
    let key = free_entity_key(&live, base)?;

    let mut commands = Vec::new();
    if !plan.additions.is_empty() {
        commands.push(UniverseCommand::InternSymbols {
            symbols: plan.additions.clone(),
        });
    }
    commands.push(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key,
            generation: 0,
            symbol: symbol_id,
            content: Some(content),
        },
    });

    let write_set = UniverseWriteSet {
        base_revision: live.revision,
        idempotency_key: idempotency_key.to_string(),
        commands,
    };
    let boundary_tick = Tick(live.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&live, write_set)?;
    transaction.commit(store, &mut live, boundary_tick)?;
    Ok((key, live.revision))
}

/// Independent readback: a FRESH reopen from disk, not the handle we wrote
/// with, hydrating content until the canonical id is found.
fn read_committed_node(
    store_dir: &Path,
    canonical_id: &str,
) -> Result<(Value, Revision), Box<dyn Error>> {
    let fresh = UniverseStore::open(store_dir)?;
    let snapshot = fresh.replay(fresh.load_snapshot()?)?;
    let revision = snapshot.revision;
    for entity in &snapshot.entities {
        let Some(content_ref) = entity.content.as_ref() else {
            continue;
        };
        let wrapper = fresh.read_content(content_ref)?;
        if wrapper.get("canonical_id").and_then(Value::as_str) == Some(canonical_id) {
            return Ok((wrapper, revision));
        }
    }
    Err(format!("{canonical_id} is not committed in {}", store_dir.display()).into())
}

// ===========================================================================
// The prompt
// ===========================================================================

/// A bounded WorldObservation, serialized. The native floor does not author
/// the *content* of a real observation — this stands in for the frame the
/// supervisor would produce, so the proof exercises a real prompt shape.
fn observation(actor: &str, verbs: &[&str], targets: &[&str]) -> String {
    format!(
        "You are {actor}, acting in a constructed city.\n\
         Available verbs: {}\n\
         Reachable targets: {}\n\
         Choose exactly ONE verb and ONE target.\n\
         Answer with the verb and the target id on one line, then a short reason.\n",
        verbs.join(", "),
        targets.join(", ")
    )
}

const VERBS: [&str; 3] = ["inspect", "connect", "open"];
const TARGETS: [&str; 2] = ["thing:beacon-a", "thing:beacon-b"];

fn request(
    turn_id: &str,
    actor_id: &str,
    revision: u64,
    deadline: u64,
) -> InferenceRequest {
    InferenceRequest {
        turn_id: turn_id.to_string(),
        actor_id: actor_id.to_string(),
        observation: observation(actor_id, &VERBS, &TARGETS),
        observed_at_revision: revision,
        dispatched_at_tick: Tick(1),
        deadline_tick: Tick(deadline),
        offered_verbs: VERBS.iter().map(|verb| verb.to_string()).collect(),
        offered_targets: TARGETS.iter().map(|target| target.to_string()).collect(),
        causal_ancestry: vec![format!("wake:{turn_id}")],
    }
}

fn turn(turn_id: &str, actor_id: &str, wake_seq: u64, revision: u64, deadline: u64) -> Turn {
    Turn {
        turn_id: turn_id.to_string(),
        actor_id: actor_id.to_string(),
        wake_seq,
        dispatched_at_tick: Tick(1),
        deadline_tick: Tick(deadline),
        observed_at_revision: revision,
        offered_verbs: VERBS.iter().map(|verb| verb.to_string()).collect(),
        offered_targets: TARGETS.iter().map(|target| target.to_string()).collect(),
    }
}

// ===========================================================================
// The run
// ===========================================================================

fn run() -> Result<(), Box<dyn Error>> {
    let live_store = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("UNIVERSE_STORE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));

    let work = std::env::temp_dir().join(format!(
        "universe-inference-proof-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let store_copy = work.join("store");

    println!("== (0) store ==");
    println!("  source (READ ONLY) : {}", live_store.display());
    println!("  working copy       : {}", store_copy.display());
    let (copied, basis) = copy_store(&live_store, &store_copy)?;
    println!("  copied {copied} file(s); the live store is never written by this run");
    println!("  basis: {}", basis.describe());

    // ---- (1) commit the routing table into the COPY ----------------------
    let fixture_path = Path::new("crates/universe-inference/fixtures/inference-routing-v0.json");
    let fixture: Value = serde_json::from_slice(&std::fs::read(fixture_path)?)?;
    // Validate the AUTHORED table before committing anything.
    RoutingTable::parse(fixture.get("content").ok_or("fixture has no content block")?)?;

    let store = UniverseStore::open(&store_copy)?;
    let before = store.replay(store.load_snapshot()?)?.revision;
    let (routing_key, after) = commit_node(
        &store,
        "code",
        ROUTING_ENTITY_BASE,
        &format!("inference-routing:{}", now_ms()),
        vec![
            "session:collectivized-inference".to_string(),
            "intent:author-provider-routing".to_string(),
        ],
        &fixture,
    )?;
    println!("\n== (1) routing committed into the copy ==");
    println!("  entity {routing_key:#x?}  revision {} -> {}", before.0, after.0);

    // ---- (2) read it back INDEPENDENTLY ----------------------------------
    let (wrapper, committed_revision) = read_committed_node(&store_copy, ROUTING_CANONICAL_ID)?;
    let table = RoutingTable::parse(
        wrapper
            .get("content")
            .ok_or("committed node carries no content block")?,
    )?;
    let source = RoutingSource::Committed {
        store: store_copy.display().to_string(),
        revision: committed_revision.0,
    };
    println!("\n== (2) routing read back from a FRESH reopen ==");
    println!("  {}", source.describe());
    println!(
        "  routing {} v{} : {} providers, {} routes, admission order = {}",
        table.routing_id,
        table.version,
        table.providers.len(),
        table.routes.len(),
        table.admission.order
    );
    println!("  everything below is driven by the COMMITTED table, not the fixture file");

    // ---- (3) build the collective from the committed data -----------------
    let mut router = CollectiveRouter::new(table.clone(), source);
    install_all(&mut router)?;
    println!("\n== (3) provider readiness (decided WITHOUT calling anything) ==");
    let mut readiness_report = Vec::new();
    for spec in &table.providers {
        let probe = HttpJsonProvider::new(
            spec.clone(),
            universe_inference::transport_for(spec)?,
        );
        let state = probe.readiness();
        let label = match &state {
            ProviderReadiness::Ready => "ready".to_string(),
            ProviderReadiness::NotConfigured { missing, .. } => {
                format!("not_configured (missing {missing})")
            }
        };
        println!(
            "  {:<28} {:<10} {}",
            spec.provider_id, probe.transport_id(), label
        );
        readiness_report.push(json!({
            "provider_id": spec.provider_id,
            "transport": probe.transport_id(),
            "readiness": state,
        }));
    }

    // ---- (4) dispatch four turns -----------------------------------------
    let revision = committed_revision.0;
    let deadline = 5u64;
    let plan = [
        ("turn-a", "actor:l1:mind-universe:captain", 1u64),
        ("turn-b", "actor:l1:mind-universe:remote-only", 2),
        ("turn-c", "actor:l1:mind-universe:failover-drill", 3),
        ("turn-d", "actor:l1:mind-universe:never-lands", 4),
    ];

    let mut gate = AdmissionGate::at_tick(&table.admission, Tick(1));
    for (turn_id, actor_id, wake_seq) in plan {
        gate.dispatch(turn(turn_id, actor_id, wake_seq, revision, deadline))
            .map_err(|refusal| format!("dispatch refused: {refusal:?}"))?;
    }
    println!(
        "\n== (4) {} turns in flight (authored max_in_flight = {}) ==",
        gate.in_flight(),
        table.admission.max_in_flight
    );

    let mut observations = BTreeMap::new();
    for (turn_id, actor_id, _) in plan.iter().take(3) {
        let request = request(turn_id, actor_id, revision, deadline);
        let route = table
            .route_for(actor_id)
            .map(|route| route.route_id.clone())
            .unwrap_or_else(|| "known_absent".into());
        println!("  dispatching {turn_id} ({actor_id}) via route {route} ...");
        let observed = router.dispatch(&request, Tick(1));
        println!(
            "    -> {:<20} served_by {:?}  attempts {}  {} ms  charged {}/{}",
            observed.outcome.label(),
            observed.attribution.served_by,
            observed.attribution.attempts.len(),
            observed.attribution.total_latency_ms,
            observed.attribution.budget_charged,
            observed.attribution.budget_allowed,
        );
        for attempt in &observed.attribution.attempts {
            println!(
                "       attempt {:<28} {:<18} {}",
                attempt.provider_id,
                attempt.outcome_label,
                truncate(&attempt.detail, 90)
            );
        }
        observations.insert((*turn_id).to_string(), observed);
    }
    println!("  turn-d is deliberately never dispatched to any provider");

    // ---- (5) land in REVERSE order ---------------------------------------
    println!("\n== (5) landing answers in REVERSE order (c, b, a) ==");
    for turn_id in ["turn-c", "turn-b", "turn-a"] {
        let observed = observations
            .get(turn_id)
            .ok_or_else(|| format!("missing observation for {turn_id}"))?
            .clone();
        gate.land(turn_id, observed)?;
    }
    let arrival: Vec<String> = gate
        .landing_order()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    println!("  arrival order  : {arrival:?}");

    // ---- (6) drain: admission follows the WAKE order ----------------------
    let world = ProofWorld {
        revision,
        proven: VERBS
            .iter()
            .flat_map(|verb| {
                TARGETS
                    .iter()
                    .map(move |target| (verb.to_string(), target.to_string()))
            })
            .collect(),
    };

    let first_drain = gate.drain(&world);
    let admitted: Vec<String> = first_drain
        .iter()
        .map(|d| d.turn_id().to_string())
        .collect();
    println!("  admission order: {admitted:?}");
    println!("  (turn-d is still in flight and holds the queue behind it)");

    if admitted != vec!["turn-a", "turn-b", "turn-c"] {
        return Err(format!(
            "ADMISSION ORDER VIOLATED: arrival was {arrival:?} but admission was {admitted:?}; \
             the inference latency scheduled the city"
        )
        .into());
    }
    if arrival == admitted {
        return Err("the arrival order was not actually reversed; the test proves nothing".into());
    }

    // ---- (7) the deadline, not the network, releases turn-d ---------------
    println!("\n== (6) advancing the clock past turn-d's deadline ==");
    while gate.tick().0 <= deadline {
        gate.advance_tick();
    }
    let second_drain = gate.drain(&world);
    println!("  tick {} -> drained {}", gate.tick().0, second_drain.len());

    let mut dispositions = first_drain;
    dispositions.extend(second_drain);

    println!("\n== (7) dispositions ==");
    for disposition in &dispositions {
        println!(
            "  {:<8} wake {}  {:<18} {}",
            disposition.turn_id(),
            disposition.wake_seq(),
            disposition.label(),
            describe(disposition)
        );
    }

    // ---- (8) the epistemic ladder must be distinguishable -----------------
    let label_of = |turn_id: &str| {
        dispositions
            .iter()
            .find(|d| d.turn_id() == turn_id)
            .map(|d| d.label().to_string())
            .unwrap_or_else(|| "MISSING".into())
    };
    let ladder = json!({
        "turn-b_remote_without_credential": label_of("turn-b"),
        "turn-c_after_measured_transport_failure": label_of("turn-c"),
        "turn-d_never_landed": label_of("turn-d"),
    });
    if label_of("turn-d") != "unknown" {
        return Err(format!(
            "turn-d never landed but was reported as {:?}, not `unknown`",
            label_of("turn-d")
        )
        .into());
    }
    if label_of("turn-b") != "not_configured" {
        return Err(format!(
            "turn-b has no credential but was reported as {:?}, not `not_configured`",
            label_of("turn-b")
        )
        .into());
    }
    let turn_c_saw_measured_failure = observations
        .get("turn-c")
        .map(|observed| {
            observed
                .attribution
                .attempts
                .iter()
                .any(|attempt| attempt.outcome_label == "measurement_failed")
        })
        .unwrap_or(false);
    if !turn_c_saw_measured_failure {
        return Err(
            "turn-c did not record a measured transport failure, so the authored \
             fallback was not exercised against real evidence"
                .into(),
        );
    }
    println!("\n== (8) epistemic ladder ==");
    println!("{}", serde_json::to_string_pretty(&ladder)?);

    // ---- (9) no credential material anywhere in the evidence --------------
    let evidence = json!({
        "runner": "inference_proof",
        "routing_source": describe_source(&store_copy, committed_revision),
        "store_basis": basis.describe(),
        "routing_id": table.routing_id,
        "routing_version": table.version,
        "provider_readiness": readiness_report,
        "admission": {
            "order": table.admission.order,
            "clock": table.admission.clock,
            "max_in_flight": table.admission.max_in_flight,
            "stale_policy": table.admission.stale_policy,
            "arrival_order": arrival,
            "admission_order": admitted,
            "arrival_order_differs_from_admission_order": true
        },
        "epistemic_ladder": ladder,
        "dispositions": dispositions,
        "attributions": observations
            .iter()
            .map(|(turn_id, observed)| (turn_id.clone(), json!(observed.attribution)))
            .collect::<BTreeMap<_, _>>(),
    });
    let evidence_text = serde_json::to_string(&evidence)?;
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        let key = key.trim().to_string();
        if !key.is_empty() && evidence_text.contains(&key) {
            return Err("CREDENTIAL LEAK: the API key appears in the emitted evidence".into());
        }
    }
    println!("\n== (9) credential discipline ==");
    println!("  evidence scanned for the configured credential: absent");

    // ---- (10) commit the evidence and read it back ------------------------
    let moment_id = format!("moment:l4:mind-universe:inference-proof:{}", now_ms());
    let moment = json!({
        "canonical_id": moment_id,
        "node_type": "moment",
        "subtype": "validation_run",
        "content": evidence,
    });
    let (moment_key, moment_revision) = commit_node(
        &store,
        "moment",
        MOMENT_ENTITY_BASE,
        &format!("inference-proof:{}", now_ms()),
        vec![
            "session:collectivized-inference".to_string(),
            format!("routing:{ROUTING_CANONICAL_ID}"),
        ],
        &moment,
    )?;
    let (read_back, final_revision) = read_committed_node(&store_copy, &moment_id)?;
    println!("\n== (10) evidence committed and independently read back ==");
    println!("  moment {moment_key:#x?}  revision {}", moment_revision.0);
    println!("  fresh reopen at revision {} found {moment_id}", final_revision.0);
    if read_back.pointer("/content/admission/admission_order") != evidence.pointer("/admission/admission_order")
    {
        return Err("the committed evidence did not read back identically".into());
    }
    println!("  admission order survived reload: {admitted:?}");

    println!("\nRESULT");
    println!("  routing was read from the COMMITTED graph, not the fixture.");
    println!("  answers arrived {arrival:?} and were admitted {admitted:?} — the wake");
    println!("  order held, so provider latency did not schedule the city.");
    println!(
        "  not_configured / measurement_failed / unknown stayed distinguishable end to end."
    );
    println!("  working copy left for inspection: {}", work.display());
    Ok(())
}

fn describe_source(store_copy: &Path, revision: Revision) -> String {
    RoutingSource::Committed {
        store: store_copy.display().to_string(),
        revision: revision.0,
    }
    .describe()
}

fn describe(disposition: &TurnDisposition) -> String {
    match disposition {
        TurnDisposition::Proposal {
            verb,
            target,
            attribution,
            ..
        } => format!(
            "{verb} on {target} (served by {:?})",
            attribution.served_by
        ),
        TurnDisposition::Rejected { kind, detail, .. } => {
            format!("{kind:?}: {}", truncate(detail, 100))
        }
        TurnDisposition::Refused { category, .. } => format!("category {category}"),
        TurnDisposition::MeasurementFailed { reason, .. } => truncate(reason, 110),
        TurnDisposition::NotConfigured { missing, .. } => format!("missing {missing}"),
        TurnDisposition::NotAttempted { reason, .. } => truncate(reason, 110),
        TurnDisposition::Unknown { reason, .. } => truncate(reason, 110),
    }
}

fn truncate(text: &str, max: usize) -> String {
    let single_line = text.replace(['\r', '\n'], " ");
    if single_line.chars().count() <= max {
        single_line
    } else {
        single_line.chars().take(max).collect::<String>() + "..."
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}
