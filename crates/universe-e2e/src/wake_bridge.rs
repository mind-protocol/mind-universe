//! The load-bearing bridge CLAUDE.md names: **physics fire -> wake queue**.
//!
//! "A construct wires itself into the field and waits to be fired. […] The
//! physics step fills a wake-queue; the serial loop drains it, so dormant
//! constructs cost nothing." The physics solver already produces the signal for
//! free — `AtomStep.fired` lists every atom that crossed its threshold this
//! tick. This module is the PRODUCER side of the bridge: it turns those firings
//! into `TriggerEvent`s of the new kind `AtomFired`, ready to be ingested by the
//! `TriggerScheduler` (the wake queue).
//!
//! Scope (honest boundary): this proves fire -> wake-events deterministically.
//! Feeding the events into `TriggerScheduler::ingest_event` so they actually
//! ENQUEUE requires subscribed constructs with compiled CodeDefinitions, and
//! wiring the call into the live supervisor tick — that is the next sub-slice
//! and stays design-gated (a construct must declare its subscription first).

use std::collections::BTreeMap;

use universe_core::{Epistemic, Revision};
use universe_ir::{TriggerEvent, TriggerEventKind, TriggerEventPayload};
use universe_physics::AtomRun;

/// Map an executed `AtomRun` to one `AtomFired` wake event per atom that fired,
/// in solver order. The event's subject is the fired atom; `occurred_at` /
/// `observed_at` are the step tick (the solver reported the firing there, for
/// free — never a poll). Measured evidence: a firing is an observed physical
/// fact, not an inferred one.
pub fn atom_run_to_wake_events(run: &AtomRun, source_revision: Revision) -> Vec<TriggerEvent> {
    let mut events = Vec::new();
    for step in &run.steps {
        for atom in &step.fired {
            events.push(TriggerEvent {
                event_id: format!("atom-fired:{}:{:#x}", step.tick.0, atom.0),
                kind: TriggerEventKind::AtomFired,
                source_revision,
                occurred_at: step.tick,
                observed_at: step.tick,
                evidence: Epistemic::Measured(TriggerEventPayload {
                    subject: Some(*atom),
                    fields: BTreeMap::new(),
                    receipt_hash: None,
                }),
                causal_ancestry: vec![],
            });
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::{EntityKey, RelationKey, Tick};
    use universe_physics::{AtomBond, AtomDynamics, AtomSpec, BondPolarity};

    fn support_atom(key: u128) -> AtomSpec {
        AtomSpec {
            key: EntityKey(key),
            threshold: 100,
            seed_energy: 100,
            required_supports: vec![],
            inhibition_threshold: None,
        }
    }

    /// A tiny construct circuit: two support atoms seed and fire, conducting into
    /// an AND-gate atom (required_supports = both bonds) that then fires. The
    /// bridge must emit exactly one `AtomFired` wake event per fired atom.
    #[test]
    fn physics_fire_produces_one_wake_event_per_fired_atom() {
        let gate = AtomSpec {
            key: EntityKey(3),
            threshold: 200, // both support bonds carry 100 -> 200 total
            seed_energy: 0,
            required_supports: vec![RelationKey(1), RelationKey(2)],
            inhibition_threshold: None,
        };
        let mut dynamics = AtomDynamics::new(
            vec![support_atom(1), support_atom(2), gate],
            vec![
                AtomBond { key: RelationKey(1), source: EntityKey(1), target: EntityKey(3), polarity: BondPolarity::Support, energy: 100 },
                AtomBond { key: RelationKey(2), source: EntityKey(2), target: EntityKey(3), polarity: BondPolarity::Support, energy: 100 },
            ],
        )
        .unwrap();
        let run = dynamics.run_until_quiescent(4).unwrap();
        assert!(run.quiescent && run.energy_conserved);
        assert!(dynamics.fired(EntityKey(3)), "the AND gate fires");

        let events = atom_run_to_wake_events(&run, Revision(1));

        // One wake event per firing, all of kind AtomFired.
        let total_fired: usize = run.steps.iter().map(|s| s.fired.len()).sum();
        assert_eq!(events.len(), total_fired);
        assert!(events.iter().all(|e| e.kind == TriggerEventKind::AtomFired));

        // The three atoms that fired are exactly the wake-event subjects.
        let mut subjects: Vec<u128> = events
            .iter()
            .filter_map(|e| match &e.evidence {
                Epistemic::Measured(p) => p.subject.map(|k| k.0),
                _ => None,
            })
            .collect();
        subjects.sort();
        assert_eq!(subjects, vec![1, 2, 3]);

        // occurred_at is the solver's step tick, not an invented time.
        assert!(events.iter().all(|e| e.occurred_at == e.observed_at));
        assert!(events.iter().any(|e| e.occurred_at != Tick(0)));
    }

    /// The full bridge, end to end: an atom fires in the physics step, the bridge
    /// turns that firing into an `AtomFired` wake event, and a construct SUBSCRIBED
    /// to `AtomFired` sees the event ENQUEUE one execution request in the
    /// `TriggerScheduler` (the wake queue). This is "the physics step fills a
    /// wake-queue; the serial loop drains it" — proven across the real scheduler,
    /// not a stub. (Wiring the ingest call into the live supervisor tick remains
    /// the next step; here the scheduler is driven directly.)
    #[test]
    fn wake_events_enqueue_on_a_subscribed_construct() {
        use universe_core::Tick;
        use universe_ir::{TriggerEventKind, TriggerSubscription};
        use universe_supervisor::{TriggerLifecycleState, TriggerScheduler, TriggerSchedulerLimits};

        // 1. Physics: run the little circuit; the AND gate (atom 3) fires.
        let gate = AtomSpec {
            key: EntityKey(3),
            threshold: 200,
            seed_energy: 0,
            required_supports: vec![RelationKey(1), RelationKey(2)],
            inhibition_threshold: None,
        };
        let mut dynamics = AtomDynamics::new(
            vec![support_atom(1), support_atom(2), gate],
            vec![
                AtomBond { key: RelationKey(1), source: EntityKey(1), target: EntityKey(3), polarity: BondPolarity::Support, energy: 100 },
                AtomBond { key: RelationKey(2), source: EntityKey(2), target: EntityKey(3), polarity: BondPolarity::Support, energy: 100 },
            ],
        )
        .unwrap();
        let run = dynamics.run_until_quiescent(4).unwrap();

        // 2. Bridge: the gate's firing becomes an AtomFired wake event.
        let events = atom_run_to_wake_events(&run, Revision(1));
        let gate_event = events
            .iter()
            .find(|e| matches!(&e.evidence, Epistemic::Measured(p) if p.subject == Some(EntityKey(3))))
            .expect("the fired gate produced a wake event")
            .clone();

        // 3. A construct subscribed to AtomFired (valid fixture, kinds overridden).
        let mut subscription: TriggerSubscription =
            serde_json::from_str(include_str!("../../../fixtures/graph-ir/trigger-subscription.json"))
                .unwrap();
        subscription.event_kinds = vec![TriggerEventKind::AtomFired];

        let limits = TriggerSchedulerLimits {
            max_backlog: 64,
            max_requests_per_tick: 16,
            max_fuel_per_tick: 4096,
            max_mutations_per_tick: 64,
            max_tracked_idempotency_keys: 1024,
        };
        let mut scheduler = TriggerScheduler::new(limits).unwrap();
        assert_eq!(scheduler.backlog(), 0);

        // 4. Ingest the wake event -> it enqueues (Accepted) one execution request.
        let issued_at = Tick(gate_event.observed_at.0 + 1);
        let receipt = scheduler.ingest_event(&[subscription], &gate_event, Revision(1), issued_at);

        assert_eq!(receipt.backlog, 1);
        assert_eq!(scheduler.backlog(), 1, "the wake event enqueued one execution request");
        assert!(
            receipt.transitions.iter().any(|t| matches!(
                t.state,
                Epistemic::Measured(TriggerLifecycleState::Accepted)
            )),
            "the subscribed construct's trigger was Accepted onto the wake queue"
        );
    }

    /// A dormant circuit (nothing crosses threshold) produces zero wake events —
    /// dormant constructs cost nothing.
    #[test]
    fn no_firing_produces_no_wake_events() {
        let starved = AtomSpec {
            key: EntityKey(1),
            threshold: 100,
            seed_energy: 0, // never reaches threshold
            required_supports: vec![],
            inhibition_threshold: None,
        };
        let mut dynamics = AtomDynamics::new(vec![starved], vec![]).unwrap();
        let run = dynamics.run_until_quiescent(4).unwrap();
        assert!(!dynamics.fired(EntityKey(1)));
        assert!(atom_run_to_wake_events(&run, Revision(1)).is_empty());
    }
}
