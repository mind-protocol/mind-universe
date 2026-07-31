//! CHANTIER C4 — **a construct CALLS a construct, with an argument, over the
//! Invocation trigger path**, and the callee returns a committed Moment.
//!
//! `construct_calls_construct` proved the *physics* call: A's fire deposits a
//! bounded quantum onto B's gate so B self-wakes on its OWN field. This bin
//! proves the *named* call: A addresses a SPECIFIC callee subscription and hands
//! it an argument, via the new `TriggerEventKind::Invocation` wake event —
//! modelled on the supervisor heartbeat (`wake_event_commits_a_moment_two_ticks_
//! after_firing`) and `wake_bridge.rs`. Where `AtomFired` is "a construct wakes
//! on its own crossing", `Invocation` is "a construct wakes a named target and
//! passes it fields".
//!
//! The chain proven end-to-end, on the SAME frozen seam the heartbeat drains
//! through (`TriggerScheduler` → `drain_eligible` → resolve_code → fields-as-
//! inputs → execute → translate):
//!
//! ```text
//! A fires -> emits an Invocation TriggerEvent
//!            { subject = B's subscription entry, fields = { "arg": Integer(N) } }
//!         -> ingest into a REAL TriggerScheduler (one execution request enqueued)
//!         -> drain_eligible (the heartbeat's drain half)
//!         -> a TriggerTickDriver resolves B's pinned CodeDefinition,
//!            runs it on its OWN VM host reading `fields` as INPUTS,
//!            B Proposes a result record built FROM the arg
//!         -> translate the receipt into a Moment write-set whose causal_ancestry
//!            carries the trigger-hop tokens (caller hop + B's own execution hop)
//!         -> commit at the tick boundary -> CommitReceipt::Committed
//!         -> independent readback: B's Moment is durably present and REFLECTS N.
//! ```
//!
//! What is PROVEN:
//!   * B fired ONLY because A invoked it: the execution request exists solely
//!     because the Invocation event was ingested and drained.
//!   * B genuinely CONSUMED the argument: B's program reads `arg` via
//!     `Operator::Input` (it holds NO constant equal to N), so the committed
//!     Moment can reflect N only if the field flowed field → VM input → proposal
//!     → commit. Change N and the committed Moment changes.
//!   * The committed Moment THREADS THE CALLER: its causal_ancestry carries the
//!     caller's trigger-hop token, so caller → callee is inspectable — the
//!     substrate on which the compiler's causal-cycle / max-depth guard
//!     (`build_execution_request`) refuses a ping-pong.
//!
//! HONEST BOUNDARIES (read before trusting this):
//!   * A's fire is SIMULATED: this harness does not run A's own wave. It seeds
//!     the Invocation event directly, standing in for A's live emitter. The
//!     A→B call path itself (event → scheduler → drain → driver → commit) is
//!     real, not simulated.
//!   * The caller hop on the event is SYNTHESIZED here. In a LIVE run the
//!     dispatcher that emits the Invocation on A's behalf MUST populate
//!     `TriggerEvent::causal_ancestry` with A's real `execution_hop`. If it
//!     emits EMPTY caller ancestry, the committed Moment still threads B's own
//!     hop but NOT the caller — and A↔B ping-pong becomes undetectable. Filling
//!     the caller hop is the live dispatcher's load-bearing job (the ping-pong
//!     guard); this bin prints that obligation explicitly rather than hide it.
//!   * READ-ONLY on any real store: this boots a fresh Genesis into a scratch
//!     dir. NEVER pass a live store path.
//!
//! Usage: `call_bridge [scratch-store-dir]`  (defaults to a fresh temp dir).

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use universe_core::{EntityKey, Epistemic, Revision, Tick, UniverseError};
use universe_ir::{
    CausalHop, CodeDefinition, Operator, QuerySpec, TriggerBudgets, TriggerControls, TriggerEvent,
    TriggerEventKind, TriggerEventPayload, TriggerEvidenceRequirement, TriggerSubscription, Value,
    TRIGGER_CONTRACT_VERSION,
};
use universe_store::{EntityRecord, UniverseSnapshot};
use universe_supervisor::{
    PhaseHook, Supervisor, TickPhase, TriggerScheduler, TriggerSchedulerLimits, TriggerTickDriver,
};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet};
use universe_vm::{execute_program, ExecutionLimits, ExecutionReceipt, VmHost};

/// The single argument A passes to B. Deliberately a value that appears NOWHERE
/// as a constant in B's program, so the only way the committed Moment can carry
/// it is by B genuinely reading the invocation field.
const ARG: i64 = 41;

/// A trivial VM host: B's program issues no queries, so every query method is
/// unreachable and returns an error rather than fabricate a result. Mirrors the
/// heartbeat test's `NullHost`.
struct NullHost;
impl VmHost for NullHost {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn open_query(&mut self, _: &QuerySpec, _: &Value, _: &Value) -> Result<Value, String> {
        Err("call program issues no query".into())
    }
    fn await_query(&mut self, _: &Value) -> Result<Value, String> {
        Err("call program issues no query".into())
    }
    fn follow_one(&mut self, _: &Value, _: &Value) -> Result<Value, String> {
        Err("call program follows nothing".into())
    }
    fn entity_symbol(&mut self, _: &Value) -> Result<Value, String> {
        Err("call program reads no symbol".into())
    }
    fn hydrate(&mut self, _: &[Value], _: u32) -> Result<Vec<Value>, String> {
        Err("call program hydrates nothing".into())
    }
}

/// The callee's cognition authority — the frozen C4-LIVE seam. It resolves B's
/// pinned code, runs it on its OWN host reading the Invocation `fields` as
/// inputs, and translates the measured receipt into B's Moment write-set. The
/// supervisor owns none of this. `moment_base` is the fresh-key floor; the
/// committed key encodes the consumed argument so the Moment REFLECTS the input.
struct CallDriver {
    host: NullHost,
    code: CodeDefinition,
    code_key: EntityKey,
    moment_base: u128,
}

impl TriggerTickDriver for CallDriver {
    fn resolve_code(
        &mut self,
        request: &universe_ir::ExecutionRequest,
    ) -> Result<CodeDefinition, UniverseError> {
        // Pinned to the one known callee code node. (C4-LIVE swaps in snapshot
        // hydration + code_hash verification against the committed store.)
        if request.code_definition != self.code_key {
            return Err(UniverseError::Validation("unknown code definition".into()));
        }
        Ok(self.code.clone())
    }

    fn execute(
        &mut self,
        code: &CodeDefinition,
        inputs: &BTreeMap<String, Value>,
        limits: ExecutionLimits,
        revision: Revision,
        tick: Tick,
    ) -> Result<ExecutionReceipt, UniverseError> {
        execute_program(code, &mut self.host, inputs, revision, tick, limits)
            .map_err(|error| UniverseError::Validation(format!("call program failed: {error}")))
    }

    fn translate(
        &mut self,
        request: &universe_ir::ExecutionRequest,
        receipt: &ExecutionReceipt,
        snapshot: &UniverseSnapshot,
    ) -> Result<Option<UniverseWriteSet>, UniverseError> {
        if receipt.proposals.len() != 1 {
            return Err(UniverseError::Validation(
                "call program did not propose exactly one command".into(),
            ));
        }
        // The consumed argument, recovered from the proposal B built FROM its
        // input. If B had not read the field, this record would not carry it.
        let consumed = match &receipt.proposals[0].command {
            Value::Record(fields) => match fields.get("result") {
                Some(Value::Integer(n)) => *n,
                _ => {
                    return Err(UniverseError::Validation(
                        "proposal record missing integer `result`".into(),
                    ))
                }
            },
            _ => {
                return Err(UniverseError::Validation(
                    "proposal command is not a record".into(),
                ))
            }
        };
        // The Moment's key ENCODES the consumed argument, so the committed,
        // independently-observable entity reflects the input. The causal
        // ancestry threads the FULL hop chain the pinned request carries: the
        // caller's hop (from the Invocation event) followed by B's own execution
        // hop — so the Moment is provably a consequence of the call.
        Ok(Some(UniverseWriteSet {
            base_revision: snapshot.revision,
            idempotency_key: format!("call-moment:{}", request.request_id),
            causal_ancestry: request.descendant_causal_tokens(),
            commands: vec![UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: EntityKey(self.moment_base + consumed as u128),
                    generation: 0,
                    symbol: 0,
                    content: None,
                },
            }],
        }))
    }
}

/// A no-op phase hook: the proof asserts on the commit, not on phases.
struct NoopHook;
impl PhaseHook for NoopHook {
    fn run(&mut self, _: TickPhase, _: &UniverseSnapshot) -> Result<(), UniverseError> {
        Ok(())
    }
}

/// B's pinned program: read the invocation argument, wrap it into a `result`
/// record, propose it, return it. It holds NO constant — the value can only come
/// from the invocation field, so a committed Moment carrying it PROVES B read it.
fn callee_program() -> CodeDefinition {
    CodeDefinition {
        ir_version: 0,
        revision: Revision(1),
        required_capabilities: Vec::new(),
        operators: vec![
            Operator::Input {
                name: "arg".into(),
                output: 0,
            },
            Operator::MakeRecord {
                fields: vec![("result".into(), 0)],
                output: 1,
            },
            Operator::Propose {
                command: 1,
                output: 2,
            },
            Operator::Return { value: 2 },
        ],
    }
}

/// The measured facts of the whole proof.
struct ProofOutput {
    // The Invocation event A emitted.
    callee_entry: EntityKey,
    event_kind: TriggerEventKind,
    event_id: String,
    arg_field: Value,
    // Scheduler evidence.
    backlog_after_ingest: u32,
    drained_requests: usize,
    request_id: String,
    // B's execution.
    inputs: BTreeMap<String, Value>,
    b_result: Value,
    // The committed Moment.
    moment_key: EntityKey,
    commit_tick: Tick,
    commit_revision: Revision,
    revision_before: Revision,
    moment_present_on_readback: bool,
    // The causal thread.
    caller_token: String,
    callee_hop_token: String,
    commit_ancestry: Vec<String>,
    // The argument the committed Moment reflects, and whether B's program baked
    // it in as a constant (it must not).
    moment_base: u128,
    program_has_arg_constant: bool,
}

fn drive(store_dir: &Path, genesis: &Path) -> Result<ProofOutput, Box<dyn Error>> {
    let mut supervisor = Supervisor::boot(store_dir, genesis)?;
    let revision_before = supervisor.revision();
    let now = supervisor.tick();

    // Fresh-key floor for B's Moment (above every Genesis entity).
    let moment_base = supervisor
        .snapshot()
        .entities
        .iter()
        .map(|e| e.key.0)
        .max()
        .unwrap_or(0)
        + 1;

    // --- Construct B: a named callee. Its entry (the subscription) is the
    // address A invokes; its pinned code reads the argument.
    let b_entry = EntityKey(0xB0);
    let b_sub_revision = Revision(3);
    let code_key = EntityKey(0x00B0_C0DE);
    let code = callee_program();
    let subscription = TriggerSubscription {
        contract_version: TRIGGER_CONTRACT_VERSION,
        subscription: b_entry,
        revision: b_sub_revision,
        enabled: true,
        event_kinds: vec![TriggerEventKind::Invocation],
        code_definition: code_key,
        code_revision: Revision(1),
        code_hash: "a".repeat(64),
        evidence_requirement: TriggerEvidenceRequirement::Measured,
        max_event_age_ticks: 8,
        budgets: TriggerBudgets {
            fuel: 64,
            max_mutations: 4,
            max_ticks: 3,
        },
        controls: TriggerControls {
            cooldown_ticks: 0,
            debounce_ticks: 0,
            max_causal_depth: 4,
            max_firings_per_tick: 3,
        },
        idempotency_namespace: "call-bridge".into(),
    };

    // --- Construct A: the caller. Its fire is SIMULATED here; we synthesize the
    // hop A's live dispatcher would attach to the Invocation event so the callee's
    // Moment threads back to A. A's entry differs from B's, so this is not a cycle.
    let a_entry = EntityKey(0x0A);
    let caller_hop = CausalHop {
        subscription: a_entry,
        subscription_revision: Revision(1),
        event_id: "A-fired:orientation-beacon".into(),
        request_id: "A-exec-0001".into(),
    };

    // --- The Invocation: subject = B's entry, fields = { arg }. This is the call.
    let arg = Value::Integer(ARG);
    let mut fields = BTreeMap::new();
    fields.insert("arg".to_string(), arg.clone());
    let event = TriggerEvent {
        event_id: "invocation:A->B:0001".into(),
        kind: TriggerEventKind::Invocation,
        source_revision: revision_before,
        occurred_at: now,
        observed_at: now,
        evidence: Epistemic::Measured(TriggerEventPayload {
            subject: Some(b_entry),
            fields: fields.clone(),
            receipt_hash: None,
        }),
        causal_ancestry: vec![caller_hop.clone()],
    };

    // --- Ingest into a REAL TriggerScheduler (the wake-queue).
    let limits = TriggerSchedulerLimits {
        max_backlog: 64,
        max_requests_per_tick: 16,
        max_fuel_per_tick: 4096,
        max_mutations_per_tick: 64,
        max_tracked_idempotency_keys: 1024,
    };
    let mut scheduler = TriggerScheduler::new(limits)?;
    let _ingress = scheduler.ingest_event(&[subscription.clone()], &event, revision_before, now);
    let backlog_after_ingest = scheduler.backlog();

    // --- Drain via the SAME shape the heartbeat uses (advance_inner's drain loop).
    let drained = scheduler.drain_eligible(now);
    let drained_requests = drained.len();
    let request = drained
        .into_iter()
        .next()
        .ok_or("the Invocation did not enqueue an execution request")?;

    // --- Run B's pinned program through the TriggerTickDriver, reading `fields`
    // as inputs — byte-for-byte the supervisor's in-tick drain (lines 1330-1352).
    let mut driver = CallDriver {
        host: NullHost,
        code: code.clone(),
        code_key,
        moment_base,
    };
    let resolved = driver.resolve_code(&request)?;
    let inputs = match &request.trigger.evidence {
        Epistemic::Measured(payload) => payload.fields.clone(),
        _ => BTreeMap::new(),
    };
    let exec_limits = ExecutionLimits {
        fuel: request.budgets.fuel,
        max_proposals: request.budgets.max_mutations,
    };
    let receipt = driver.execute(&resolved, &inputs, exec_limits, revision_before, now)?;
    let b_result = receipt.result.clone();
    let write_set = driver
        .translate(&request, &receipt, supervisor.snapshot())?
        .ok_or("driver proposed no write-set")?;

    let moment_key = match &write_set.commands[0] {
        UniverseCommand::PutEntity { entity } => entity.key,
        other => return Err(format!("unexpected command: {other:?}").into()),
    };
    let commit_ancestry_expected = write_set.causal_ancestry.clone();

    // --- Commit B's result Moment at the tick boundary.
    let transaction = UniverseTransaction::prepare(supervisor.snapshot(), write_set)?;
    supervisor.enqueue(transaction);
    let mut hook = NoopHook;
    let commits = supervisor.advance(&mut hook)?;
    let commit = commits
        .into_iter()
        .next()
        .ok_or("the drained call committed no Moment")?;
    let (commit_tick, commit_revision, commit_ancestry) = match &commit {
        CommitReceipt::Committed {
            tick,
            revision,
            causal_ancestry,
            ..
        } => (*tick, *revision, causal_ancestry.clone()),
        other => return Err(format!("expected a committed Moment, got {other:?}").into()),
    };
    debug_assert_eq!(commit_ancestry, commit_ancestry_expected);

    // --- Independent readback: the Moment is durably present and reflects N.
    let readback = supervisor.independent_readback()?;
    let moment_present_on_readback = readback.entities.iter().any(|e| e.key == moment_key);

    // Provenance tokens: the caller hop A attached, and B's own execution hop.
    let caller_token = caller_hop.canonical_token();
    let callee_hop_token = request.execution_hop().canonical_token();

    // B's program provably holds no constant equal to the argument.
    let program_has_arg_constant = code.operators.iter().any(|op| {
        matches!(op, Operator::Constant { value: Value::Integer(n), .. } if *n == ARG)
    });

    Ok(ProofOutput {
        callee_entry: b_entry,
        event_kind: event.kind,
        event_id: event.event_id.clone(),
        arg_field: arg,
        backlog_after_ingest,
        drained_requests,
        request_id: request.request_id.clone(),
        inputs,
        b_result,
        moment_key,
        commit_tick,
        commit_revision,
        revision_before,
        moment_present_on_readback,
        caller_token,
        callee_hop_token,
        commit_ancestry,
        moment_base,
        program_has_arg_constant,
    })
}

/// Assert the proof's measured facts. Shared by the bin and the `#[test]`.
fn assert_proof(out: &ProofOutput) -> Result<(), String> {
    if out.event_kind != TriggerEventKind::Invocation {
        return Err("the wake event was not an Invocation".into());
    }
    if out.callee_entry != EntityKey(0xB0) {
        return Err("the Invocation did not address B's entry".into());
    }
    if out.backlog_after_ingest != 1 {
        return Err(format!(
            "the Invocation enqueued {} requests, expected exactly 1",
            out.backlog_after_ingest
        ));
    }
    if out.drained_requests != 1 {
        return Err(format!(
            "drain returned {} requests, expected exactly 1",
            out.drained_requests
        ));
    }
    // B genuinely consumed the argument: its input carried it, its result echoes
    // it, and its program bakes in no such constant.
    match out.inputs.get("arg") {
        Some(Value::Integer(n)) if *n == ARG => {}
        other => return Err(format!("B's input `arg` was {other:?}, expected Integer({ARG})")),
    }
    let expected_result = Value::Record(BTreeMap::from([("result".to_string(), Value::Integer(ARG))]));
    if out.b_result != expected_result {
        return Err(format!(
            "B's result {:?} did not reflect the argument {ARG}",
            out.b_result
        ));
    }
    if out.program_has_arg_constant {
        return Err("B's program baked in the argument as a constant — consumption not proven".into());
    }
    // The committed Moment reflects the argument in its stable key.
    if out.moment_key != EntityKey(out.moment_base + ARG as u128) {
        return Err("the committed Moment key does not encode the consumed argument".into());
    }
    if !out.moment_present_on_readback {
        return Err("the committed Moment is absent on independent readback".into());
    }
    if out.commit_revision != Revision(out.revision_before.0 + 1) {
        return Err("the Moment did not advance the Universe revision by one".into());
    }
    // The committed Moment threads BOTH the caller hop and B's own execution hop.
    if !out.commit_ancestry.contains(&out.caller_token) {
        return Err("the committed Moment does not thread the caller's trigger-hop token".into());
    }
    if !out.commit_ancestry.contains(&out.callee_hop_token) {
        return Err("the committed Moment does not thread B's own execution hop".into());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("CALL-BRIDGE FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir_arg = env::args_os().nth(1).map(PathBuf::from);
    let store_dir = store_dir_arg.clone().unwrap_or_else(default_scratch_store);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let genesis = repo.join("fixtures/genesis/minimal-genesis.json");
    fs::create_dir_all(&store_dir)?;
    println!("scratch store: {}", store_dir.display());
    println!("genesis      : {}", genesis.display());

    let out = drive(&store_dir, &genesis)?;

    println!("\n== C4: a construct CALLS a construct (Invocation), the callee returns a Moment ==");

    println!("\n-- (1) A fires -> the Invocation TriggerEvent A emits --");
    println!("  event_id : {}", out.event_id);
    println!("  kind     : {:?}", out.event_kind);
    println!(
        "  subject  : {:#x}   (B's subscription/entry — the named callee)",
        out.callee_entry.0
    );
    println!("  fields   : {{ \"arg\": {:?} }}   (the ONE argument A passes)", out.arg_field);

    println!("\n-- (2) ingest into a real TriggerScheduler, drain via the heartbeat's shape --");
    println!("  backlog after ingest : {}", out.backlog_after_ingest);
    println!("  drained requests     : {}", out.drained_requests);
    println!("  request_id           : {}", out.request_id);

    println!("\n-- (3) B's pinned program runs through the TriggerTickDriver, fields as inputs --");
    println!("  B inputs : {:?}", out.inputs);
    println!("  B result : {:?}   (a record built FROM the input — B consumed the arg)", out.b_result);
    println!(
        "  program bakes in a constant == {ARG}? {}   (false => the value came from the field)",
        out.program_has_arg_constant
    );

    println!("\n-- (4) commit B's result Moment; independent readback --");
    println!(
        "  Moment key : {:#x}   (= moment_base {:#x} + arg {ARG} — the committed Moment REFLECTS the input)",
        out.moment_key.0, out.moment_base
    );
    println!("  commit tick / revision : {} / {}", out.commit_tick.0, out.commit_revision.0);
    println!(
        "  revision before -> after : {} -> {}",
        out.revision_before.0, out.commit_revision.0
    );
    println!("  Moment present on readback : {}", out.moment_present_on_readback);

    println!("\n-- (5) the causal thread (caller -> callee), carried into the committed Moment --");
    println!("  caller hop token (A) : {}", out.caller_token);
    println!("  callee hop token (B) : {}", out.callee_hop_token);
    println!("  committed Moment causal_ancestry : {:?}", out.commit_ancestry);

    assert_proof(&out).map_err(|e| format!("proof failed: {e}"))?;

    println!("\n=================================================================================");
    println!("A CONSTRUCT CALLS A CONSTRUCT (WITH AN ARGUMENT) — proven end-to-end.");
    println!("=================================================================================");
    println!("  (a) A emitted an Invocation TriggerEvent naming B's entry and carrying one arg;");
    println!("  (b) a REAL TriggerScheduler enqueued exactly one execution request; draining it");
    println!("      ran B's pinned program on the driver's host, reading the fields as inputs;");
    println!("  (c) B consumed the arg (its program holds no such constant) and Proposed a result;");
    println!("  (d) the result committed as a Moment that REFLECTS the arg (key = base + arg) and");
    println!("      is durably present on independent readback at revision+1;");
    println!("  (e) the committed Moment's causal_ancestry threads the caller hop AND B's own hop,");
    println!("      so caller -> callee is inspectable.");
    println!();
    println!("  HONEST BOUNDARY — the ping-pong guard: A's fire is SIMULATED and the caller hop is");
    println!("  SYNTHESIZED here. In a LIVE run the dispatcher emitting the Invocation on A's behalf");
    println!("  MUST populate TriggerEvent::causal_ancestry with A's real execution_hop. If it emits");
    println!("  EMPTY caller ancestry, the Moment still threads B's own hop but NOT the caller, and");
    println!("  A<->B ping-pong becomes undetectable — the compiler's causal-cycle / max-depth guard");
    println!("  in build_execution_request can only bite on ancestry the live dispatcher actually fills.");

    if store_dir_arg.is_none() {
        let _ = fs::remove_dir_all(&store_dir);
    }
    Ok(())
}

fn default_scratch_store() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!("call-bridge-{}-{nanos}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full C4 Invocation proof over a fresh tempdir scratch store: A's
    /// Invocation event ingests into a real scheduler, drains in the heartbeat's
    /// shape, B's pinned program consumes the arg and proposes a result, the
    /// result commits as a Moment that reflects the arg and threads the caller.
    #[test]
    fn a_construct_calls_a_construct_with_an_argument() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let genesis = repo.join("fixtures/genesis/minimal-genesis.json");
        fs::create_dir_all(&store_dir).unwrap();

        let out = drive(&store_dir, &genesis).unwrap();
        assert_proof(&out).unwrap();

        // Independent restatement of the load-bearing facts.
        assert_eq!(out.event_kind, TriggerEventKind::Invocation);
        assert_eq!(out.backlog_after_ingest, 1, "the Invocation enqueued one request");
        assert_eq!(
            out.moment_key,
            EntityKey(out.moment_base + ARG as u128),
            "the committed Moment reflects the consumed argument"
        );
        assert!(out.moment_present_on_readback, "Moment durably present after commit");
        assert!(
            out.commit_ancestry.contains(&out.caller_token),
            "the committed Moment threads the caller's trigger-hop token"
        );
    }
}
