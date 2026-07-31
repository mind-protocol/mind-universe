//! The load-bearing proof: **a construct CALLS a construct.**
//!
//! Doctrine (CLAUDE.md, "constructs that call each other"): there is no separate
//! intent object — only constructs that call each other. A *call* is physical:
//!
//! ```text
//! construct A fires  -> AtomFired (terminal for A — A's energy is untouched)
//!                    -> a DepositBond, triggered BY that fire-event,
//!                    -> deposits a fresh BOUNDED quantum onto the gate atom of
//!                       the precondition-cluster of a NAMED target construct B
//!                    -> B's gate crosses its threshold -> B fires -> B calls C ...
//! ```
//!
//! This driver builds the smallest end-to-end proof of exactly that, on the SAME
//! generic machinery `house_alarm_resolved` / `construct_loop` use:
//!
//!   * A and B are two constructs resolved FROM the authored `alarm_atom_circuit`
//!     (Lumina Prime house-alarm). B is resolved with its sensor injection
//!     stripped, so B is genuinely DORMANT — its own sensor never fires; B can
//!     only fire if something deposits onto its gate. B is re-keyed into a
//!     disjoint atom/bond namespace so A and B are two distinct constructs in one
//!     global namespace (as they would be in a live store).
//!   * The call is ONE graph-declared `ConstructCall`: when A's emitter
//!     (`notify_emitter`) fires, deposit a fresh bounded +100 onto B's gate
//!     (`alarm_trigger`), and wake B. Nothing about the wiring is native policy.
//!   * The serial wake-queue drains it: wake A -> A's wave fires A's trigger and
//!     terminal emitter -> `deliver_fire` turns A's firing into `AtomFired` events
//!     (the wake_bridge PRODUCER), resolves the call (the call_bridge CONSUMER),
//!     stages a fresh +100 onto B's gate and enqueues B -> the NEXT drain runs B,
//!     whose gate crosses threshold on that deposit and FIRES.
//!
//! What is PROVEN here:
//!   * A -> B is a real call: B fires ONLY as a consequence of A's fire. The
//!     negative control runs B woken on its own (no call): with its sensor
//!     stripped, B does NOT fire. So B's fire in the chain is caused by A, not by
//!     B merely running.
//!   * Conservation holds: A's emitter is TERMINAL (it starves nothing and
//!     conducts no energy onward); the deposit onto B is a NEW bounded quantum
//!     (weight 100), never a transfer of A's energy. Both waves conserve energy.
//!   * The committed store is byte-identical throughout — a PhysicsEvent / a
//!     staged call deposit / a candidate effect never mutates the store.
//!
//! HONEST BOUNDARIES:
//!   * A's initial crossing is SIMULATED (A's sensor is seeded by the authored
//!     `external_measured_injection`, standing in for a real Rapier collider
//!     crossing), exactly as in `house_alarm_resolved`. The A->B call itself is
//!     NOT simulated: it is the real AtomFired-producer -> call-consumer ->
//!     wake-queue path.
//!   * Because no transaction is enqueued, the committed tick does not advance —
//!     the honest consequence of "only what commits stays authoritative". The loop
//!     is still serial and still drains the queue.
//!
//! Usage: `construct_calls_construct [scratch-store-dir]`
//!   scratch-store-dir defaults to a fresh unique dir under the system temp dir.
//!   NEVER pass a live store: this boots a fresh Genesis and needs an empty dir.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use serde_json::Value;

use universe_core::{EntityKey, RelationKey, UniverseError};
use universe_e2e::call_bridge::ConstructCall;
use universe_e2e::construct_resolver::{resolve_construct, AlarmAtomCircuit, ResolvedConstruct};
use universe_e2e::wave_selector::GraphWaveSelector;
use universe_physics::{AtomBond, AtomExecutionBudget, AtomSpec, LocalAtomCluster, PhysicsEventDeposit};
use universe_supervisor::{PhaseHook, PhysicsDepositOutcome, Supervisor, TickPhase};

/// The disjoint atom/bond offset applied to construct B so A and B occupy one
/// global namespace without collision — as two distinct constructs would in a
/// live store. Comfortably above the ~5 atoms / ~4 bonds a single circuit uses.
const B_ATOM_OFFSET: u128 = 0x1000;
const B_BOND_OFFSET: u128 = 0x1000;

/// The fresh bounded quantum construct A's fire deposits onto construct B's gate.
/// Matches B's `alarm_trigger` threshold (100) so one call is one crossing.
const CALL_QUANTUM: u64 = 100;

/// A trivial no-op phase hook — the proof asserts on the waves, not on phases.
struct NoopHook;
impl PhaseHook for NoopHook {
    fn run(&mut self, _phase: TickPhase, _snapshot: &universe_store::UniverseSnapshot) -> Result<(), UniverseError> {
        Ok(())
    }
}

/// The caller-supplied (graph-authority) execution budget every wave is bounded
/// by. Mirrors `house_alarm_resolved` / `construct_loop`.
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
    let root: Value = serde_json::from_slice(&fs::read(fixture)?)?;
    let members = root.get("members").and_then(Value::as_array).ok_or("fixture has no members")?;
    let code_member = members
        .iter()
        .find(|m| m.get("id").and_then(Value::as_str) == Some("code:l2:lumina-prime:house-alarm-v0"))
        .ok_or("fixture has no code member")?;
    let circuit_value = code_member
        .get("content")
        .and_then(|c| c.get("alarm_atom_circuit"))
        .ok_or("code member has no alarm_atom_circuit")?;
    Ok(serde_json::from_value(circuit_value.clone())?)
}

/// Re-key a resolved construct into a disjoint namespace by offsetting every atom
/// [`EntityKey`] and bond [`RelationKey`]. Pure runtime materialization: it moves
/// the whole construct (both clusters, deposits, effect bindings, and both name
/// maps) consistently, so it remains internally self-consistent while no longer
/// colliding with the un-offset construct.
fn rekey(resolved: &ResolvedConstruct, atom_offset: u128, bond_offset: u128) -> ResolvedConstruct {
    let atom = |k: EntityKey| EntityKey(k.0 + atom_offset);
    let bond = |k: RelationKey| RelationKey(k.0 + bond_offset);
    let rekey_cluster = |cluster: &LocalAtomCluster| LocalAtomCluster {
        atoms: cluster
            .atoms
            .iter()
            .map(|spec| AtomSpec {
                key: atom(spec.key),
                threshold: spec.threshold,
                seed_energy: spec.seed_energy,
                required_supports: spec.required_supports.iter().copied().map(bond).collect(),
                inhibition_threshold: spec.inhibition_threshold,
            })
            .collect(),
        bonds: cluster
            .bonds
            .iter()
            .map(|b| AtomBond {
                key: bond(b.key),
                source: atom(b.source),
                target: atom(b.target),
                polarity: b.polarity,
                energy: b.energy,
            })
            .collect(),
        injections: cluster
            .injections
            .iter()
            .cloned()
            .map(|mut inj| {
                inj.atom = atom(inj.atom);
                inj
            })
            .collect(),
    };
    ResolvedConstruct {
        sensor_cluster: rekey_cluster(&resolved.sensor_cluster),
        deposit_bindings: resolved
            .deposit_bindings
            .iter()
            .map(|d| PhysicsEventDeposit {
                trigger: atom(d.trigger),
                target: atom(d.target),
                weight: d.weight,
            })
            .collect(),
        construct_cluster: rekey_cluster(&resolved.construct_cluster),
        effect_bindings: resolved
            .effect_bindings
            .iter()
            .map(|binding| {
                let mut b = binding.clone();
                b.atom = atom(binding.atom);
                b
            })
            .collect(),
        atom_keys: resolved.atom_keys.iter().map(|(n, k)| (n.clone(), atom(*k))).collect(),
        bond_keys: resolved.bond_keys.iter().map(|(n, k)| (n.clone(), bond(*k))).collect(),
    }
}

/// The measured facts of one construct's wave.
#[derive(Clone, Debug)]
struct WaveFacts {
    fired_trigger: bool,
    fired_emitter: bool,
    candidates: usize,
    energy_conserved: bool,
    terminal_starved_empty: bool,
    injected_energy: u64,
}

fn wave_facts(outcome: &PhysicsDepositOutcome, trigger: EntityKey, emitter: EntityKey) -> WaveFacts {
    WaveFacts {
        fired_trigger: outcome.fired_construct_atoms.contains(&trigger),
        fired_emitter: outcome.fired_construct_atoms.contains(&emitter),
        candidates: outcome.candidate_effects.len(),
        energy_conserved: outcome.construct.energy.conserved,
        terminal_starved_empty: outcome.construct.terminal_starved.is_empty(),
        injected_energy: outcome.construct.run.injected_energy,
    }
}

/// The measured outcome of the whole proof.
struct ProofOutput {
    a_id: EntityKey,
    b_id: EntityKey,
    /// A's emitter atom (the call's source) and B's gate atom (the call's target).
    call_source: EntityKey,
    call_target: EntityKey,
    /// A's wave, driven by waking A (its simulated crossing).
    a_wave: WaveFacts,
    /// The delivered call A->B: (callee, deposit target atom, deposit energy).
    call_delivered: Option<(EntityKey, EntityKey, u64)>,
    /// Staged deposits on B after the call was delivered, before B is drained.
    b_pending_after_call: usize,
    /// B's wave, driven purely by draining the queue after the call (NOT re-woken).
    b_wave: WaveFacts,
    /// The negative control: B woken on its own, with NO call. Should not fire.
    b_control_wave: WaveFacts,
    /// The committed store was byte-identical after every step.
    store_byte_identical: bool,
}

/// Boot a scratch supervisor, resolve A and B, wire the A->B call, and drive the
/// serial wake-queue so A's fire CALLS B. Asserts live in the caller so the bin
/// and the `#[test]` share one execution path.
fn drive(store_dir: &Path, genesis: &Path, fixture: &Path) -> Result<ProofOutput, Box<dyn Error>> {
    let circuit = load_circuit(fixture)?;

    // Construct A: the authored circuit, sensor seeded -> A fires on wake.
    let a = resolve_construct(&circuit).map_err(|e| format!("resolve A failed: {e:?}"))?;

    // Construct B: the SAME circuit with its sensor injection STRIPPED, so B is
    // genuinely dormant (its sensor never fires); re-keyed into a disjoint
    // namespace so A and B are two distinct constructs.
    let mut dormant_circuit = circuit.clone();
    dormant_circuit.external_measured_injections.clear();
    let b_base = resolve_construct(&dormant_circuit).map_err(|e| format!("resolve B failed: {e:?}"))?;
    let b = rekey(&b_base, B_ATOM_OFFSET, B_BOND_OFFSET);

    let a_trigger = *a.atom_keys.get("alarm_trigger").ok_or("A has no alarm_trigger")?;
    let a_emitter = *a.atom_keys.get("notify_emitter").ok_or("A has no notify_emitter")?;
    let b_trigger = *b.atom_keys.get("alarm_trigger").ok_or("B has no alarm_trigger")?;
    let b_emitter = *b.atom_keys.get("notify_emitter").ok_or("B has no notify_emitter")?;

    // Stable wake-queue ids for the two constructs (distinct code-node keys).
    let a_id = EntityKey(0xA);
    let b_id = EntityKey(0xB);

    // The ONE graph-declared call: A's emitter fire -> +CALL_QUANTUM onto B's gate,
    // waking B. `trigger` = A's emitter atom, `target` = B's gate atom.
    let call = ConstructCall {
        deposit: PhysicsEventDeposit {
            trigger: a_emitter,
            target: b_trigger,
            weight: CALL_QUANTUM,
        },
        target_construct: b_id,
    };

    let mut supervisor = Supervisor::boot(store_dir, genesis)?;
    let revision = supervisor.revision();
    let bytes_before = read_all_files(store_dir)?;
    let mut hook = NoopHook;
    let mut store_byte_identical = true;
    let check_store = |dir: &Path, ok: &mut bool| -> Result<(), Box<dyn Error>> {
        if read_all_files(dir)? != bytes_before {
            *ok = false;
        }
        Ok(())
    };

    // --- The chain: wake A, then drain, delivering each fired construct's calls.
    let mut selector = GraphWaveSelector::new(budget());
    selector.register(a_id, a.clone());
    selector.register(b_id, b.clone());
    selector.wire_call(call);

    // Turn 1: wake A (its simulated crossing) and drain -> A's wave.
    selector.wake(a_id);
    let out1 = supervisor.advance_driving_physics_wave(&mut hook, &mut selector)?;
    let a_wave_outcome = out1.physics_wave.ok_or("turn 1 ran no wave — A did not drain")?;
    let a_wave = wave_facts(&a_wave_outcome, a_trigger, a_emitter);
    check_store(store_dir, &mut store_byte_identical)?;

    // Deliver A's fire: its AtomFired events resolve the call, staging a fresh
    // bounded deposit onto B's gate and waking B. This is the physical "A calls B".
    let delivered = selector
        .deliver_fire(&a_wave_outcome.construct.run, revision)
        .map_err(|e| format!("deliver_fire(A) failed: {e:?}"))?;
    let call_delivered = delivered
        .first()
        .map(|d| (d.target_construct, d.deposit.atom, d.deposit.energy));
    let b_pending_after_call = selector.pending_deposits(b_id);

    // Turn 2: drain again WITHOUT re-waking — B is queued ONLY because A called it.
    let out2 = supervisor.advance_driving_physics_wave(&mut hook, &mut selector)?;
    let b_wave_outcome = out2.physics_wave.ok_or("turn 2 ran no wave — the call did not enqueue B")?;
    let b_wave = wave_facts(&b_wave_outcome, b_trigger, b_emitter);
    check_store(store_dir, &mut store_byte_identical)?;
    // Also deliver B's fire (B calls no one) to prove the chain terminates cleanly.
    let b_onward = selector
        .deliver_fire(&b_wave_outcome.construct.run, revision)
        .map_err(|e| format!("deliver_fire(B) failed: {e:?}"))?;
    let _ = b_onward; // expected empty: no call is keyed on B's emitter.

    // --- Negative control: B woken ON ITS OWN, no call. A fresh selector with the
    // SAME wiring but A never fires; we wake B directly. With its sensor stripped
    // and no staged deposit, B must NOT fire — proving B's fire above was caused
    // by A's call, not by B merely running.
    let mut control = GraphWaveSelector::new(budget());
    control.register(b_id, b.clone());
    control.wake(b_id);
    let control_out = supervisor.advance_driving_physics_wave(&mut hook, &mut control)?;
    let control_outcome = control_out.physics_wave.ok_or("control ran no wave")?;
    let b_control_wave = wave_facts(&control_outcome, b_trigger, b_emitter);
    check_store(store_dir, &mut store_byte_identical)?;

    Ok(ProofOutput {
        a_id,
        b_id,
        call_source: a_emitter,
        call_target: b_trigger,
        a_wave,
        call_delivered,
        b_pending_after_call,
        b_wave,
        b_control_wave,
        store_byte_identical,
    })
}

/// Assert the proof's measured facts. Shared by the bin and the `#[test]`.
fn assert_proof(out: &ProofOutput) -> Result<(), String> {
    // A fired end-to-end (its simulated crossing), terminal emitter and one candidate.
    if !out.a_wave.fired_trigger {
        return Err("A's trigger did not fire".into());
    }
    if !out.a_wave.fired_emitter {
        return Err("A's terminal emitter did not fire".into());
    }
    if !out.a_wave.terminal_starved_empty {
        return Err("A's emitter starved — it must be terminal and conduct nothing onward".into());
    }
    if !out.a_wave.energy_conserved {
        return Err("A's wave did not conserve energy".into());
    }

    // The call was delivered: B named, a fresh bounded deposit onto B's gate.
    match out.call_delivered {
        Some((callee, target_atom, energy)) => {
            if callee != out.b_id {
                return Err("delivered call named the wrong callee".into());
            }
            if target_atom != out.call_target {
                return Err("delivered deposit did not land on B's gate atom".into());
            }
            if energy != CALL_QUANTUM {
                return Err(format!(
                    "delivered deposit was {energy}, expected the fresh bounded quantum {CALL_QUANTUM}"
                ));
            }
        }
        None => return Err("A's fire delivered NO call — A did not call B".into()),
    }
    if out.b_pending_after_call != 1 {
        return Err(format!(
            "expected exactly 1 staged deposit on B after the call, got {}",
            out.b_pending_after_call
        ));
    }

    // B fired — as a real consequence of A's call (drained, not re-woken).
    if !out.b_wave.fired_trigger {
        return Err("B's gate did not cross its threshold on A's call — B did not fire".into());
    }
    if !out.b_wave.fired_emitter {
        return Err("B's terminal emitter did not fire".into());
    }
    if out.b_wave.candidates != 1 {
        return Err(format!(
            "B surfaced {} candidates, expected exactly 1",
            out.b_wave.candidates
        ));
    }
    if out.b_wave.injected_energy != CALL_QUANTUM {
        return Err(format!(
            "B's wave injected {} energy, expected the fresh call quantum {CALL_QUANTUM}",
            out.b_wave.injected_energy
        ));
    }
    if !out.b_wave.energy_conserved {
        return Err("B's wave did not conserve energy".into());
    }

    // Negative control: B on its own does NOT fire -> B's fire above was caused by A.
    if out.b_control_wave.fired_trigger || out.b_control_wave.fired_emitter {
        return Err("CONTROL: B fired on its own — B's fire is not a consequence of A".into());
    }
    if out.b_control_wave.candidates != 0 {
        return Err("CONTROL: dormant B surfaced a candidate".into());
    }

    // The store was never mutated across the whole proof.
    if !out.store_byte_identical {
        return Err("committed store changed — a call/PhysicsEvent must not mutate it".into());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("CONSTRUCT-CALLS-CONSTRUCT FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir_arg = env::args_os().nth(1).map(PathBuf::from);
    let store_dir = store_dir_arg.clone().unwrap_or_else(default_scratch_store);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let genesis = repo.join("fixtures/genesis/minimal-genesis.json");
    let fixture = repo.join("fixtures/ontology/lumina-prime-house-alarm-v0.json");
    fs::create_dir_all(&store_dir)?;
    println!("scratch store: {}", store_dir.display());
    println!("genesis      : {}", genesis.display());
    println!("construct    : {}", fixture.display());

    let out = drive(&store_dir, &genesis, &fixture)?;

    println!("\n-- two constructs, wired A -> B (call: A.notify_emitter fire -> +{CALL_QUANTUM} onto B.alarm_trigger) --");
    println!("A (caller) id={:#x}  call source atom (A.notify_emitter)={:#x}", out.a_id.0, out.call_source.0);
    println!("B (callee) id={:#x}  call target atom (B.alarm_trigger)  ={:#x}", out.b_id.0, out.call_target.0);

    println!("\n-- turn 1: wake A (simulated crossing), drain -> A's wave --");
    println!(
        "  A fired: trigger={} emitter={}  terminal_starved_empty={} energy_conserved={}",
        out.a_wave.fired_trigger, out.a_wave.fired_emitter, out.a_wave.terminal_starved_empty, out.a_wave.energy_conserved
    );

    println!("\n-- A CALLS B: A's fire -> AtomFired -> DepositBond -> B's gate --");
    match out.call_delivered {
        Some((callee, target, energy)) => println!(
            "  delivered call: wake construct {:#x}, deposit +{energy} onto atom {:#x} (a FRESH bounded quantum, not a transfer of A's energy)",
            callee.0, target.0
        ),
        None => println!("  NO call delivered"),
    }
    println!("  staged deposits on B after the call: {}", out.b_pending_after_call);

    println!("\n-- turn 2: drain again (B queued ONLY by A's call, NOT re-woken) -> B's wave --");
    println!(
        "  B fired: trigger={} emitter={}  candidates={} injected_energy={} energy_conserved={}",
        out.b_wave.fired_trigger, out.b_wave.fired_emitter, out.b_wave.candidates, out.b_wave.injected_energy, out.b_wave.energy_conserved
    );

    println!("\n-- negative control: B woken ON ITS OWN, no call --");
    println!(
        "  B fired: trigger={} emitter={}  candidates={}  (dormant sensor + no call => must NOT fire)",
        out.b_control_wave.fired_trigger, out.b_control_wave.fired_emitter, out.b_control_wave.candidates
    );

    println!("\ncommitted store byte-identical across the whole proof: {}", out.store_byte_identical);

    assert_proof(&out).map_err(|e| format!("proof failed: {e}"))?;

    println!("\n=================================================================================");
    println!("A CONSTRUCT CALLS A CONSTRUCT — proven end-to-end.");
    println!("=================================================================================");
    println!("  (a) A fired (its terminal emitter), conducting NO energy onward (conservation);");
    println!("  (b) A's fire produced AtomFired events (wake_bridge producer), a graph-declared");
    println!("      DepositBond resolved them (call_bridge consumer) into a FRESH bounded +{CALL_QUANTUM}");
    println!("      onto B's gate, and enqueued B on the serial wake-queue;");
    println!("  (c) draining the queue ran B, whose gate crossed threshold on that deposit and FIRED;");
    println!("  (d) NEGATIVE CONTROL: B woken on its own did NOT fire => B's fire is a real");
    println!("      consequence of A's call, not of B merely running;");
    println!("  (e) both waves conserved energy and the committed store is byte-identical.");
    println!("  HONEST BOUNDARY: A's initial crossing is simulated (seeded sensor); the A->B call");
    println!("  is NOT simulated. Still-open gap: wiring a real Rapier crossing into the wake-queue.");

    if store_dir_arg.is_none() {
        let _ = fs::remove_dir_all(&store_dir);
    }
    Ok(())
}

fn default_scratch_store() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    env::temp_dir().join(format!("construct-calls-construct-{}-{nanos}", std::process::id()))
}

/// Read every file under `dir` into a path-keyed map of raw bytes, for a literal
/// byte-identity comparison of the committed store across the proof.
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
                out.insert(path.clone(), fs::read(&path)?);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full "construct calls construct" proof over a fresh tempdir scratch
    /// store: A fires, calls B via the AtomFired->DepositBond bridge across the
    /// serial wake-queue, B fires as a consequence, the negative control shows B
    /// does not fire on its own, energy is conserved and the store is untouched.
    #[test]
    fn a_construct_calls_a_construct_end_to_end() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let genesis = repo.join("fixtures/genesis/minimal-genesis.json");
        let fixture = repo.join("fixtures/ontology/lumina-prime-house-alarm-v0.json");
        fs::create_dir_all(&store_dir).unwrap();

        let out = drive(&store_dir, &genesis, &fixture).unwrap();
        assert_proof(&out).unwrap();

        // Independent restatement of the load-bearing facts.
        assert!(out.a_wave.fired_emitter, "A's terminal emitter fired");
        assert_eq!(out.call_delivered.map(|(_, _, e)| e), Some(CALL_QUANTUM), "fresh bounded call quantum");
        assert!(out.b_wave.fired_trigger && out.b_wave.fired_emitter, "B fired from the call");
        assert!(
            !out.b_control_wave.fired_trigger && !out.b_control_wave.fired_emitter,
            "B does not fire on its own — its fire is caused by A"
        );
        assert!(out.store_byte_identical, "store byte-identical across the proof");
    }
}
