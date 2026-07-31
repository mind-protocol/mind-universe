//! The ingest half of the load-bearing bridge: **AtomFired -> +energy deposit
//! onto a NAMED target construct's precondition gate**. This is where "a
//! construct CALLS a construct" becomes physics.
//!
//! `wake_bridge` is the PRODUCER: it turns each Atom that crossed its threshold
//! this wave (`AtomStep.fired`) into an `AtomFired` `TriggerEvent`. This module
//! is the CONSUMER: given the AtomFired events of a fired construct A and a set
//! of graph-declared call wirings, it resolves a fresh BOUNDED deposit onto the
//! gate atom of a named callee B — the DepositBond fire — and reports which
//! construct to wake.
//!
//! Conservation is exact and doctrinal: the emitter of A stays TERMINAL. It is
//! A's *fire-event* that triggers a *new, bounded* quantum (`ConstructCall.
//! deposit.weight`) onto B — never a transfer of A's own energy to B. This is
//! the same "emitter terminal (conservation)" rule that lets `house_alarm`'s
//! `notify_emitter` conduct nothing onward while still surfacing an effect.
//!
//! It holds no policy of its own. WHICH atom's fire calls WHICH construct's gate,
//! and with what quantum, is entirely the graph-declared `ConstructCall` wiring;
//! the resolution reuses the native floor's [`resolve_physics_event_deposits`]
//! (whose `trigger` is documented as "either a sensor collider entity or an Atom
//! threshold crossing — the native floor does not distinguish them"). A firing is
//! an atom threshold crossing, so the SAME primitive that lands a sensor deposit
//! lands a construct-call deposit. No new native mechanism is minted here.

use std::collections::BTreeSet;

use universe_core::{Epistemic, EntityKey};
use universe_ir::{TriggerEvent, TriggerEventKind};
use universe_physics::{resolve_physics_event_deposits, AtomInjectionRequest, PhysicsEventDeposit};

use crate::E2eError;

/// One graph-declared construct -> construct call wiring.
///
/// When the source atom — an emitter of the caller A, named by `deposit.trigger`
/// — fires, a fresh bounded quantum `deposit.weight` is deposited onto the gate
/// atom of the callee B, named by `deposit.target`. `target_construct` is the
/// callee's stable wake-queue id (the construct that owns `deposit.target`), so
/// the drained serial loop knows which dormant construct this call wakes.
///
/// The wiring is DATA, never native policy: the source, the target gate, the
/// quantum, and the callee id all come from the graph. A different threshold,
/// target, or quantum is a different `ConstructCall`, not a code change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstructCall {
    /// The bounded event -> +energy edge. `trigger` is the caller's fired emitter
    /// atom; `target` is the callee's gate atom; `weight` is the fresh bounded
    /// quantum deposited on the call (never drawn from the caller's energy).
    pub deposit: PhysicsEventDeposit,
    /// The callee's stable wake-queue id — the construct that owns `deposit.target`.
    pub target_construct: EntityKey,
}

/// One resolved call: the callee to wake and the fresh bounded deposit to land on
/// its gate atom before its wave runs. A proposal for the wake-queue, not a
/// mutation: nothing is committed and the caller's energy is untouched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveredCall {
    /// The callee construct to enqueue on the wake-queue.
    pub target_construct: EntityKey,
    /// The fresh bounded `+weight` deposit onto the callee's gate atom.
    pub deposit: AtomInjectionRequest,
}

/// Consume a fired construct's `AtomFired` events and resolve the graph-declared
/// call wirings whose source atom fired into fresh bounded deposits onto the
/// named callees' gate atoms.
///
/// This is the ingest that closes the bridge whose producer is
/// [`crate::wake_bridge::atom_run_to_wake_events`]: the producer turns a firing
/// into an `AtomFired` event; this consumes those events and, per declared call,
/// deposits. Only events of kind [`TriggerEventKind::AtomFired`] carrying a
/// measured subject drive a call — an unmeasured or non-firing event never
/// deposits. Deterministic and bounded: output follows `calls`, one deposit per
/// call whose `deposit.trigger` is among the fired subjects, each a fixed
/// authored quantum (no wake storm, no energy drawn from the caller).
pub fn deliver_atom_fired_calls(
    calls: &[ConstructCall],
    fired_events: &[TriggerEvent],
) -> Result<Vec<DeliveredCall>, E2eError> {
    // "Consume AtomFired": the fired subjects are the measured threshold-crossing
    // atoms this wave reported. Non-AtomFired or unmeasured events never call.
    let fired_subjects: BTreeSet<EntityKey> = fired_events
        .iter()
        .filter(|event| event.kind == TriggerEventKind::AtomFired)
        .filter_map(|event| match &event.evidence {
            Epistemic::Measured(payload) => payload.subject,
            _ => None,
        })
        .collect();

    let mut delivered = Vec::new();
    for call in calls {
        // Reuse the native floor: "event `trigger` occurred + deposit `weight`
        // onto `target`" -> a runtime deposit. A firing IS an atom threshold
        // crossing, so this is exactly the sensor-deposit primitive.
        let deposits =
            resolve_physics_event_deposits(std::slice::from_ref(&call.deposit), &fired_subjects)
                .map_err(E2eError::Universe)?;
        for deposit in deposits {
            delivered.push(DeliveredCall {
                target_construct: call.target_construct,
                deposit,
            });
        }
    }
    Ok(delivered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use universe_core::{Revision, Tick};
    use universe_ir::TriggerEventPayload;

    fn atom_fired(subject: EntityKey) -> TriggerEvent {
        TriggerEvent {
            event_id: format!("atom-fired:{:#x}", subject.0),
            kind: TriggerEventKind::AtomFired,
            source_revision: Revision(1),
            occurred_at: Tick(7),
            observed_at: Tick(7),
            evidence: Epistemic::Measured(TriggerEventPayload {
                subject: Some(subject),
                fields: BTreeMap::new(),
                receipt_hash: None,
            }),
            causal_ancestry: vec![],
        }
    }

    fn call(trigger: u128, target: u128, weight: u64, callee: u128) -> ConstructCall {
        ConstructCall {
            deposit: PhysicsEventDeposit {
                trigger: EntityKey(trigger),
                target: EntityKey(target),
                weight,
            },
            target_construct: EntityKey(callee),
        }
    }

    /// A's emitter (atom 0x5) fires -> the call keyed on 0x5 deposits a fresh
    /// bounded quantum onto B's gate (atom 0x104), and names B (0xB) to wake.
    #[test]
    fn a_emitter_fire_delivers_a_bounded_deposit_onto_bs_gate() {
        let calls = vec![call(0x5, 0x104, 100, 0xB)];
        // A's construct wave fired its trigger (0x4) and its emitter (0x5).
        let events = vec![atom_fired(EntityKey(0x4)), atom_fired(EntityKey(0x5))];

        let delivered = deliver_atom_fired_calls(&calls, &events).unwrap();
        assert_eq!(delivered.len(), 1, "exactly one call fired");
        assert_eq!(delivered[0].target_construct, EntityKey(0xB), "B is the callee");
        assert_eq!(delivered[0].deposit.atom, EntityKey(0x104), "deposit lands on B's gate");
        assert_eq!(
            delivered[0].deposit.energy, 100,
            "the deposit is the authored fresh quantum, not a transfer of A's energy"
        );
    }

    /// A call whose source atom did NOT fire this wave deposits nothing — a
    /// dormant caller costs the callee nothing.
    #[test]
    fn a_call_whose_source_did_not_fire_deposits_nothing() {
        let calls = vec![call(0x5, 0x104, 100, 0xB)];
        // Only the trigger fired; the emitter (0x5) did not -> A did not complete.
        let events = vec![atom_fired(EntityKey(0x4))];
        assert!(deliver_atom_fired_calls(&calls, &events).unwrap().is_empty());
    }

    /// A non-AtomFired event never drives a call, even if its subject matches.
    #[test]
    fn non_atom_fired_events_never_call() {
        let calls = vec![call(0x5, 0x104, 100, 0xB)];
        let mut event = atom_fired(EntityKey(0x5));
        event.kind = TriggerEventKind::ScheduledTick;
        assert!(deliver_atom_fired_calls(&calls, &[event]).unwrap().is_empty());
    }
}
