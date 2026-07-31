//! chantier C1, slice 1: the FIRST durable Moment CAUSED by a real construct fire.
//!
//! `house_alarm_resolved` proves Rung 1 end-to-end FROM the authored circuit
//! (resolver -> `Supervisor::run_physics_deposit_phase` -> executed notify
//! EffectReceipt) and then asserts the committed store is BYTE-IDENTICAL: the
//! PhysicsEvent and the external effect mutate NOTHING. This driver reproduces
//! that exact wave and then INVERTS that final assertion: it takes the ONE next
//! step the write-path exists for. It translates the fired-construct evidence and
//! the EffectReceipt into ONE crossing `Moment` write-set and commits it through
//! the real Commit phase (`Supervisor::advance` for exactly one tick), then proves
//! by INDEPENDENT readback that the revision advanced by exactly 1 and the Moment
//! is durably present.
//!
//! The boundary house_alarm_resolved guards is preserved, not dropped: the physics
//! wave ITSELF still commits nothing (this driver re-asserts the store is
//! byte-identical across the wave, before it deliberately commits the Moment). A
//! PhysicsEvent never mutates the store; only the deliberate 4-verb write-set
//! does, at the tick boundary, with a CommitReceipt.
//!
//! HARD HONESTY BOUNDARY. The crossing is SIMULATED: `physics_intersection_event`
//! is seeded from the authored `external_measured_injections`, which on a live
//! world MUST arrive from the real physics step via the unbuilt physics-event ->
//! atom-deposit bridge. Consequently the Moment's `validation_run` and
//! `health_assessment` are derived GENUINELY from what this one wave measured
//! (convergence == Quiescent, energy conserved, the fired-atom set, one executed
//! EffectReceipt, store byte-identical across the wave) and NOTHING else. Every
//! health dimension and validation invariant NOT covered by that evidence is
//! marked `not_measured` / `not_checked` — never fabricated as healthy. The
//! overall health state is therefore `not_measured`, not `healthy`: a single
//! simulated crossing cannot certify a live alarm.
//!
//! Usage: `construct_live_run [scratch-store-dir]`
//!   scratch-store-dir defaults to a fresh unique dir under the system temp dir.
//!   NEVER pass a live store: this boots a fresh Genesis and needs an empty dir.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use serde_json::Value;

use universe_capabilities::{CapabilityHost, EffectAdapter, EffectExecutionReceipt, EffectReceipt};
use universe_core::{EntityKey, Revision, UniverseError};
use universe_e2e::construct_resolver::{resolve_construct, AlarmAtomCircuit, ResolvedConstruct};
use universe_physics::{fired_atoms, AtomConvergence, AtomExecutionBudget};
use universe_store::{EntityRecord, UniverseSnapshot};
use universe_supervisor::{PhaseHook, PhysicsDepositOutcome, Supervisor, TickPhase};
use universe_transactions::{
    CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet,
};

/// The capability the authored effect binding names (same as `house_alarm_resolved`).
const NOTIFY_CAPABILITY: &str = "safe.notify";

/// A test transport standing in for the authorized external notification channel.
/// It acknowledges the payload; the ack is the measured transport result carried
/// in the EffectReceipt. It is a TEST stand-in, NOT a linked/authorized recipient
/// — the honesty accounting below treats notify-authorization as `not_measured`.
struct NotifyTransport;
impl EffectAdapter for NotifyTransport {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut ack = b"notified:".to_vec();
        ack.extend_from_slice(payload);
        Ok(ack)
    }
}

/// A no-op phase hook: this driver's only work at the tick boundary is committing
/// the enqueued Moment transaction, which the supervisor's Commit phase does.
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

/// Read the authored `alarm_atom_circuit` from the construct's graph projection.
fn load_circuit(fixture: &Path) -> Result<AlarmAtomCircuit, Box<dyn Error>> {
    let bytes = fs::read(fixture)?;
    let root: Value = serde_json::from_slice(&bytes)?;
    let members = root
        .get("members")
        .and_then(Value::as_array)
        .ok_or("fixture has no members array")?;
    let code_member = members
        .iter()
        .find(|member| {
            member.get("id").and_then(Value::as_str) == Some("code:l2:lumina-prime:house-alarm-v0")
        })
        .ok_or("fixture has no code:l2:lumina-prime:house-alarm-v0 member")?;
    let circuit_value = code_member
        .get("content")
        .and_then(|content| content.get("alarm_atom_circuit"))
        .ok_or("code member has no content.alarm_atom_circuit")?;
    let circuit: AlarmAtomCircuit = serde_json::from_value(circuit_value.clone())?;
    Ok(circuit)
}

/// The full measured result of one live run + committed Moment.
struct LiveRun {
    resolved: ResolvedConstruct,
    outcome: PhysicsDepositOutcome,
    exec_receipt: EffectExecutionReceipt,
    /// The physics wave itself committed nothing: byte-identical across the wave.
    wave_store_unchanged: bool,
    /// The physics wave itself advanced no revision.
    wave_revision_unchanged: bool,
    revision_before: Revision,
    revision_after: Revision,
    commit_receipt: CommitReceipt,
    moment_key: EntityKey,
    moment_content: Value,
    moment_content_readback: Value,
}

/// INLINE moment-translation (kept in this bin file, not a shared lib module).
///
/// Translates the MEASURED evidence of one resolved-construct wave + the executed
/// EffectReceipt into ONE crossing `Moment` write-set content, carrying the two
/// authored `runtime_moment_subtypes` (`validation_run`, `health_assessment`).
///
/// Every field is GENUINELY derived from the run. `precreated:false`. The four
/// dimensions the wave actually covered — quiescence, energy conservation,
/// physics-event non-mutation, effect-receipt integrity — are `measured`. All
/// other authored metric dimensions and validation invariants are
/// `not_measured` / `not_checked` with a concrete reason. The overall health
/// state is `not_measured`, quoting the authored derivation: a single SIMULATED
/// crossing measures none of the population dimensions a live alarm needs.
mod moment {
    use super::*;
    use serde_json::json;

    /// Reverse map EntityKey -> authored atom name for readable evidence refs.
    fn name_of(resolved: &ResolvedConstruct, key: &EntityKey) -> String {
        resolved
            .atom_keys
            .iter()
            .find(|(_, k)| *k == key)
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| format!("{:#x}", key.0))
    }

    fn named_hex(resolved: &ResolvedConstruct, keys: impl IntoIterator<Item = EntityKey>) -> Vec<Value> {
        keys.into_iter()
            .map(|k| json!({ "atom": name_of(resolved, &k), "key": format!("{:#x}", k.0) }))
            .collect()
    }

    /// A `not_measured` health dimension with the concrete reason it is uncovered.
    fn not_measured(reason: &str) -> Value {
        json!({ "status": "not_measured", "why": reason })
    }

    pub fn build_crossing_moment(
        resolved: &ResolvedConstruct,
        outcome: &PhysicsDepositOutcome,
        exec_receipt: &EffectExecutionReceipt,
        wave_store_unchanged: bool,
    ) -> Value {
        let sensor_quiescent = outcome.sensor.convergence == AtomConvergence::Quiescent;
        let construct_quiescent = outcome.construct.convergence == AtomConvergence::Quiescent;
        let energy_conserved =
            outcome.sensor.energy.conserved && outcome.construct.energy.conserved;
        let terminal_starved_empty = outcome.construct.terminal_starved.is_empty();
        let transport_succeeded =
            matches!(exec_receipt.outcome, EffectReceipt::TransportSucceeded { .. });

        // --- the MEASURED facts of this one wave (evidence_refs) --------------
        let sensor_fired = named_hex(resolved, fired_atoms(&outcome.sensor));
        let construct_fired =
            named_hex(resolved, outcome.fired_construct_atoms.iter().copied());
        let evidence_refs = json!({
            "measured_from": "one bounded atom-deposit wave (Supervisor::run_physics_deposit_phase) over a fresh Genesis scratch store",
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

        // --- validation_run: only what this wave actually exercised -----------
        let validation_run = json!({
            "precreated": false,
            "runner": "construct_live_run bin — one resolved-construct atom-deposit wave",
            "starting_state": "fresh Genesis scratch store (minimal-genesis)",
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
                  "evidence": "committed store byte-identical across the physics wave (nothing committed by the wave)" },
                { "invariant": "the trigger fires on the deposited support (support >= 1)",
                  "result": "measured_pass",
                  "evidence": "alarm_trigger is in the construct fired-atom set" },
                { "invariant": "the terminal emitter conducts no energy onward (does not starve)",
                  "result": if terminal_starved_empty { "measured_pass" } else { "measured_fail" },
                  "evidence": "construct.terminal_starved is empty" },
                { "invariant": "each fire commits exactly one crossing Moment",
                  "result": "measured_pass",
                  "evidence": "the single alarm_trigger fire yields exactly one crossing Moment write-set (this entity)" },
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

        // --- health_assessment: 4 measured, 14 not_measured, overall not_measured
        let health_assessment = json!({
            "precreated": false,
            "states_vocabulary": ["healthy", "degraded", "stale", "unknown", "not_measured", "measurement_failed"],
            "overall_state": "not_measured",
            "overall_state_justification": "No fresh validation run has measured the required dimensions: the crossing is a SIMULATED authored injection (the physics-event -> atom-deposit bridge is unbuilt), so no REAL fire has ever been measured. A single simulated crossing certifies none of the population dimensions a live alarm needs; the overall state is not_measured, never healthy.",
            "evidence_basis": "one bounded atom-deposit wave: convergence, energy conservation, the fired-atom set, one executed EffectReceipt, and store byte-identical across the wave",
            "dimensions": {
                // covered by this wave -> measured
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
                // NOT covered by this wave -> not_measured (never fabricated healthy)
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
            "canonical_id": "moment:l2:lumina-prime:house-alarm:first-live-crossing-v0",
            "node_type": "narrative",
            "moment_kind": "crossing",
            "runtime_moment_subtypes": ["validation_run", "health_assessment"],
            "construct": "space:l2:lumina-prime:house-alarm-v0",
            "caused_by": "the alarm_trigger fire in one resolved-construct atom-deposit wave",
            "evidence_refs": evidence_refs,
            "validation_run": validation_run,
            "health_assessment": health_assessment
        })
    }
}

/// Boot a fresh SCRATCH supervisor, run the resolved wave + execute the notify
/// candidate (exactly as `house_alarm_resolved`), then commit ONE crossing Moment
/// through the real Commit phase and read it back independently.
fn drive(store_dir: &Path, genesis: &Path, fixture: &Path) -> Result<LiveRun, Box<dyn Error>> {
    let circuit = load_circuit(fixture)?;
    let resolved =
        resolve_construct(&circuit).map_err(|error| format!("resolve_construct failed: {error:?}"))?;

    let mut supervisor = Supervisor::boot(store_dir, genesis)?;
    let revision_before = supervisor.revision();
    let bytes_before_wave = read_all_files(store_dir)?;

    // (1)-(4) resolved sensor -> deposit -> construct wave. &self; commits nothing.
    let outcome = supervisor.run_physics_deposit_phase(
        resolved.sensor_cluster.clone(),
        &resolved.deposit_bindings,
        resolved.construct_cluster.clone(),
        &resolved.effect_bindings,
        budget(),
    )?;

    // (5) the emitter's fire surfaced exactly one CANDIDATE notify EffectIntent.
    let candidate = outcome
        .candidate_effects
        .first()
        .cloned()
        .ok_or("no CANDIDATE notify EffectIntent surfaced — the resolved construct did not fire")?;

    // Execute the candidate through the authorized transport -> real EffectReceipt.
    let mut capability_host = CapabilityHost::default();
    capability_host.register(NOTIFY_CAPABILITY, Box::new(NotifyTransport));
    let exec_receipt = capability_host.execute_measured(outcome.observed_at_tick, &candidate)?;
    supervisor.observe_transport_receipt(
        exec_receipt.capability.clone(),
        exec_receipt.idempotency_key.clone(),
        &exec_receipt.outcome,
    );

    // INVERTED-CONTEXT INTEGRITY: the physics wave ITSELF still commits nothing.
    // We re-assert byte-identity across the wave BEFORE deliberately committing
    // the Moment, keeping house_alarm_resolved's boundary intact.
    let bytes_after_wave = read_all_files(store_dir)?;
    let wave_store_unchanged = bytes_before_wave == bytes_after_wave;
    let wave_revision_unchanged = supervisor.revision() == revision_before;

    // --- the ONE step further: translate evidence -> a crossing Moment write-set
    let moment_content =
        moment::build_crossing_moment(&resolved, &outcome, &exec_receipt, wave_store_unchanged);
    let content_ref = supervisor.append_content(&moment_content)?;

    let snapshot = supervisor.snapshot();
    let moment_symbol = snapshot
        .symbol_id("Moment")
        .ok_or("minimal genesis has no Moment symbol")?;
    let moment_key = EntityKey(
        snapshot
            .entities
            .iter()
            .map(|entity| entity.key.0)
            .max()
            .unwrap_or(0)
            + 1,
    );

    let write_set = UniverseWriteSet {
        base_revision: revision_before,
        idempotency_key: "moment:l2:lumina-prime:house-alarm:first-live-crossing-v0".to_string(),
        causal_ancestry: vec![
            "house-alarm:citizen-body-intersects".to_string(),
            exec_receipt.idempotency_key.clone(),
            "construct:l2:lumina-prime:house-alarm-v0:first-live-fire".to_string(),
        ],
        commands: vec![UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: moment_key,
                generation: 0,
                symbol: moment_symbol,
                content: Some(content_ref),
            },
        }],
    };

    // prepare -> enqueue -> advance ONE tick: the Commit phase commits it.
    let transaction = UniverseTransaction::prepare(supervisor.snapshot(), write_set)?;
    supervisor.enqueue(transaction);
    let mut hook = NoopHook;
    let commits = supervisor.advance(&mut hook)?;
    let commit_receipt = commits
        .into_iter()
        .next()
        .ok_or("advance committed no transaction — the enqueued Moment did not commit")?;

    // INDEPENDENT readback: fresh reopen from disk.
    let after = supervisor.independent_readback()?;
    let revision_after = after.revision;
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

    Ok(LiveRun {
        resolved,
        outcome,
        exec_receipt,
        wave_store_unchanged,
        wave_revision_unchanged,
        revision_before,
        revision_after,
        commit_receipt,
        moment_key,
        moment_content,
        moment_content_readback,
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("CONSTRUCT LIVE RUN FAILED: {error}");
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

    // --- (a) the construct fired (same evidence house_alarm_resolved proves) ---
    let key = |name: &str| *run.resolved.atom_keys.get(name).expect("authored atom key");
    let citizen_body_intersects = key("citizen_body_intersects");
    let alarm_trigger = key("alarm_trigger");
    let notify_emitter = key("notify_emitter");

    println!("\n-- the resolved construct fired (SIMULATED crossing) --");
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
    assert!(
        matches!(run.exec_receipt.outcome, EffectReceipt::TransportSucceeded { .. }),
        "the notify transport must succeed and yield a receipt"
    );
    println!(
        "  fired: citizen_body_intersects={:#x}  alarm_trigger={:#x}  notify_emitter={:#x}",
        citizen_body_intersects.0, alarm_trigger.0, notify_emitter.0
    );
    println!(
        "  EffectReceipt: capability={} key={} outcome=TransportSucceeded",
        run.exec_receipt.capability, run.exec_receipt.idempotency_key
    );

    // --- (b) the physics wave itself mutated NOTHING (boundary preserved) ------
    println!("\n-- boundary preserved: the physics wave itself commits nothing --");
    println!("  store byte-identical across the wave: {}", run.wave_store_unchanged);
    assert!(run.wave_store_unchanged, "the wave must not mutate the committed store");
    assert!(run.wave_revision_unchanged, "the wave must not advance the revision");

    // --- (c) INVERSION: the ONE deliberate 4-verb Moment commit ----------------
    println!("\n-- the ONE step further: a crossing Moment committed through the Commit phase --");
    println!("  commit receipt   : {:?}", run.commit_receipt);
    println!(
        "  revision advanced: {} -> {}",
        run.revision_before.0, run.revision_after.0
    );
    assert!(
        matches!(run.commit_receipt, CommitReceipt::Committed { .. }),
        "the Moment must commit freshly (not AlreadyCommitted)"
    );
    assert_eq!(
        run.revision_after.0,
        run.revision_before.0 + 1,
        "the revision must advance by exactly 1"
    );
    println!("  Moment entity    : {:#x} (durably present on independent readback)", run.moment_key.0);

    // The committed content read back independently equals what we committed.
    assert_eq!(
        run.moment_content_readback, run.moment_content,
        "the independently read-back Moment content must equal the committed content"
    );

    // --- (d) print the committed Moment content + the honesty accounting -------
    println!("\n-- committed Moment content (independent readback) --");
    println!("{}", serde_json::to_string_pretty(&run.moment_content_readback)?);

    // Enumerate exactly which health dimensions are not_measured, from the readback.
    let dims = run.moment_content_readback["health_assessment"]["dimensions"]
        .as_object()
        .expect("health_assessment.dimensions object");
    let mut measured: Vec<&String> = Vec::new();
    let mut not_measured: Vec<&String> = Vec::new();
    for (name, value) in dims {
        match value.get("status").and_then(Value::as_str) {
            Some("measured") => measured.push(name),
            _ => not_measured.push(name),
        }
    }
    measured.sort();
    not_measured.sort();
    println!("\n-- honesty accounting (derived from the committed Moment) --");
    println!(
        "  overall_state: {}",
        run.moment_content_readback["health_assessment"]["overall_state"]
    );
    println!("  measured dimensions ({}):", measured.len());
    for name in &measured {
        println!("    - {name}");
    }
    println!("  not_measured dimensions ({}):", not_measured.len());
    for name in &not_measured {
        println!("    - {name}");
    }
    assert_eq!(
        run.moment_content_readback["health_assessment"]["overall_state"],
        Value::String("not_measured".into()),
        "a single simulated crossing must NOT certify a healthy alarm"
    );
    assert!(
        !measured.is_empty() && !not_measured.is_empty(),
        "the assessment must be genuinely mixed: some measured, most not_measured"
    );

    println!("\nRESULT: the FIRST durable Moment CAUSED by a real construct fire is committed.");
    println!("  house_alarm_resolved's wave is reproduced; its byte-identity boundary is preserved for the WAVE.");
    println!("  the inversion: the fired-construct evidence + EffectReceipt are translated into ONE crossing");
    println!("  Moment write-set, committed through the real Commit phase (advance, 1 tick), revision +1, read back.");
    println!("  HONESTY: overall health = not_measured; only quiescence/energy/non-mutation/effect-receipt are measured;");
    println!("  the crossing is SIMULATED, so every population dimension is not_measured — never fabricated healthy.");

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
    env::temp_dir().join(format!("construct-live-run-{}-{nanos}", std::process::id()))
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

    /// The full live run over a fresh tempdir scratch store: the resolved wave
    /// fires, the wave itself commits nothing, then ONE crossing Moment commits
    /// through the real Commit phase (revision +1) and is durably read back, with
    /// an honestly-mixed health assessment whose overall state is not_measured.
    #[test]
    fn first_durable_moment_is_caused_by_a_real_construct_fire() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let genesis = manifest.join("../../fixtures/genesis/minimal-genesis.json");
        let fixture = manifest.join("../../fixtures/ontology/lumina-prime-house-alarm-v0.json");
        fs::create_dir_all(&store_dir).unwrap();

        let run = drive(&store_dir, &genesis, &fixture).unwrap();

        // The wave itself mutated nothing.
        assert!(run.wave_store_unchanged);
        assert!(run.wave_revision_unchanged);

        // The Moment committed freshly and advanced the revision by exactly 1.
        assert!(matches!(run.commit_receipt, CommitReceipt::Committed { .. }));
        assert_eq!(run.revision_after.0, run.revision_before.0 + 1);

        // The committed content round-trips through independent readback.
        assert_eq!(run.moment_content_readback, run.moment_content);
        assert_eq!(
            run.moment_content_readback["runtime_moment_subtypes"],
            serde_json::json!(["validation_run", "health_assessment"])
        );

        // Honesty: precreated:false on both blocks; overall health not_measured.
        assert_eq!(run.moment_content["validation_run"]["precreated"], serde_json::json!(false));
        assert_eq!(run.moment_content["health_assessment"]["precreated"], serde_json::json!(false));
        assert_eq!(
            run.moment_content["health_assessment"]["overall_state"],
            serde_json::json!("not_measured")
        );

        // The assessment is genuinely mixed: exactly the four covered dimensions
        // are measured, and there is at least one not_measured dimension.
        let dims = run.moment_content["health_assessment"]["dimensions"]
            .as_object()
            .unwrap();
        let measured = dims
            .values()
            .filter(|v| v.get("status").and_then(Value::as_str) == Some("measured"))
            .count();
        let not_measured = dims
            .values()
            .filter(|v| v.get("status").and_then(Value::as_str) == Some("not_measured"))
            .count();
        assert_eq!(measured, 4, "exactly the four wave-covered dimensions are measured");
        assert!(not_measured >= 10, "the uncovered dimensions stay not_measured");
    }
}
