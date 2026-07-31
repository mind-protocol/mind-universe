//! Run the house-alarm construct FROM ITS GRAPH PROJECTION (resolver), not
//! hand-built constants — reproducing Rung 1 (approach -> the construct wakes ->
//! it notifies) end-to-end from the AUTHORED circuit.
//!
//! `house_alarm_fire` proves Rung 1 but assembles the alarm's atom circuit BY
//! HAND as hardcoded `EntityKey`/`RelationKey` constants and hand-written
//! clusters. This driver instead reads the AUTHORED `alarm_atom_circuit` from the
//! construct's graph projection (`fixtures/ontology/lumina-prime-house-alarm-v0.json`,
//! member `code:l2:lumina-prime:house-alarm-v0`), runs it through the generic
//! `construct_resolver`, and feeds the resolved runtime inputs to the SAME generic
//! `Supervisor::run_physics_deposit_phase`. Nothing about the alarm is hardcoded
//! here — the atoms, bonds, deposit and effect binding all come from the graph.
//!
//! The proven chain (identical to `house_alarm_fire`, but resolved not hand-built):
//!   intersection (SIMULATED PhysicsEvent) -> +deposit -> alarm_trigger threshold
//!   cross -> FIRE -> notify_emitter fires -> CANDIDATE notify EffectIntent ->
//!   executed through the capability transport -> EffectReceipt.
//!
//! `notify_emitter` is a TERMINAL effect atom in the authored circuit: on a fire
//! it surfaces exactly one notify candidate and conducts no energy onward. The
//! genuine EffectReceipt is produced by the authorized transport (below) and the
//! genuine crossing Moment by the commit path — neither is a shadow atom the
//! emitter energizes. (An earlier authoring made the emitter fan out into
//! `effect_receipt`/`crossing_ledger` atoms, which violated atom energy
//! conservation and starved it; the fixture now stops at the terminal emitter.)
//!
//! HONEST BOUNDARIES (identical to `house_alarm_fire`):
//!   * The crossing is SIMULATED: `physics_intersection_event`'s seed is the
//!     authored `external_measured_injection`, which on a live world MUST arrive
//!     from the real physics step via the physics-event -> atom-deposit bridge.
//!   * The store is NEVER mutated. A PhysicsEvent cannot mutate the store; the
//!     deposit is transient runtime state; `notify` is an external effect with a
//!     receipt, not a store delta. The driver proves the committed store is
//!     BYTE-IDENTICAL before and after.
//!   * The canonical fixture is READ as a portable JSON projection; it is not
//!     injected into a store here (see `inject_house_alarm` for that path).
//!
//! Usage: `house_alarm_resolved [scratch-store-dir]`
//!   scratch-store-dir defaults to a fresh unique dir under the system temp dir.
//!   NEVER pass a live store: this boots a fresh Genesis and needs an empty dir.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use serde_json::Value;

use universe_capabilities::{CapabilityHost, EffectAdapter, EffectExecutionReceipt, EffectReceipt};
use universe_core::Epistemic;
use universe_e2e::construct_resolver::{resolve_construct, AlarmAtomCircuit, ResolvedConstruct};
use universe_physics::{fired_atoms, AtomConvergence, AtomExecutionBudget};
use universe_supervisor::{HealthLevel, PhysicsDepositOutcome, Supervisor};

/// The capability the authored effect binding names. Registered on the transport
/// host so the surfaced candidate can be EXECUTED to a real receipt.
const NOTIFY_CAPABILITY: &str = "safe.notify";

/// A test transport standing in for the authorized external notification
/// channel. It acknowledges the payload; the ack is the measured transport result
/// carried in the EffectReceipt. In production this is the real authorized notify
/// transport, never invented text. (Identical to `house_alarm_fire`.)
struct NotifyTransport;
impl EffectAdapter for NotifyTransport {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut ack = b"notified:".to_vec();
        ack.extend_from_slice(payload);
        Ok(ack)
    }
}

/// The caller-supplied (graph-authority) execution budget. Mirrors
/// `house_alarm_fire`: a bounded, finite budget the supervisor's caller brings —
/// never a literal buried in the resolver.
fn budget() -> AtomExecutionBudget {
    AtomExecutionBudget {
        max_atoms: 16,
        max_bonds: 16,
        max_steps: 16,
        max_total_energy: 10_000,
    }
}

/// Read the authored `alarm_atom_circuit` block from the construct's graph
/// projection: the `code:l2:lumina-prime:house-alarm-v0` member's
/// `.content.alarm_atom_circuit`.
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

/// The measured facts of one resolved-wave run, keyed to the authored atom names.
struct ResolvedRun {
    resolved: ResolvedConstruct,
    outcome: PhysicsDepositOutcome,
    exec_receipt: EffectExecutionReceipt,
    effect_health: Epistemic<HealthLevel>,
    store_unchanged: bool,
    revision_unchanged: bool,
}

/// Boot a fresh SCRATCH supervisor, resolve the authored circuit, drive one
/// bounded Physics-phase deposit wave through the SAME generic primitive
/// `house_alarm_fire` uses, and EXECUTE the surfaced candidate through the
/// capability transport to a real receipt. Reads `&self` on the supervisor for
/// the wave; the store is never mutated. Errors (rather than silently passing) if
/// the resolved circuit surfaces no candidate — an honest failure, never a forced
/// fire.
fn drive(store_dir: &Path, genesis: &Path, fixture: &Path) -> Result<ResolvedRun, Box<dyn Error>> {
    let circuit = load_circuit(fixture)?;
    let resolved = resolve_construct(&circuit)
        .map_err(|error| format!("resolve_construct failed: {error:?}"))?;

    let mut supervisor = Supervisor::boot(store_dir, genesis)?;
    let revision_before = supervisor.revision();
    let bytes_before = read_all_files(store_dir)?;

    // (1)-(4): resolved sensor -> deposit -> construct wave. &self; no mutation.
    let outcome = supervisor.run_physics_deposit_phase(
        resolved.sensor_cluster.clone(),
        &resolved.deposit_bindings,
        resolved.construct_cluster.clone(),
        &resolved.effect_bindings,
        budget(),
    )?;

    // (5): the emitter's fire surfaced exactly one CANDIDATE notify EffectIntent.
    // If it is absent, the construct did not fire — fail honestly, do not fake.
    let candidate = outcome
        .candidate_effects
        .first()
        .cloned()
        .ok_or("no CANDIDATE notify EffectIntent surfaced — the resolved construct did not fire")?;

    // Rung-1 completion: EXECUTE the candidate through the authorized transport.
    // EffectIntent -> transport -> EffectReceipt. NOT a 4-verb store mutation.
    let mut capability_host = CapabilityHost::default();
    capability_host.register(NOTIFY_CAPABILITY, Box::new(NotifyTransport));
    let exec_receipt = capability_host.execute_measured(outcome.observed_at_tick, &candidate)?;
    supervisor.observe_transport_receipt(
        exec_receipt.capability.clone(),
        exec_receipt.idempotency_key.clone(),
        &exec_receipt.outcome,
    );
    let effect_health = supervisor.status().health.effect;

    let bytes_after = read_all_files(store_dir)?;
    let readback = supervisor.independent_readback()?;
    Ok(ResolvedRun {
        resolved,
        outcome,
        exec_receipt,
        effect_health,
        store_unchanged: bytes_before == bytes_after,
        revision_unchanged: readback.revision == revision_before,
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("HOUSE ALARM RESOLVED FAILED: {error}");
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
    let ResolvedRun {
        resolved,
        outcome,
        exec_receipt,
        effect_health,
        store_unchanged,
        revision_unchanged,
    } = &run;
    print_resolution(resolved);

    let key = |name: &str| *resolved.atom_keys.get(name).expect("authored atom key");
    let citizen_body_intersects = key("citizen_body_intersects");
    let alarm_trigger = key("alarm_trigger");
    let notify_emitter = key("notify_emitter");

    // --- (1)-(2) sensor: the SIMULATED crossing fires ----------------------
    println!("\n-- approach: a developer crosses the armed entry sensor (SIMULATED) --");
    let events = fired_atoms(&outcome.sensor);
    println!(
        "sensor fired atoms (physics events): {:?}",
        events.iter().map(|k| format!("{:#x}", k.0)).collect::<Vec<_>>()
    );
    assert!(
        events.contains(&citizen_body_intersects),
        "the measured crossing atom must have fired"
    );
    assert_eq!(
        outcome.sensor.convergence,
        AtomConvergence::Quiescent,
        "sensor cluster must reach quiescence"
    );
    assert!(outcome.sensor.energy.conserved, "sensor energy must be conserved");

    // --- (3) deposit: the bridge routed +100 onto the dormant trigger -------
    println!("\ndeposit routed by the bridge: {} request(s)", outcome.deposits.len());
    for deposit in &outcome.deposits {
        println!(
            "  +{} energy onto atom {:#x}   [{}]",
            deposit.energy, deposit.atom.0, deposit.provenance
        );
    }
    assert_eq!(outcome.deposits.len(), 1, "exactly one deposit expected");
    assert_eq!(outcome.deposits[0].atom, alarm_trigger, "deposit lands on alarm_trigger");
    assert_eq!(outcome.deposits[0].energy, 100, "deposit weight is the authored +100");

    // --- (4) the construct self-woke: trigger fires, conducts to emitter ----
    println!(
        "\nconstruct fired atoms (the alarm woke): {:?}",
        outcome
            .fired_construct_atoms
            .iter()
            .map(|k| format!("{:#x}", k.0))
            .collect::<Vec<_>>()
    );
    println!(
        "construct terminal-starved: {:?}",
        outcome
            .construct
            .terminal_starved
            .iter()
            .map(|k| format!("{:#x}", k.0))
            .collect::<Vec<_>>()
    );
    assert!(
        outcome.fired_construct_atoms.contains(&alarm_trigger),
        "the deposit must cross alarm_trigger's threshold and FIRE it"
    );
    assert!(
        outcome.fired_construct_atoms.contains(&notify_emitter),
        "the fired trigger must conduct into the terminal notify_emitter and FIRE it"
    );
    assert!(
        outcome.construct.terminal_starved.is_empty(),
        "the terminal emitter must not starve — it conducts no energy onward"
    );
    assert_eq!(outcome.construct.convergence, AtomConvergence::Quiescent);
    assert!(outcome.construct.energy.conserved, "construct energy must be conserved");

    // --- (5) the CANDIDATE notify effect, surfaced by the emitter's fire ----
    assert_eq!(
        outcome.candidate_effects.len(),
        1,
        "exactly one CANDIDATE notify EffectIntent expected"
    );
    println!(
        "\nCANDIDATE effect surfaced (proposal, from the authored effect binding): capability={} key={}",
        outcome.candidate_effects[0].capability, outcome.candidate_effects[0].idempotency_key
    );
    assert_eq!(
        outcome.candidate_effects[0].capability, NOTIFY_CAPABILITY,
        "the candidate carries the authored capability"
    );

    // --- the EXECUTED external effect: EffectIntent -> transport -> receipt --
    println!("\nexecuted the candidate through the capability transport:");
    println!("  transport_attempted: {}", exec_receipt.transport_attempted);
    println!("  outcome            : {:?}", exec_receipt.outcome);
    assert!(exec_receipt.transport_attempted, "the notify transport must actually run");
    assert!(
        matches!(exec_receipt.outcome, EffectReceipt::TransportSucceeded { .. }),
        "the notify transport must succeed and yield a receipt"
    );
    println!("reinjected as effect health: {effect_health:?}");
    assert_eq!(
        *effect_health,
        Epistemic::Measured(HealthLevel::Nominal),
        "one observed successful transport => effect health Measured(Nominal)"
    );

    // --- the store was NEVER mutated ---------------------------------------
    println!("\n-- store integrity (a PhysicsEvent / external effect never mutates the store) --");
    println!("committed store byte-identical before/after: {store_unchanged}");
    assert!(
        *store_unchanged,
        "committed store bytes changed — a PhysicsEvent / external effect must NOT mutate the store"
    );
    assert!(*revision_unchanged, "committed revision must be unchanged");

    println!("\nRESULT: Rung 1 reproduced FROM the construct's GRAPH PROJECTION (resolver), not hand-built constants.");
    println!("  chain: intersection (SIMULATED) -> +deposit -> alarm_trigger cross -> FIRE -> notify_emitter FIRE");
    println!("         -> CANDIDATE notify EffectIntent -> EffectReceipt (external effect, receipted)");
    println!("  every atom/bond/deposit/effect came from the authored alarm_atom_circuit — nothing hardcoded.");
    println!("  the committed store is byte-identical: nothing was mutated by the event or the effect.");
    println!("  HONEST BOUNDARY: the crossing is simulated (authored injection), not yet a real Rapier collider.");

    if env::args_os().nth(1).is_none() {
        let _ = fs::remove_dir_all(&store_dir);
    }
    Ok(())
}

fn print_resolution(resolved: &ResolvedConstruct) {
    println!("\n-- resolved FROM the authored circuit --");
    println!(
        "sensor cluster: {} atoms, {} bonds",
        resolved.sensor_cluster.atoms.len(),
        resolved.sensor_cluster.bonds.len()
    );
    println!(
        "construct cluster: {} atoms, {} bonds",
        resolved.construct_cluster.atoms.len(),
        resolved.construct_cluster.bonds.len()
    );
    println!("deposit bindings: {}", resolved.deposit_bindings.len());
    for deposit in &resolved.deposit_bindings {
        println!(
            "  event {:#x} -> +{} onto {:#x}",
            deposit.trigger.0, deposit.weight, deposit.target.0
        );
    }
    println!("effect bindings: {}", resolved.effect_bindings.len());
    println!("atom key map (authored -> EntityKey):");
    for (name, key) in &resolved.atom_keys {
        println!("  {name:<28} {:#x}", key.0);
    }
}

fn default_scratch_store() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!("house-alarm-resolved-{}-{nanos}", std::process::id()))
}

/// Read every file under `dir` into a path-keyed map of raw bytes, for a literal
/// byte-identity comparison of the committed store before and after the flow.
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

    /// The resolved authored circuit, driven over a fresh tempdir scratch store,
    /// reproducing Rung 1 end-to-end FROM the graph projection: the sensor fires,
    /// the bridge deposits onto the trigger, the trigger fires the terminal
    /// emitter, exactly one notify candidate surfaces and is executed to a real
    /// EffectReceipt, and the committed store is byte-identical throughout.
    #[test]
    fn resolved_authored_circuit_reproduces_rung1_without_mutating_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let genesis = manifest.join("../../fixtures/genesis/minimal-genesis.json");
        let fixture = manifest.join("../../fixtures/ontology/lumina-prime-house-alarm-v0.json");
        fs::create_dir_all(&store_dir).unwrap();

        let run = drive(&store_dir, &genesis, &fixture).unwrap();
        let key = |name: &str| *run.resolved.atom_keys.get(name).unwrap();
        let citizen_body_intersects = key("citizen_body_intersects");
        let alarm_trigger = key("alarm_trigger");
        let notify_emitter = key("notify_emitter");

        // The resolver split the authored circuit: 3 sensor atoms, a terminal
        // 2-atom construct half (alarm_trigger + notify_emitter), one deposit.
        assert_eq!(run.resolved.sensor_cluster.atoms.len(), 3);
        assert_eq!(run.resolved.construct_cluster.atoms.len(), 2);
        assert_eq!(run.resolved.deposit_bindings.len(), 1);
        assert_eq!(run.resolved.deposit_bindings[0].trigger, citizen_body_intersects);
        assert_eq!(run.resolved.deposit_bindings[0].target, alarm_trigger);
        assert_eq!(run.resolved.deposit_bindings[0].weight, 100);
        assert_eq!(run.resolved.effect_bindings.len(), 1);

        // Sensor -> deposit -> trigger -> terminal emitter, all firing.
        let events = fired_atoms(&run.outcome.sensor);
        assert!(events.contains(&citizen_body_intersects));
        assert_eq!(run.outcome.sensor.convergence, AtomConvergence::Quiescent);
        assert!(run.outcome.sensor.energy.conserved);
        assert_eq!(run.outcome.deposits.len(), 1);
        assert_eq!(run.outcome.deposits[0].atom, alarm_trigger);
        assert_eq!(run.outcome.deposits[0].energy, 100);
        assert!(run.outcome.fired_construct_atoms.contains(&alarm_trigger));
        assert!(run.outcome.fired_construct_atoms.contains(&notify_emitter));
        assert!(run.outcome.construct.terminal_starved.is_empty());
        assert_eq!(run.outcome.construct.convergence, AtomConvergence::Quiescent);
        assert!(run.outcome.construct.energy.conserved);

        // Exactly one CANDIDATE notify effect, executed to a real receipt.
        assert_eq!(run.outcome.candidate_effects.len(), 1);
        assert_eq!(run.outcome.candidate_effects[0].capability, NOTIFY_CAPABILITY);
        assert!(run.exec_receipt.transport_attempted);
        assert!(matches!(
            run.exec_receipt.outcome,
            EffectReceipt::TransportSucceeded { .. }
        ));
        assert_eq!(
            run.effect_health,
            Epistemic::Measured(HealthLevel::Nominal)
        );

        // The committed store is byte-identical: nothing was mutated.
        assert!(run.store_unchanged);
        assert!(run.revision_unchanged);
    }
}
