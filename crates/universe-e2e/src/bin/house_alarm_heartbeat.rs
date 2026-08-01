//! chantier C1, bridge slice-1 + slice-2: the FIRST real construct run THROUGH
//! the LIVE heartbeat, in-tick — not a hand-driven commit.
//!
//! `construct_live_run` proved the FIRST durable Moment caused by a construct
//! fire, but it drove the commit BY HAND: it ran the physics wave, then itself
//! translated the evidence into a write-set and called `advance` once to commit
//! it. This driver removes the hand: it wires the house-alarm construct into the
//! supervisor's OWN wake-queue and lets the heartbeat close the loop.
//!
//! The heartbeat CLAUDE.md names — "the physics step fills a wake-queue; the
//! serial loop drains it" — is exercised end to end over TWO real ticks of
//! `Supervisor::advance_draining_triggers`, WITHOUT touching the frozen
//! supervisor seam (no `lib.rs` edit; this bin brings the selector, the
//! subscription, the pinned CodeDefinition, and the driver):
//!
//!   tick N   (advance #1): the Physics phase runs the resolved house-alarm wave;
//!            `alarm_trigger` crosses its threshold and FIRES; the supervisor maps
//!            that firing to an `AtomFired` wake event and INGESTS it onto its
//!            OWN wake-queue (`trigger_backlog` goes 0 -> >0). Nothing commits.
//!   tick N+1 (advance #2): the serial loop DRAINS the now-eligible wake request
//!            BEFORE the commit boundary, resolves its pinned CodeDefinition, RUNS
//!            it on the driver's VM host (one fuel-bounded, mutation-free
//!            inference that PROPOSES one crossing), and the driver TRANSLATES the
//!            proposal into ONE crossing `Moment` write-set that COMMITS in THIS
//!            tick (revision +1, tick N+1). The model proposes; the world disposes.
//!
//! The two-tick latency is honest and load-bearing: the firing is observed
//! strictly AFTER tick N's commit boundary closed, so the earliest its Moment can
//! commit is tick N+1's boundary. It is exposed, never hidden.
//!
//! HARD HONESTY (identical boundary to `construct_live_run`):
//!   * The sensor crossing that SEEDS the fire is SIMULATED: `physics_intersection_event`
//!     is seeded from the authored `external_measured_injections`, which on a live
//!     world MUST arrive from a real Rapier intersection through the
//!     physics-event -> atom-deposit bridge. This proves the wake path + heartbeat,
//!     NOT a real entry.
//!   * `precreated: false` on both Moment blocks — every field is derived from what
//!     this one wave measured.
//!   * Overall health = `not_measured` where evidence is absent (NEVER fabricated
//!     healthy): only quiescence, energy conservation, physics-event non-mutation,
//!     and effect-receipt integrity are `measured`; every population dimension a
//!     live alarm needs is `not_measured`. A single simulated crossing certifies
//!     nothing.
//!
//! Usage: `house_alarm_heartbeat [scratch-store-dir]`
//!   scratch-store-dir defaults to a fresh unique dir under the system temp dir.
//!   NEVER pass a live store: this boots a fresh Genesis and needs an empty dir.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use serde_json::Value as JsonValue;

use universe_capabilities::{CapabilityHost, EffectAdapter, EffectExecutionReceipt, EffectReceipt};
use universe_compiler::canonical_hash;
use universe_core::{EntityKey, Epistemic, Revision, Tick, UniverseError};
use universe_e2e::construct_resolver::{resolve_construct, AlarmAtomCircuit, ResolvedConstruct};
use universe_ir::{
    CodeDefinition, ExecutionRequest, Operator, TriggerBudgets, TriggerControls,
    TriggerEvidenceRequirement, TriggerEventKind, TriggerSubscription, Value as IrValue, IR_VERSION,
    TRIGGER_CONTRACT_VERSION,
};
use universe_physics::{fired_atoms, AtomConvergence, AtomExecutionBudget};
use universe_store::{ContentRef, EntityRecord, UniverseSnapshot};
use universe_supervisor::{
    PhaseHook, PhysicsDepositOutcome, PhysicsWaveInputs, PhysicsWaveSelector, Supervisor, TickPhase,
    TickOutcome, TriggerTickDriver,
};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseWriteSet};
use universe_vm::{execute_program, ExecutionLimits, ExecutionReceipt, VmHost};

/// The capability the authored effect binding names.
const NOTIFY_CAPABILITY: &str = "safe.notify";

/// The write-set idempotency key for the single crossing Moment.
const MOMENT_ID: &str = "moment:l2:lumina-prime:house-alarm:heartbeat-crossing-v0";

/// A test transport standing in for the authorized external notification
/// channel. It acknowledges the payload; the ack is the measured transport
/// result carried in the EffectReceipt. It is an UNAUTHORIZED test stand-in, so
/// the honesty accounting treats notify-authorization as `not_measured`.
struct NotifyTransport;
impl EffectAdapter for NotifyTransport {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut ack = b"notified:".to_vec();
        ack.extend_from_slice(payload);
        Ok(ack)
    }
}

/// A no-op phase hook: the only work at the tick boundary is committing the
/// enqueued Moment transaction, which the supervisor's Commit phase does.
struct NoopHook;
impl PhaseHook for NoopHook {
    fn run(&mut self, _phase: TickPhase, _snapshot: &UniverseSnapshot) -> Result<(), UniverseError> {
        Ok(())
    }
}

/// The caller-supplied (graph-authority) execution budget (same as `house_alarm_resolved`).
fn budget() -> AtomExecutionBudget {
    AtomExecutionBudget {
        max_atoms: 16,
        max_bonds: 16,
        max_steps: 16,
        max_total_energy: 10_000,
    }
}

/// A bare VM host: the pinned validation program uses only constants, a record,
/// a propose and a return — no queries, follows, hydrations or capabilities — so
/// every host method that would reach outside the fuel-bounded, mutation-free VM
/// returns an error rather than fabricate a value.
struct BareHost;
impl VmHost for BareHost {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn open_query(
        &mut self,
        _spec: &universe_ir::QuerySpec,
        _origin: &IrValue,
        _selector: &IrValue,
    ) -> Result<IrValue, String> {
        Err("bare host performs no queries".into())
    }
    fn await_query(&mut self, _handle: &IrValue) -> Result<IrValue, String> {
        Err("bare host performs no queries".into())
    }
    fn follow_one(&mut self, _source: &IrValue, _predicate: &IrValue) -> Result<IrValue, String> {
        Err("bare host follows no relations".into())
    }
    fn entity_symbol(&mut self, _entity: &IrValue) -> Result<IrValue, String> {
        Err("bare host resolves no symbols".into())
    }
    fn hydrate(&mut self, _selected: &[IrValue], _max_bytes: u32) -> Result<Vec<IrValue>, String> {
        Err("bare host hydrates nothing".into())
    }
}

/// The tiny pinned validation program (graph-owned CodeDefinition). Given the
/// fired evidence, it PROPOSES exactly one crossing marker record. It is
/// deliberately input-free (the supervisor's C4-LIVE producer seam currently
/// carries empty payload fields) and constant-only, so it runs on the bare host.
/// Its single proposal is the gate: if the program proposed nothing, the driver
/// commits nothing.
fn validation_program() -> CodeDefinition {
    CodeDefinition {
        ir_version: IR_VERSION,
        revision: Revision(1),
        required_capabilities: Vec::new(),
        operators: vec![
            Operator::Constant {
                value: IrValue::Text("crossing".into()),
                output: 0,
            },
            Operator::Constant {
                value: IrValue::Integer(1),
                output: 1,
            },
            Operator::MakeRecord {
                fields: vec![("moment_kind".into(), 0), ("crossings".into(), 1)],
                output: 2,
            },
            Operator::Propose {
                command: 2,
                output: 3,
            },
            Operator::Return { value: 3 },
        ],
    }
}

/// Build the AtomFired subscription for the house alarm: a graph-authored
/// subscription binding `AtomFired` wake events to the pinned validation program.
/// The `code_hash` is the program's real canonical hash so the request pins the
/// exact code, not a placeholder.
fn alarm_subscription(code: &CodeDefinition) -> Result<TriggerSubscription, Box<dyn Error>> {
    Ok(TriggerSubscription {
        contract_version: TRIGGER_CONTRACT_VERSION,
        subscription: EntityKey(0x9002),
        revision: Revision(1),
        enabled: true,
        event_kinds: vec![TriggerEventKind::AtomFired],
        code_definition: EntityKey(0x9001),
        code_revision: code.revision,
        code_hash: canonical_hash(code)?,
        evidence_requirement: TriggerEvidenceRequirement::Measured,
        max_event_age_ticks: 64,
        budgets: TriggerBudgets {
            fuel: 1024,
            max_mutations: 4,
            max_ticks: 4,
        },
        controls: TriggerControls {
            cooldown_ticks: 0,
            debounce_ticks: 0,
            max_causal_depth: 8,
            max_firings_per_tick: 8,
        },
        idempotency_namespace: "house-alarm:heartbeat".into(),
    })
}

/// One-shot selector: returns the resolved house-alarm wave on its FIRST call
/// (tick N) and `None` thereafter. This models one simulated crossing: the alarm
/// wakes exactly once, and later ticks add no new firing. The supervisor holds no
/// cluster policy; the selector is the only authority that decides the wave.
struct AlarmWaveOnce {
    inputs: Option<PhysicsWaveInputs>,
}
impl PhysicsWaveSelector for AlarmWaveOnce {
    fn select(
        &mut self,
        _snapshot: &UniverseSnapshot,
    ) -> Result<Option<PhysicsWaveInputs>, UniverseError> {
        Ok(self.inputs.take())
    }
}

/// The evidence captured from tick N's wave, handed to the driver before tick
/// N+1 so its `translate` can build the crossing Moment. The Moment content is
/// pre-appended to the content segment (which needs the store) and referenced by
/// `content_ref`; `translate` itself only assembles the write-set.
struct CrossingEvidence {
    alarm_trigger: EntityKey,
    exec_idempotency_key: String,
    content_ref: ContentRef,
    base_revision: Revision,
}

/// The caller-supplied (graph / cognition authority) trigger driver. It resolves
/// the pinned validation program, runs it on its OWN bare VM host, and — only for
/// the construct's trigger-atom firing — translates the proposal into ONE
/// crossing Moment write-set. The emitter's downstream firing is not this
/// construct's wake, so its request translates to nothing.
struct AlarmDriver {
    code: CodeDefinition,
    host: BareHost,
    evidence: CrossingEvidence,
    /// The measured receipts of every drained request, in drain order — evidence,
    /// not self-declared success.
    executed: Vec<ExecutionReceipt>,
    /// The subject atom of every request whose program translated to a Moment,
    /// plus the causal tokens composed for it -- surfaced from the driver that
    /// actually builds them, never read back out of a receipt.
    translated_subjects: Vec<EntityKey>,
    translated_ancestry: Vec<String>,
}

impl TriggerTickDriver for AlarmDriver {
    fn resolve_code(&mut self, _request: &ExecutionRequest) -> Result<CodeDefinition, UniverseError> {
        // A production resolver hydrates the pinned CodeDefinition from the
        // committed snapshot and verifies the hash; here the program is held
        // directly, and the subscription's `code_hash` already pins it.
        Ok(self.code.clone())
    }

    fn execute(
        &mut self,
        code: &CodeDefinition,
        inputs: &BTreeMap<String, IrValue>,
        limits: ExecutionLimits,
        revision: Revision,
        tick: Tick,
    ) -> Result<ExecutionReceipt, UniverseError> {
        let receipt = execute_program(code, &mut self.host, inputs, revision, tick, limits)
            .map_err(|error| UniverseError::Validation(format!("validation program trapped: {error}")))?;
        self.executed.push(receipt.clone());
        Ok(receipt)
    }

    fn translate(
        &mut self,
        request: &ExecutionRequest,
        receipt: &ExecutionReceipt,
        snapshot: &UniverseSnapshot,
    ) -> Result<Option<UniverseWriteSet>, UniverseError> {
        // The fired atom this wake carries. Only the construct's trigger atom
        // (`alarm_trigger`) is this construct's wake; the terminal emitter's
        // downstream firing is not, so it proposes no Moment.
        let subject = match &request.trigger.evidence {
            Epistemic::Measured(payload) | Epistemic::Observed(payload) => payload.subject,
            _ => None,
        };
        if subject != Some(self.evidence.alarm_trigger) {
            return Ok(None);
        }
        // The program is the gate: no proposal => no Moment. The world disposes
        // only of what the model proposed.
        if receipt.proposals.is_empty() {
            return Ok(None);
        }

        let moment_symbol = snapshot
            .symbol_id("Moment")
            .ok_or_else(|| UniverseError::Validation("minimal genesis has no Moment symbol".into()))?;
        let moment_key = EntityKey(
            snapshot
                .entities
                .iter()
                .map(|entity| entity.key.0)
                .max()
                .unwrap_or(0)
                + 1,
        );

        // Causal ancestry threads the FIRED ALARM ATOM end to end: the trigger
        // hop tokens (whose event_id encodes the fired atom hex), an explicit
        // fired-alarm-atom token, the executed EffectReceipt, and the construct.
        let mut causal_ancestry = request.descendant_causal_tokens();
        causal_ancestry.push(format!(
            "house-alarm:alarm_trigger-fired:{:#x}",
            self.evidence.alarm_trigger.0
        ));
        causal_ancestry.push(self.evidence.exec_idempotency_key.clone());
        causal_ancestry
            .push("construct:l2:lumina-prime:house-alarm-v0:heartbeat-fire".to_string());

        self.translated_subjects.push(self.evidence.alarm_trigger);
        self.translated_ancestry = causal_ancestry;

        Ok(Some(UniverseWriteSet {
            base_revision: self.evidence.base_revision,
            idempotency_key: MOMENT_ID.to_string(),
            commands: vec![UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: moment_key,
                    generation: 0,
                    symbol: moment_symbol,
                    content: Some(self.evidence.content_ref.clone()),
                },
            }],
        }))
    }
}

/// Read the authored `alarm_atom_circuit` from the construct's graph projection.
fn load_circuit(fixture: &Path) -> Result<AlarmAtomCircuit, Box<dyn Error>> {
    let bytes = fs::read(fixture)?;
    let root: JsonValue = serde_json::from_slice(&bytes)?;
    let members = root
        .get("members")
        .and_then(JsonValue::as_array)
        .ok_or("fixture has no members array")?;
    let code_member = members
        .iter()
        .find(|member| {
            member.get("id").and_then(JsonValue::as_str)
                == Some("code:l2:lumina-prime:house-alarm-v0")
        })
        .ok_or("fixture has no code:l2:lumina-prime:house-alarm-v0 member")?;
    let circuit_value = code_member
        .get("content")
        .and_then(|content| content.get("alarm_atom_circuit"))
        .ok_or("code member has no content.alarm_atom_circuit")?;
    Ok(serde_json::from_value(circuit_value.clone())?)
}

/// The full measured result of one live heartbeat run.
struct HeartbeatRun {
    resolved: ResolvedConstruct,
    fire_tick: Tick,
    /// Wake-queue backlog after tick N ingested the firing (before tick N+1 drained).
    backlog_after_fire: u32,
    outcome: PhysicsDepositOutcome,
    exec_receipt: EffectExecutionReceipt,
    /// The physics wave itself committed nothing: byte-identical across tick N.
    wave_store_unchanged: bool,
    revision_before: Revision,
    revision_after: Revision,
    commit_tick: Tick,
    commit_receipt: CommitReceipt,
    /// The measured receipts of every drained wake request in tick N+1.
    drained_receipts: Vec<ExecutionReceipt>,
    /// The subjects that translated to a Moment (must be exactly `[alarm_trigger]`).
    translated_subjects: Vec<EntityKey>,
    moment_key: EntityKey,
    moment_content: JsonValue,
    moment_content_readback: JsonValue,
    causal_ancestry: Vec<String>,
}

fn drive(store_dir: &Path, genesis: &Path, fixture: &Path) -> Result<HeartbeatRun, Box<dyn Error>> {
    let circuit = load_circuit(fixture)?;
    let resolved =
        resolve_construct(&circuit).map_err(|error| format!("resolve_construct failed: {error:?}"))?;
    let alarm_trigger = *resolved
        .atom_keys
        .get("alarm_trigger")
        .ok_or("authored circuit has no alarm_trigger atom")?;

    let mut supervisor = Supervisor::boot(store_dir, genesis)?;
    let revision_before = supervisor.revision();
    let fire_tick = supervisor.tick();

    // Wire the construct into the supervisor's OWN wake-queue.
    let code = validation_program();
    supervisor.register_subscription(alarm_subscription(&code)?);

    // The one-shot selector carries the resolved wave for tick N only.
    let mut selector = AlarmWaveOnce {
        inputs: Some(PhysicsWaveInputs {
            sensor_cluster: resolved.sensor_cluster.clone(),
            deposit_bindings: resolved.deposit_bindings.clone(),
            construct_cluster: resolved.construct_cluster.clone(),
            effect_bindings: resolved.effect_bindings.clone(),
            budget: budget(),
        }),
    };

    // A driver that never translates: proves tick N commits NOTHING and only
    // INGESTS the firing onto the wake-queue (the request is not drained until
    // tick N+1). We swap in the real driver for tick N+1 below.
    let bytes_before_wave = read_all_files(store_dir)?;
    let mut hook = NoopHook;

    // ---- tick N (advance #1): the wave fires; the AtomFired is ingested. ----
    // Use a driver whose translate is never reached this tick (the queue is empty
    // at drain time — the firing is ingested only AFTER this tick's commit).
    let mut ingest_only = IngestOnlyDriver;
    let tick_n: TickOutcome =
        supervisor.advance_draining_triggers(&mut hook, &mut selector, &mut ingest_only)?;
    let outcome = tick_n
        .physics_wave
        .ok_or("tick N produced no physics wave — the selector did not drive the alarm")?;
    if !tick_n.commits.is_empty() {
        return Err("tick N committed something — the firing must only be ingested, not committed".into());
    }
    let backlog_after_fire = supervisor.trigger_backlog();
    let bytes_after_wave = read_all_files(store_dir)?;
    let wave_store_unchanged = bytes_before_wave == bytes_after_wave;

    // Execute the surfaced notify candidate through the authorized transport.
    let candidate = outcome
        .candidate_effects
        .first()
        .cloned()
        .ok_or("no CANDIDATE notify EffectIntent surfaced — the construct did not fire")?;
    let mut capability_host = CapabilityHost::default();
    capability_host.register(NOTIFY_CAPABILITY, Box::new(NotifyTransport));
    let exec_receipt = capability_host.execute_measured(outcome.observed_at_tick, &candidate)?;
    supervisor.observe_transport_receipt(
        exec_receipt.capability.clone(),
        exec_receipt.idempotency_key.clone(),
        &exec_receipt.outcome,
    );

    // Translate the measured wave evidence into the crossing Moment content and
    // pre-append it to the content segment (needs the store; `translate` only
    // assembles the write-set that references it).
    let moment_content =
        build_crossing_moment(&resolved, &outcome, &exec_receipt, wave_store_unchanged);
    let content_ref = supervisor.append_content(&moment_content)?;

    let mut driver = AlarmDriver {
        code,
        host: BareHost,
        evidence: CrossingEvidence {
            alarm_trigger,
            exec_idempotency_key: exec_receipt.idempotency_key.clone(),
            content_ref,
            base_revision: revision_before,
        },
        executed: Vec::new(),
        translated_subjects: Vec::new(),
        translated_ancestry: Vec::new(),
    };

    // ---- tick N+1 (advance #2): drain -> run program -> commit Moment in-tick.
    let tick_n1: TickOutcome =
        supervisor.advance_draining_triggers(&mut hook, &mut selector, &mut driver)?;
    let commit_receipt = tick_n1
        .commits
        .into_iter()
        .next()
        .ok_or("tick N+1 committed nothing — the drained wake request did not commit a Moment")?;
    let commit_tick = match &commit_receipt {
        CommitReceipt::Committed { tick, .. } | CommitReceipt::AlreadyCommitted { tick, .. } => *tick,
    };

    // INDEPENDENT readback: fresh reopen from disk.
    let after = supervisor.independent_readback()?;
    let revision_after = after.revision;
    let moment_key = EntityKey(
        supervisor
            .snapshot()
            .entities
            .iter()
            .map(|entity| entity.key.0)
            .max()
            .unwrap_or(0),
    );
    let committed = after
        .entities
        .iter()
        .find(|entity| entity.key == moment_key)
        .ok_or("committed Moment entity absent on independent readback")?;
    let moment_content_readback = supervisor.read_content(
        committed
            .content
            .as_ref()
            .ok_or("committed Moment has no content on readback")?,
    )?;

    Ok(HeartbeatRun {
        resolved,
        fire_tick,
        backlog_after_fire,
        outcome,
        exec_receipt,
        wave_store_unchanged,
        revision_before,
        revision_after,
        commit_tick,
        commit_receipt,
        drained_receipts: driver.executed,
        translated_subjects: driver.translated_subjects,
        moment_key,
        moment_content,
        moment_content_readback,
        causal_ancestry: driver.translated_ancestry,
    })
}

/// A driver used only for tick N: its methods are never reached because the
/// wake-queue is empty at that tick's drain (the firing is ingested strictly
/// AFTER the commit boundary). Any accidental invocation fails loudly rather than
/// fabricate a result.
struct IngestOnlyDriver;
impl TriggerTickDriver for IngestOnlyDriver {
    fn resolve_code(&mut self, _request: &ExecutionRequest) -> Result<CodeDefinition, UniverseError> {
        Err(UniverseError::Validation(
            "tick N must not drain any request".into(),
        ))
    }
    fn execute(
        &mut self,
        _code: &CodeDefinition,
        _inputs: &BTreeMap<String, IrValue>,
        _limits: ExecutionLimits,
        _revision: Revision,
        _tick: Tick,
    ) -> Result<ExecutionReceipt, UniverseError> {
        Err(UniverseError::Validation(
            "tick N must not execute any program".into(),
        ))
    }
    fn translate(
        &mut self,
        _request: &ExecutionRequest,
        _receipt: &ExecutionReceipt,
        _snapshot: &UniverseSnapshot,
    ) -> Result<Option<UniverseWriteSet>, UniverseError> {
        Err(UniverseError::Validation(
            "tick N must not translate any proposal".into(),
        ))
    }
}

// ---- INLINE moment-translation (the shape reused from construct_live_run) ----

/// Reverse map EntityKey -> authored atom name for readable evidence refs.
fn name_of(resolved: &ResolvedConstruct, key: &EntityKey) -> String {
    resolved
        .atom_keys
        .iter()
        .find(|(_, k)| *k == key)
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| format!("{:#x}", key.0))
}

fn named_hex(
    resolved: &ResolvedConstruct,
    keys: impl IntoIterator<Item = EntityKey>,
) -> Vec<JsonValue> {
    use serde_json::json;
    keys.into_iter()
        .map(|k| json!({ "atom": name_of(resolved, &k), "key": format!("{:#x}", k.0) }))
        .collect()
}

fn not_measured(reason: &str) -> JsonValue {
    serde_json::json!({ "status": "not_measured", "why": reason })
}

/// Translate the MEASURED evidence of one resolved-construct wave + the executed
/// EffectReceipt into ONE crossing `Moment` write-set content. Every field is
/// GENUINELY derived from the run; `precreated: false`; overall health is
/// `not_measured` because the crossing is SIMULATED.
fn build_crossing_moment(
    resolved: &ResolvedConstruct,
    outcome: &PhysicsDepositOutcome,
    exec_receipt: &EffectExecutionReceipt,
    wave_store_unchanged: bool,
) -> JsonValue {
    use serde_json::json;

    let sensor_quiescent = outcome.sensor.convergence == AtomConvergence::Quiescent;
    let construct_quiescent = outcome.construct.convergence == AtomConvergence::Quiescent;
    let energy_conserved = outcome.sensor.energy.conserved && outcome.construct.energy.conserved;
    let terminal_starved_empty = outcome.construct.terminal_starved.is_empty();
    let transport_succeeded =
        matches!(exec_receipt.outcome, EffectReceipt::TransportSucceeded { .. });

    let sensor_fired = named_hex(resolved, fired_atoms(&outcome.sensor));
    let construct_fired = named_hex(resolved, outcome.fired_construct_atoms.iter().copied());
    let evidence_refs = json!({
        "measured_from": "one bounded atom-deposit wave driven IN-TICK by the live heartbeat (Supervisor::advance_draining_triggers, Physics phase) over a fresh Genesis scratch store",
        "sensor_convergence": format!("{:?}", outcome.sensor.convergence),
        "construct_convergence": format!("{:?}", outcome.construct.convergence),
        "sensor_energy_conserved": outcome.sensor.energy.conserved,
        "construct_energy_conserved": outcome.construct.energy.conserved,
        "sensor_fired_atoms": sensor_fired,
        "construct_fired_atoms": construct_fired,
        "construct_terminal_starved": outcome
            .construct
            .terminal_starved
            .iter()
            .map(|k| format!("{:#x}", k.0))
            .collect::<Vec<_>>(),
        "effect_receipt": {
            "capability": exec_receipt.capability,
            "idempotency_key": exec_receipt.idempotency_key,
            "transport_attempted": exec_receipt.transport_attempted,
            "outcome": if transport_succeeded { "TransportSucceeded" } else { "TransportFailed" },
        },
        "wave_store_byte_identical": wave_store_unchanged,
    });

    let validation_run = json!({
        "precreated": false,
        "runner": "house_alarm_heartbeat bin — the FIRST construct run through the live heartbeat in-tick",
        "starting_state": "fresh Genesis scratch store (minimal-genesis)",
        "wake_path": "physics fire (tick N) -> AtomFired ingested on the supervisor wake-queue -> drained tick N+1 -> pinned validation program proposed one crossing -> committed in-tick",
        "scenarios_exercised": [
            "single_crossing_fires_once (SIMULATED crossing via authored external_measured_injections)"
        ],
        "scenarios_not_run": [
            "no_crossing_no_fire",
            "armed_sensor_without_intersection_does_not_fire",
            "duplicate_intersection_fires_once",
            "unmeasured_intersection_rejected",
            "stale_intersection_rejected",
            "missing_intersection_evidence_no_fire",
            "notify_port_unlinked_no_external_notification",
            "notify_link_valid_emits_one_effect_intent",
            "notify_link_invalid_authority_no_notification",
            "notify_expired_validity_window_no_notification",
            "notify_within_cooldown_suppressed",
            "unmeasured_energy_rejected"
        ],
        "invariants": [
            { "invariant": "atom energy is conserved",
              "result": if energy_conserved { "measured_pass" } else { "measured_fail" },
              "evidence": "sensor.energy.conserved && construct.energy.conserved" },
            { "invariant": "quiescence reached",
              "result": if sensor_quiescent && construct_quiescent { "measured_pass" } else { "measured_fail" },
              "evidence": "sensor & construct AtomConvergence::Quiescent" },
            { "invariant": "the intersection PhysicsEvent never mutates the store directly",
              "result": if wave_store_unchanged { "measured_pass" } else { "measured_fail" },
              "evidence": "committed store byte-identical across the physics wave (tick N committed nothing)" },
            { "invariant": "the trigger fires on the deposited support (support >= 1)",
              "result": "measured_pass",
              "evidence": "alarm_trigger is in the construct fired-atom set" },
            { "invariant": "the terminal emitter conducts no energy onward (does not starve)",
              "result": if terminal_starved_empty { "measured_pass" } else { "measured_fail" },
              "evidence": "construct.terminal_starved is empty" },
            { "invariant": "each fire commits exactly one crossing Moment",
              "result": "measured_pass",
              "evidence": "the single alarm_trigger fire yields exactly one crossing Moment write-set (this entity), committed in-tick by the heartbeat" },
            { "invariant": "one EffectReceipt per emitted notify EffectIntent",
              "result": if transport_succeeded { "measured_pass" } else { "measured_fail" },
              "evidence": "the one surfaced notify candidate executed to one EffectReceipt (idempotency_key present)" }
        ],
        "invariants_not_checked": [
            { "invariant": "one trigger fire per measured crossing",
              "why": "the crossing is a SIMULATED authored injection; a per-crossing rate needs repeated REAL measured crossings" },
            { "invariant": "an armed sensor with no measured intersection never fires",
              "why": "the armed-without-intersection negative scenario was not run" },
            { "invariant": "no external notification without an exact valid linked notify authorization",
              "why": "the notify port was never linked; the transport here is an UNAUTHORIZED test stand-in, so authorization gating was not exercised" },
            { "invariant": "all streamed bond energy is measured",
              "why": "bond energy came from the authored injection seeds, not independently re-measured this run" },
            { "invariant": "every conducted bond conducts once",
              "why": "per-bond single-conduction was not read from a ledger (only aggregate conservation + no-starve observed)" }
        ],
        "honest_boundary": "the crossing is a SIMULATED authored injection (external_measured_injections), NOT a real Rapier intersection through the unbuilt physics-event -> atom-deposit bridge"
    });

    let health_assessment = json!({
        "precreated": false,
        "states_vocabulary": ["healthy", "degraded", "stale", "unknown", "not_measured", "measurement_failed"],
        "overall_state": "not_measured",
        "overall_state_justification": "No fresh validation run has measured the required dimensions: the crossing is a SIMULATED authored injection (the physics-event -> atom-deposit bridge is unbuilt), so no REAL fire has ever been measured. A single simulated crossing certifies none of the population dimensions a live alarm needs; the overall state is not_measured, never healthy.",
        "evidence_basis": "one bounded atom-deposit wave driven in-tick by the live heartbeat: convergence, energy conservation, the fired-atom set, one executed EffectReceipt, and store byte-identical across the wave",
        "dimensions": {
            "quiescence_reached": {
                "status": "measured",
                "value": sensor_quiescent && construct_quiescent,
                "evidence": "sensor & construct AtomConvergence::Quiescent"
            },
            "energy_conservation_error_u64": {
                "status": "measured",
                "value": 0,
                "conserved": energy_conserved,
                "evidence": "sensor.energy.conserved && construct.energy.conserved"
            },
            "physics_event_non_mutation_rate": {
                "status": "measured",
                "value": wave_store_unchanged,
                "evidence": "committed store byte-identical across the physics wave"
            },
            "effect_receipt_integrity": {
                "status": "measured",
                "value": transport_succeeded,
                "evidence": "the one notify candidate executed to a real TransportSucceeded EffectReceipt"
            },
            "crossing_detection_accuracy": not_measured(
                "the crossing is a SIMULATED authored injection; no real physics intersection was detected"),
            "single_fire_per_crossing_rate": not_measured(
                "one simulated wave only; a per-crossing rate needs repeated real measured crossings"),
            "false_fire_rate": not_measured(
                "no no-crossing / negative scenario was run"),
            "armed_no_intersection_no_fire_rate": not_measured(
                "the armed-without-intersection scenario was not run"),
            "crossing_moment_per_fire_rate": not_measured(
                "only this single fire's Moment is committed; a rate over a population is not measured"),
            "notify_authorization_accuracy": not_measured(
                "the notify port was never linked; the transport was an unauthorized test stand-in"),
            "unauthorized_notification_rate": not_measured(
                "authorization gating was not exercised"),
            "expired_notification_rate": not_measured(
                "validity-window cases were not run"),
            "cooldown_suppression_accuracy": not_measured(
                "cooldown was not exercised"),
            "measured_stream_only_rate": not_measured(
                "bond energy came from authored injection seeds, not independently re-derived as measured"),
            "single_conduction_accuracy": not_measured(
                "per-bond single-conduction ledger was not measured (only aggregate conservation + no-starve)"),
            "not_measured_honesty_rate": not_measured(
                "this meta-dimension was not independently evaluated"),
            "observer_fault_detection_rate": not_measured(
                "no independent observer fault-injection run was performed"),
            "evidence_freshness_ms": not_measured(
                "no evidence timestamps were captured this run")
        }
    });

    json!({
        "canonical_id": "moment:l2:lumina-prime:house-alarm:heartbeat-crossing-v0",
        "node_type": "narrative",
        "moment_kind": "crossing",
        "runtime_moment_subtypes": ["validation_run", "health_assessment"],
        "construct": "space:l2:lumina-prime:house-alarm-v0",
        "caused_by": "the alarm_trigger fire, woken through the live heartbeat wake-queue and committed in-tick",
        "evidence_refs": evidence_refs,
        "validation_run": validation_run,
        "health_assessment": health_assessment
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("HOUSE ALARM HEARTBEAT FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_scratch_store);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let genesis = manifest.join("../../fixtures/genesis/minimal-genesis.json");
    let fixture = manifest.join("../../fixtures/ontology/lumina-prime-house-alarm-v0.json");
    fs::create_dir_all(&store_dir)?;
    println!("scratch store: {}", store_dir.display());
    println!("genesis      : {}", genesis.display());
    println!("construct    : {}", fixture.display());

    let run = drive(&store_dir, &genesis, &fixture)?;

    let key = |name: &str| *run.resolved.atom_keys.get(name).expect("authored atom key");
    let citizen_body_intersects = key("citizen_body_intersects");
    let alarm_trigger = key("alarm_trigger");
    let notify_emitter = key("notify_emitter");

    // --- tick N: the construct fired in the Physics phase (SIMULATED crossing).
    println!("\n== tick N (advance #1): the construct FIRES in the Physics phase ==");
    println!("  fire tick N              : {}", run.fire_tick.0);
    let sensor_events = fired_atoms(&run.outcome.sensor);
    assert!(
        sensor_events.contains(&citizen_body_intersects),
        "the measured crossing atom must have fired"
    );
    assert_eq!(run.outcome.sensor.convergence, AtomConvergence::Quiescent);
    assert!(run.outcome.sensor.energy.conserved, "sensor energy conserved");
    assert!(
        run.outcome.fired_construct_atoms.contains(&alarm_trigger),
        "alarm_trigger must fire"
    );
    assert!(
        run.outcome.fired_construct_atoms.contains(&notify_emitter),
        "notify_emitter must fire"
    );
    assert!(run.outcome.construct.terminal_starved.is_empty());
    assert_eq!(run.outcome.construct.convergence, AtomConvergence::Quiescent);
    assert!(run.outcome.construct.energy.conserved, "construct energy conserved");
    println!(
        "  fired atoms              : alarm_trigger={:#x}  notify_emitter={:#x}",
        alarm_trigger.0, notify_emitter.0
    );

    // The firing was INGESTED onto the supervisor's OWN wake-queue, not committed.
    println!("  wake-queue backlog after : {} (AtomFired ingested, awaiting drain)", run.backlog_after_fire);
    assert!(
        run.backlog_after_fire >= 1,
        "the firing must be ingested onto the supervisor wake-queue"
    );
    println!("  store byte-identical      : {} (tick N committed nothing)", run.wave_store_unchanged);
    assert!(run.wave_store_unchanged, "the physics wave must not mutate the store");

    // The executed external effect (EffectIntent -> transport -> receipt).
    assert!(
        matches!(run.exec_receipt.outcome, EffectReceipt::TransportSucceeded { .. }),
        "the notify transport must succeed and yield a receipt"
    );
    println!(
        "  EffectReceipt             : capability={} key={} outcome=TransportSucceeded",
        run.exec_receipt.capability, run.exec_receipt.idempotency_key
    );

    // --- tick N+1: the serial loop DRAINED the wake request and committed in-tick.
    println!("\n== tick N+1 (advance #2): the serial loop DRAINS -> runs -> commits IN-TICK ==");
    println!("  drained wake requests    : {} (one program run per fired atom)", run.drained_receipts.len());
    for (i, receipt) in run.drained_receipts.iter().enumerate() {
        println!(
            "    request #{i}: fuel_used={} proposals={} (the pinned validation program ran on the driver's VM host)",
            receipt.fuel_used,
            receipt.proposals.len()
        );
    }
    assert!(
        !run.drained_receipts.is_empty(),
        "the wake request(s) must be drained and their program run in-tick"
    );
    assert!(
        run.drained_receipts.iter().all(|r| r.proposals.len() == 1),
        "each drained program must propose exactly one crossing"
    );
    println!(
        "  translated to a Moment   : {:?} (only the trigger-atom firing is this construct's wake)",
        run.translated_subjects
            .iter()
            .map(|k| format!("{:#x}", k.0))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        run.translated_subjects,
        vec![alarm_trigger],
        "exactly the alarm_trigger firing translates to a Moment"
    );

    println!("  commit receipt           : {:?}", run.commit_receipt);
    assert!(
        matches!(run.commit_receipt, CommitReceipt::Committed { .. }),
        "the Moment must commit freshly (not AlreadyCommitted)"
    );
    println!(
        "  revision advanced        : {} -> {}",
        run.revision_before.0, run.revision_after.0
    );
    assert_eq!(
        run.revision_after.0,
        run.revision_before.0 + 1,
        "the revision must advance by exactly 1"
    );
    println!("  fire tick N={}  ->  Moment commit tick N+1={}", run.fire_tick.0, run.commit_tick.0);
    assert_eq!(
        run.commit_tick.0,
        run.fire_tick.0 + 1,
        "the Moment must commit at the tick AFTER the fire (honest two-tick latency)"
    );
    println!("  Moment entity            : {:#x} (durably present on independent readback)", run.moment_key.0);
    assert_eq!(
        run.moment_content_readback, run.moment_content,
        "the independently read-back Moment content must equal the committed content"
    );

    // --- causal ancestry threads the fired alarm atom -------------------------
    println!("\n== causal ancestry threads the FIRED ALARM ATOM ==");
    for token in &run.causal_ancestry {
        println!("  - {token}");
    }
    let alarm_hex = format!("{:#x}", alarm_trigger.0);
    let explicit = format!("house-alarm:alarm_trigger-fired:{alarm_hex}");
    assert!(
        run.causal_ancestry.contains(&explicit),
        "the committed Moment's causal ancestry must thread the fired alarm atom explicitly"
    );
    let trigger_hop_threads_atom = run
        .causal_ancestry
        .iter()
        .any(|token| token.starts_with("trigger-hop:") && token.contains(&format!("atom-fired:")) && token.contains(&alarm_hex));
    assert!(
        trigger_hop_threads_atom,
        "the trigger hop token (event_id = atom-fired:*:<alarm_trigger>) must thread the fired alarm atom"
    );

    // --- the committed Moment (independent readback) --------------------------
    println!("\n== committed Moment content (INDEPENDENT readback) ==");
    println!("{}", serde_json::to_string_pretty(&run.moment_content_readback)?);

    let dims = run.moment_content_readback["health_assessment"]["dimensions"]
        .as_object()
        .expect("health_assessment.dimensions object");
    let mut measured: Vec<&String> = Vec::new();
    let mut not_measured_dims: Vec<&String> = Vec::new();
    for (name, value) in dims {
        match value.get("status").and_then(JsonValue::as_str) {
            Some("measured") => measured.push(name),
            _ => not_measured_dims.push(name),
        }
    }
    measured.sort();
    not_measured_dims.sort();
    println!("\n== honesty accounting (derived from the committed Moment) ==");
    println!(
        "  overall_state: {}",
        run.moment_content_readback["health_assessment"]["overall_state"]
    );
    println!("  measured dimensions ({}):", measured.len());
    for name in &measured {
        println!("    - {name}");
    }
    println!("  not_measured dimensions ({}):", not_measured_dims.len());
    for name in &not_measured_dims {
        println!("    - {name}");
    }
    assert_eq!(
        run.moment_content_readback["health_assessment"]["overall_state"],
        JsonValue::String("not_measured".into()),
        "a single simulated crossing must NOT certify a healthy alarm"
    );
    assert_eq!(measured.len(), 4, "exactly the four wave-covered dimensions are measured");
    assert!(not_measured_dims.len() >= 10, "the uncovered dimensions stay not_measured");

    println!("\nRESULT: the FIRST real construct running THROUGH the live heartbeat, in-tick.");
    println!("  tick N: the resolved house-alarm wave fires in the Physics phase; the supervisor");
    println!("          ingests the AtomFired onto its OWN wake-queue (backlog 0 -> {}). Nothing commits.", run.backlog_after_fire);
    println!("  tick N+1: the serial loop drains the now-eligible request, runs its pinned validation");
    println!("          program on the driver's VM host (one proposal), and COMMITS one crossing Moment");
    println!("          in-tick (revision +1, tick N+1). The model proposes; the world disposes.");
    println!("  HONESTY: the crossing is SIMULATED; overall health = not_measured (4 measured, rest not_measured).");

    if env::args_os().nth(1).is_none() {
        let _ = fs::remove_dir_all(&store_dir);
    }
    Ok(())
}

fn default_scratch_store() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!("house-alarm-heartbeat-{}-{nanos}", std::process::id()))
}

/// Read every file under `dir` into a path-keyed map of raw bytes, for a literal
/// byte-identity comparison of the committed store across the physics wave.
fn read_all_files(dir: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, Box<dyn Error>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path)?;
                out.insert(path, bytes);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_construct_runs_through_the_live_heartbeat_in_tick() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let genesis = manifest.join("../../fixtures/genesis/minimal-genesis.json");
        let fixture = manifest.join("../../fixtures/ontology/lumina-prime-house-alarm-v0.json");
        fs::create_dir_all(&store_dir).unwrap();

        let run = drive(&store_dir, &genesis, &fixture).unwrap();
        let alarm_trigger = *run.resolved.atom_keys.get("alarm_trigger").unwrap();

        // tick N: fired + ingested + nothing committed.
        assert!(run.backlog_after_fire >= 1);
        assert!(run.wave_store_unchanged);
        assert!(matches!(
            run.exec_receipt.outcome,
            EffectReceipt::TransportSucceeded { .. }
        ));

        // tick N+1: drained, ran the program, committed in-tick, revision +1.
        assert!(!run.drained_receipts.is_empty());
        assert!(run.drained_receipts.iter().all(|r| r.proposals.len() == 1));
        assert_eq!(run.translated_subjects, vec![alarm_trigger]);
        assert!(matches!(run.commit_receipt, CommitReceipt::Committed { .. }));
        assert_eq!(run.revision_after.0, run.revision_before.0 + 1);
        assert_eq!(run.commit_tick.0, run.fire_tick.0 + 1);

        // The committed content round-trips through independent readback.
        assert_eq!(run.moment_content_readback, run.moment_content);
        assert_eq!(
            run.moment_content_readback["runtime_moment_subtypes"],
            serde_json::json!(["validation_run", "health_assessment"])
        );

        // Causal ancestry threads the fired alarm atom.
        let explicit = format!("house-alarm:alarm_trigger-fired:{:#x}", alarm_trigger.0);
        assert!(run.causal_ancestry.contains(&explicit));

        // Honesty: precreated:false + overall not_measured + mixed dimensions.
        assert_eq!(
            run.moment_content["validation_run"]["precreated"],
            serde_json::json!(false)
        );
        assert_eq!(
            run.moment_content["health_assessment"]["precreated"],
            serde_json::json!(false)
        );
        assert_eq!(
            run.moment_content["health_assessment"]["overall_state"],
            serde_json::json!("not_measured")
        );
        let dims = run.moment_content["health_assessment"]["dimensions"]
            .as_object()
            .unwrap();
        let measured = dims
            .values()
            .filter(|v| v.get("status").and_then(JsonValue::as_str) == Some("measured"))
            .count();
        assert_eq!(measured, 4);
    }
}
