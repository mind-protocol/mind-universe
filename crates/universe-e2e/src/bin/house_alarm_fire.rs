//! Rung 1, at mechanism level: **approach -> the construct wakes -> it notifies.**
//!
//! This driver assembles the Lumina Prime house-alarm construct
//! (`fixtures/ontology/lumina-prime-house-alarm-v0.json`) as two bounded
//! `LocalAtomCluster`s, SIMULATES a developer crossing the armed entry sensor,
//! and drives the just-landed physics-event -> Atom-deposit bridge through the
//! generic `Supervisor::run_physics_deposit_phase`. The construct that was
//! dormant self-wakes because a deposit crosses its trigger threshold, and the
//! fire surfaces a CANDIDATE `notify` EffectIntent. That candidate is then
//! EXECUTED through the capability host (EffectIntent -> transport ->
//! EffectReceipt) — a receipted EXTERNAL effect, never a 4-verb store mutation.
//!
//! The end-to-end chain proven here is exactly the fixture's:
//!   intersection (PhysicsEvent, never mutates) -> +energy deposit -> threshold
//!   cross -> fire -> CANDIDATE EffectIntent{notify} -> EffectReceipt -> Moment.
//!
//! HONEST BOUNDARIES (do not overclaim):
//!   * A real Rapier collider intersection is a LATER step. Here the crossing is
//!     SIMULATED: the sensor cluster's armed-sensor and intersection-event atoms
//!     are seeded so the AND-gate `citizen_body_intersects` fires. Its firing —
//!     an `EntityKey` handle in the observed-event set — stands in for the real
//!     collider handle that `resolve_physics_event_deposits` will one day
//!     consume from the running physics step. Seeding it by hand proves the
//!     circuit + bridge shape, NOT a real entry.
//!   * The store is NEVER mutated by this flow. A PhysicsEvent cannot mutate the
//!     store; the deposit is transient runtime state; `notify` is an external
//!     effect with a receipt, not a store delta. The driver proves the committed
//!     store is BYTE-IDENTICAL before and after the whole flow.
//!   * The clusters are hand-built to MIRROR the authored fixture circuit. The
//!     canonical fixture is not injected into a store here; that (and driving the
//!     bridge from a real collider) is the next rung.
//!
//! Usage: `house_alarm_fire [scratch-store-dir]`
//!   scratch-store-dir defaults to a fresh unique dir under the system temp dir.
//!   NEVER pass a live store: this boots a fresh Genesis and needs an empty dir.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use universe_capabilities::{CapabilityHost, EffectAdapter, EffectExecutionReceipt, EffectIntent};
use universe_core::{EntityKey, Epistemic, RelationKey, Tick};
use universe_physics::{
    AtomBond, AtomConvergence, AtomExecutionBudget, AtomSpec, BondPolarity, LocalAtomCluster,
    PhysicsEventDeposit,
};
use universe_supervisor::{
    HealthLevel, PhysicsDepositOutcome, PhysicsEffectBinding, Supervisor,
};

// --- The alarm circuit's stable Atom / Bond handles (mirror the fixture). ---
// SENSOR side (the "physics event" producer):
const ENTRY_SENSOR_ARMED: EntityKey = EntityKey(0xA1);
const PHYSICS_INTERSECTION_EVENT: EntityKey = EntityKey(0xA2);
const CITIZEN_BODY_INTERSECTS: EntityKey = EntityKey(0xA3); // the AND-gate crossing
const SENSOR_TO_INTERSECTION: RelationKey = RelationKey(0xB1);
const PHYSICS_EVENT_TO_INTERSECTION: RelationKey = RelationKey(0xB2);
// CONSTRUCT side (the dormant alarm that self-wakes on the deposit):
const ALARM_TRIGGER: EntityKey = EntityKey(0xA4);
const NOTIFY_EMITTER: EntityKey = EntityKey(0xA5);
const TRIGGER_TO_EMITTER: RelationKey = RelationKey(0xB3);

/// One measured support = 100 energy units; the trigger fires at one crossing.
/// These mirror the fixture's `trigger_rules` (support_per_crossing = 100,
/// trigger_threshold = 100, fires_at_support = 1).
const ONE_SUPPORT: u64 = 100;
const NOTIFY_CAPABILITY: &str = "safe.notify";

/// A test transport standing in for the authorized external notification
/// channel. It acknowledges the payload; the ack is the measured transport
/// result carried in the EffectReceipt. In production this is the real
/// authorized notify transport (email/push/etc.), never invented text.
struct NotifyTransport;
impl EffectAdapter for NotifyTransport {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut ack = b"notified:".to_vec();
        ack.extend_from_slice(payload);
        Ok(ack)
    }
}

/// The SENSOR cluster: the armed entry sensor + a SIMULATED intersection event,
/// AND-gated into the measured crossing atom `citizen_body_intersects`. Mirrors
/// the fixture's alarm_atom_circuit sensor half. The two support atoms are
/// seeded so the gate fires — this is the simulation of a citizen body crossing
/// the armed sensor (a real Rapier collider is the later step).
fn sensor_cluster() -> LocalAtomCluster {
    LocalAtomCluster {
        atoms: vec![
            // The armed sensor: a physical condition placed in the field.
            AtomSpec {
                key: ENTRY_SENSOR_ARMED,
                threshold: ONE_SUPPORT,
                seed_energy: ONE_SUPPORT,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            },
            // SIMULATED intersection: seeded here in place of the real collider
            // handle the running physics step will one day supply.
            AtomSpec {
                key: PHYSICS_INTERSECTION_EVENT,
                threshold: ONE_SUPPORT,
                seed_energy: ONE_SUPPORT,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            },
            // The measured crossing: fires only when BOTH the armed sensor and
            // the intersection event conduct (an AND gate on both bonds).
            AtomSpec {
                key: CITIZEN_BODY_INTERSECTS,
                threshold: 2 * ONE_SUPPORT,
                seed_energy: 0,
                required_supports: vec![SENSOR_TO_INTERSECTION, PHYSICS_EVENT_TO_INTERSECTION],
                inhibition_threshold: None,
            },
        ],
        bonds: vec![
            AtomBond {
                key: SENSOR_TO_INTERSECTION,
                source: ENTRY_SENSOR_ARMED,
                target: CITIZEN_BODY_INTERSECTS,
                polarity: BondPolarity::Support,
                energy: ONE_SUPPORT,
            },
            AtomBond {
                key: PHYSICS_EVENT_TO_INTERSECTION,
                source: PHYSICS_INTERSECTION_EVENT,
                target: CITIZEN_BODY_INTERSECTS,
                polarity: BondPolarity::Support,
                energy: ONE_SUPPORT,
            },
        ],
        injections: Vec::new(),
    }
}

/// The CONSTRUCT cluster: the dormant alarm. `alarm_trigger` fires on a single
/// deposited support (it carries NO required-support bond — the deposit IS its
/// wake), conducting into `notify_emitter`. Nothing here is seeded: the cluster
/// is inert until the bridge lands a deposit onto `alarm_trigger`.
fn construct_cluster() -> LocalAtomCluster {
    LocalAtomCluster {
        atoms: vec![
            AtomSpec {
                key: ALARM_TRIGGER,
                threshold: ONE_SUPPORT,
                seed_energy: 0,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            },
            AtomSpec {
                key: NOTIFY_EMITTER,
                threshold: ONE_SUPPORT,
                seed_energy: 0,
                required_supports: vec![TRIGGER_TO_EMITTER],
                inhibition_threshold: None,
            },
        ],
        bonds: vec![AtomBond {
            key: TRIGGER_TO_EMITTER,
            source: ALARM_TRIGGER,
            target: NOTIFY_EMITTER,
            polarity: BondPolarity::Support,
            energy: ONE_SUPPORT,
        }],
        injections: Vec::new(),
    }
}

/// The DepositBond: WHEN the measured crossing fires (a physics event), deposit
/// one support onto the alarm's trigger atom. This is the `event -> +energy`
/// edge; the crossing NEVER mutates the store.
fn deposit_bindings() -> Vec<PhysicsEventDeposit> {
    vec![PhysicsEventDeposit {
        trigger: CITIZEN_BODY_INTERSECTS,
        target: ALARM_TRIGGER,
        weight: ONE_SUPPORT,
    }]
}

/// The CANDIDATE `notify` EffectIntent the emitter atom's fire proposes. It is a
/// proposal only until executed through the capability transport.
fn notify_candidate() -> EffectIntent {
    EffectIntent {
        capability: NOTIFY_CAPABILITY.into(),
        idempotency_key: "house-alarm:notify:citizen-body-crossing-v0".into(),
        payload: b"Entree detectee : un corps a franchi le seuil.".to_vec(),
        // Generous deadline: this is a simulated candidate that has not been
        // through the trigger scheduler, so no real eligibility tick applies.
        deadline_tick: Tick(1_000),
        causal_ancestry: vec!["house-alarm:citizen-body-intersects".into()],
    }
}

/// The emitter's fire is bound to the notify candidate: only when
/// `notify_emitter` crosses its threshold is the notify EffectIntent surfaced.
fn effect_bindings() -> Vec<PhysicsEffectBinding> {
    vec![PhysicsEffectBinding {
        atom: NOTIFY_EMITTER,
        candidate: notify_candidate(),
    }]
}

/// Bounded execution budget for both cluster runs. A driver-local bound (this is
/// not the supervisor); it is deliberately generous but finite, matching the
/// shape the supervisor's callers supply from graph authority.
fn budget() -> AtomExecutionBudget {
    AtomExecutionBudget {
        max_atoms: 16,
        max_bonds: 16,
        max_steps: 16,
        max_total_energy: 10_000,
    }
}

/// Drive the whole Rung 1 chain over an already-booted supervisor and return the
/// measured deposit outcome plus the executed effect receipt. Touches the store
/// through `&self` reads only; the effect is executed on a separate capability
/// host and reinjected as effect-health evidence.
fn fire(
    supervisor: &mut Supervisor,
) -> Result<(PhysicsDepositOutcome, EffectExecutionReceipt), Box<dyn Error>> {
    // (1)-(3): run the sensor, collect its threshold crossings as physics events,
    // route them through the DepositBond onto the dormant construct's trigger,
    // and (4) run the construct so it self-wakes. run_physics_deposit_phase takes
    // &self and NEVER mutates the store or snapshot.
    let outcome = supervisor.run_physics_deposit_phase(
        sensor_cluster(),
        &deposit_bindings(),
        construct_cluster(),
        &effect_bindings(),
        budget(),
    )?;

    // The candidate must exist: the emitter fired, so exactly one notify
    // EffectIntent is proposed. (It is a CANDIDATE — not committed anywhere.)
    let candidate = outcome
        .candidate_effects
        .first()
        .cloned()
        .ok_or("no CANDIDATE notify EffectIntent was surfaced — the construct did not fire")?;

    // (4)/(5): EXECUTE the candidate through the capability host. `notify` is an
    // EXTERNAL effect: EffectIntent -> authorized transport -> EffectReceipt.
    // This is NOT a 4-verb store mutation.
    let mut capability_host = CapabilityHost::default();
    capability_host.register(NOTIFY_CAPABILITY, Box::new(NotifyTransport));
    let exec_receipt = capability_host.execute_measured(outcome.observed_at_tick, &candidate)?;

    // Reinject the transport receipt as measured effect-health evidence. This is
    // in-memory supervisor evidence, NOT a store write.
    supervisor.observe_transport_receipt(
        exec_receipt.capability.clone(),
        exec_receipt.idempotency_key.clone(),
        &exec_receipt.outcome,
    );

    Ok((outcome, exec_receipt))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("HOUSE ALARM FIRE FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    // A fresh, unique SCRATCH store dir. NEVER the live store.
    let store_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_scratch_store);
    let genesis = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/genesis/minimal-genesis.json");
    fs::create_dir_all(&store_dir)?;
    println!("scratch store: {}", store_dir.display());
    println!("genesis      : {}", genesis.display());

    // Boot a supervisor over the scratch store (writes the Genesis checkpoint).
    let mut supervisor = Supervisor::boot(&store_dir, &genesis)?;
    let revision_before = supervisor.revision();
    let tick_before = supervisor.tick();
    println!(
        "\nbooted: revision {} tick {} (state {:?})",
        revision_before.0,
        tick_before.0,
        supervisor.state()
    );

    // Capture the committed store bytes BEFORE the flow (byte-identity baseline).
    let bytes_before = read_all_files(&store_dir)?;

    // --- Drive Rung 1 ------------------------------------------------------
    println!("\n-- approach: a developer crosses the armed entry sensor (SIMULATED) --");
    let (outcome, exec_receipt) = fire(&mut supervisor)?;

    // (2) The physics events: the sensor cluster's threshold crossings.
    let events = universe_physics::fired_atoms(&outcome.sensor);
    println!(
        "sensor fired atoms (physics events): {:?}",
        events.iter().map(|k| format!("{:#x}", k.0)).collect::<Vec<_>>()
    );
    assert!(
        events.contains(&CITIZEN_BODY_INTERSECTS),
        "the measured crossing atom must have fired"
    );
    assert_eq!(
        outcome.sensor.convergence,
        AtomConvergence::Quiescent,
        "sensor cluster must reach quiescence"
    );
    assert!(
        outcome.sensor.energy.conserved,
        "sensor cluster energy must be conserved"
    );

    // (3) The deposit: the bridge routed one +100 support onto alarm_trigger.
    println!(
        "\ndeposit routed by the bridge: {} request(s)",
        outcome.deposits.len()
    );
    for deposit in &outcome.deposits {
        println!(
            "  +{} energy onto atom {:#x}   [{}]",
            deposit.energy, deposit.atom.0, deposit.provenance
        );
    }
    assert_eq!(outcome.deposits.len(), 1, "exactly one deposit expected");
    assert_eq!(outcome.deposits[0].atom, ALARM_TRIGGER);
    assert_eq!(outcome.deposits[0].energy, ONE_SUPPORT);

    // (4) The construct self-woke: the trigger crossed threshold and fired,
    // conducting to the emitter.
    println!(
        "\nconstruct fired atoms (the alarm woke): {:?}",
        outcome
            .fired_construct_atoms
            .iter()
            .map(|k| format!("{:#x}", k.0))
            .collect::<Vec<_>>()
    );
    assert!(
        outcome.fired_construct_atoms.contains(&ALARM_TRIGGER),
        "the deposit must cross alarm_trigger's threshold and FIRE it"
    );
    assert!(
        outcome.fired_construct_atoms.contains(&NOTIFY_EMITTER),
        "the fired trigger must conduct into notify_emitter"
    );
    assert_eq!(outcome.construct.convergence, AtomConvergence::Quiescent);
    assert!(
        outcome.construct.energy.conserved,
        "construct cluster energy must be conserved"
    );

    // (5) The CANDIDATE notify EffectIntent — a proposal, never committed here.
    assert_eq!(
        outcome.candidate_effects.len(),
        1,
        "exactly one CANDIDATE notify EffectIntent expected"
    );
    println!(
        "\nCANDIDATE effect surfaced (proposal, not committed): capability={} key={}",
        outcome.candidate_effects[0].capability, outcome.candidate_effects[0].idempotency_key
    );

    // The EXECUTED external effect: EffectIntent -> transport -> EffectReceipt.
    println!("\nexecuted the candidate through the capability transport:");
    println!("  transport_attempted: {}", exec_receipt.transport_attempted);
    println!("  outcome            : {:?}", exec_receipt.outcome);
    assert!(
        exec_receipt.transport_attempted,
        "the notify transport must actually run"
    );
    assert!(
        matches!(
            exec_receipt.outcome,
            universe_capabilities::EffectReceipt::TransportSucceeded { .. }
        ),
        "the notify transport must succeed and yield a receipt"
    );

    // Reinjected effect-health evidence is now measured from the real receipt.
    let effect_health = supervisor.status().health.effect;
    println!("\nreinjected as effect health: {:?}", effect_health);
    assert_eq!(
        effect_health,
        Epistemic::Measured(HealthLevel::Nominal),
        "one observed successful transport => effect health Measured(Nominal)"
    );

    // --- Prove the store was NEVER mutated ---------------------------------
    let bytes_after = read_all_files(&store_dir)?;
    let store_unchanged = bytes_before == bytes_after;
    let readback = supervisor.independent_readback()?;
    println!("\n-- store integrity (a PhysicsEvent never mutates the store) --");
    println!(
        "revision: {} -> {}   tick: {} -> {}",
        revision_before.0,
        readback.revision.0,
        tick_before.0,
        readback.tick.0
    );
    println!("committed store byte-identical before/after: {store_unchanged}");
    assert!(
        store_unchanged,
        "committed store bytes changed — a PhysicsEvent / external effect must NOT mutate the store"
    );
    assert_eq!(
        readback.revision, revision_before,
        "committed revision must be unchanged"
    );

    println!(
        "\nRESULT: Rung 1 proven end-to-end (mechanism level) — approach -> the construct wakes -> it notifies."
    );
    println!("  chain: intersection (SIMULATED PhysicsEvent) -> +deposit -> threshold cross -> fire");
    println!("         -> CANDIDATE notify EffectIntent -> EffectReceipt (external effect, receipted)");
    println!("  the committed store is byte-identical: nothing was mutated by the event or the effect.");
    println!("  HONEST BOUNDARY: the crossing is simulated (seeded), not a real Rapier collider; the");
    println!("                   canonical fixture is mirrored, not injected into a store.");

    // Best-effort scratch cleanup (only when we created a default temp dir).
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
    env::temp_dir().join(format!("house-alarm-fire-{}-{nanos}", std::process::id()))
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

    /// The same Rung-1 chain, driven over a fresh tempdir scratch store, asserting
    /// every mechanism-level invariant and the store's byte-identity.
    #[test]
    fn approach_wakes_the_construct_and_notifies_without_mutating_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        let genesis = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        fs::create_dir_all(&store_dir).unwrap();

        let mut supervisor = Supervisor::boot(&store_dir, &genesis).unwrap();
        let revision_before = supervisor.revision();
        let bytes_before = read_all_files(&store_dir).unwrap();

        let (outcome, exec_receipt) = fire(&mut supervisor).unwrap();

        // Physics event: the measured crossing fired.
        let events = universe_physics::fired_atoms(&outcome.sensor);
        assert!(events.contains(&CITIZEN_BODY_INTERSECTS));

        // Deposit routed onto the dormant trigger.
        assert_eq!(outcome.deposits.len(), 1);
        assert_eq!(outcome.deposits[0].atom, ALARM_TRIGGER);
        assert_eq!(outcome.deposits[0].energy, ONE_SUPPORT);

        // Threshold cross -> fire -> conduct to emitter.
        assert!(outcome.fired_construct_atoms.contains(&ALARM_TRIGGER));
        assert!(outcome.fired_construct_atoms.contains(&NOTIFY_EMITTER));
        assert_eq!(outcome.construct.convergence, AtomConvergence::Quiescent);
        assert!(outcome.construct.energy.conserved);

        // Exactly one CANDIDATE notify effect, executed to a real receipt.
        assert_eq!(outcome.candidate_effects.len(), 1);
        assert_eq!(outcome.candidate_effects[0].capability, NOTIFY_CAPABILITY);
        assert!(exec_receipt.transport_attempted);
        assert!(matches!(
            exec_receipt.outcome,
            universe_capabilities::EffectReceipt::TransportSucceeded { .. }
        ));

        // Reinjected effect health is measured from the real transport.
        assert_eq!(
            supervisor.status().health.effect,
            Epistemic::Measured(HealthLevel::Nominal)
        );

        // The committed store is byte-identical: nothing was mutated.
        let bytes_after = read_all_files(&store_dir).unwrap();
        assert_eq!(
            bytes_before, bytes_after,
            "a PhysicsEvent / external effect must not mutate the committed store"
        );
        assert_eq!(supervisor.independent_readback().unwrap().revision, revision_before);
    }
}
