//! **What the clock is, once the inference isn't.**
//!
//! CLAUDE.md's current-direction section says *"an L1 actor's turn = one local
//! inference (Ollama, ~2.1B), one call in flight"* and *"the inference is the
//! clock"*. Collectivized inference breaks that: with many providers, many
//! inferences are in flight at once, and they return in an order set by
//! provider latency — an **exogenous** property of somebody else's network.
//! If arrival order were admission order, then choosing a faster provider
//! would silently be a scheduling decision, and a vendor would be scheduling
//! the city.
//!
//! # The answer
//!
//! **The tick is the clock; the inference becomes a receipted external
//! effect.** This is not a new mechanism — it is the one CLAUDE.md already
//! lists in the irreducible native floor (*"the clock — tick advance +
//! scheduler bounding"*) plus the external-effect chain it already mandates
//! (`EffectIntent → authorized transport → EffectReceipt → reinjection`). A
//! call to an arbitrary provider over the network always belonged on that
//! path; "one call in flight" was hiding it by making the effect synchronous.
//!
//! # What is serial and what is parallel
//!
//! ```text
//! PARALLEL   in flight     N inferences across N providers, unordered.
//!                          Safe because an inference reads nothing and writes
//!                          nothing: CLAUDE.md already guarantees "the
//!                          inference never touches the store". Two pure
//!                          functions of a frozen observation cannot race.
//!
//! SERIAL     admission     one at a time, in ENDOGENOUS order (`wake_seq`,
//!                          the order the physics wake-queue produced) — never
//!                          in arrival order. This is where the world disposes.
//!
//! SERIAL     commit        unchanged: one transaction at a time, one resident
//!                          process, monotonic revision. No locks, no CAS.
//! ```
//!
//! # Head-of-line blocking is the point, not a bug
//!
//! [`AdmissionGate::drain`] stops at the first slot whose inference has not
//! landed. A later turn is **not** admitted ahead of an earlier one just
//! because a faster provider answered it. That costs throughput, and buys the
//! only thing that matters: the city's order is its own.
//!
//! It cannot wedge, because every turn carries a `deadline_tick`. When the
//! clock passes it, the slot is admitted as
//! [`TurnDisposition::Unknown`] — a first-class measured state meaning *the
//! deadline passed and nothing landed*, distinct from `measurement_failed`
//! (we tried and it failed) and from `not_configured` (we never could try).
//!
//! # Why parallel dispatch is safe at all
//!
//! Because *"the model proposes; the world disposes"* was already true. An
//! observation is frozen at revision R when it is dispatched; by the time the
//! answer lands the store may be at R+k. Admission re-checks the proposal's
//! precondition against the world **now**, and a proposal whose ground has
//! moved is rejected with a receipt
//! ([`RejectionKind::StaleObservation`]) — never silently applied. The world
//! having the final say is exactly what lets many models speak at once.

use serde::{Deserialize, Serialize};
use universe_core::Tick;

use crate::contract::{InferenceAttribution, InferenceObservation, InferenceOutcome};
use crate::routing::AdmissionSpec;

/// One L1 actor's turn, as the gate tracks it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub turn_id: String,
    pub actor_id: String,
    /// The ENDOGENOUS order of this turn: the position the physics wake-queue
    /// gave it. Admission order is this, and only this.
    pub wake_seq: u64,
    pub dispatched_at_tick: Tick,
    pub deadline_tick: Tick,
    /// Revision the turn's observation was serialized at.
    pub observed_at_revision: u64,
    pub offered_verbs: Vec<String>,
    pub offered_targets: Vec<String>,
}

/// Why an answer that arrived was not admitted as a proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionKind {
    /// The completion names none of the offered verbs.
    NoOfferedVerb,
    /// The completion names more than one offered verb, so the choice is not
    /// a choice. Never resolved by picking the first.
    AmbiguousVerb,
    /// The completion names none of the offered (proven) targets.
    NoOfferedTarget,
    /// The completion names more than one offered target.
    AmbiguousTarget,
    /// The world moved under the observation while the inference was in
    /// flight, and the proposal's precondition no longer holds.
    StaleObservation,
    /// The precondition does not hold and the world did not move — the model
    /// proposed something that was never true.
    UnprovenTarget,
}

/// The total vocabulary of what a turn can come to. Every arm is measured and
/// distinguishable; there is no "nothing happened" state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum TurnDisposition {
    /// Admitted. This is a candidate IntentProposal for the body (L3) to
    /// attempt — still not a commit.
    Proposal {
        turn_id: String,
        actor_id: String,
        wake_seq: u64,
        verb: String,
        target: String,
        justification: String,
        attribution: Box<InferenceAttribution>,
    },
    /// An answer arrived and the world disposed against it, with a reason.
    Rejected {
        turn_id: String,
        actor_id: String,
        wake_seq: u64,
        kind: RejectionKind,
        detail: String,
        attribution: Box<InferenceAttribution>,
    },
    /// The provider replied and declined.
    Refused {
        turn_id: String,
        actor_id: String,
        wake_seq: u64,
        category: String,
        detail: String,
        attribution: Box<InferenceAttribution>,
    },
    /// A transport ran and produced failure evidence.
    MeasurementFailed {
        turn_id: String,
        actor_id: String,
        wake_seq: u64,
        reason: String,
        attribution: Box<InferenceAttribution>,
    },
    /// A precondition for calling was known-absent. Nothing was measured.
    NotConfigured {
        turn_id: String,
        actor_id: String,
        wake_seq: u64,
        missing: String,
        detail: String,
        attribution: Box<InferenceAttribution>,
    },
    /// The chain was bounded out before any transport.
    NotAttempted {
        turn_id: String,
        actor_id: String,
        wake_seq: u64,
        reason: String,
        attribution: Box<InferenceAttribution>,
    },
    /// The deadline passed and NOTHING landed. Not a failure — an absence of
    /// measurement. There is no attribution because no chain reported one.
    Unknown {
        turn_id: String,
        actor_id: String,
        wake_seq: u64,
        reason: String,
    },
}

impl TurnDisposition {
    pub fn turn_id(&self) -> &str {
        match self {
            TurnDisposition::Proposal { turn_id, .. }
            | TurnDisposition::Rejected { turn_id, .. }
            | TurnDisposition::Refused { turn_id, .. }
            | TurnDisposition::MeasurementFailed { turn_id, .. }
            | TurnDisposition::NotConfigured { turn_id, .. }
            | TurnDisposition::NotAttempted { turn_id, .. }
            | TurnDisposition::Unknown { turn_id, .. } => turn_id,
        }
    }

    pub fn wake_seq(&self) -> u64 {
        match self {
            TurnDisposition::Proposal { wake_seq, .. }
            | TurnDisposition::Rejected { wake_seq, .. }
            | TurnDisposition::Refused { wake_seq, .. }
            | TurnDisposition::MeasurementFailed { wake_seq, .. }
            | TurnDisposition::NotConfigured { wake_seq, .. }
            | TurnDisposition::NotAttempted { wake_seq, .. }
            | TurnDisposition::Unknown { wake_seq, .. } => *wake_seq,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            TurnDisposition::Proposal { .. } => "proposal",
            TurnDisposition::Rejected { .. } => "rejected",
            TurnDisposition::Refused { .. } => "refused",
            TurnDisposition::MeasurementFailed { .. } => "measurement_failed",
            TurnDisposition::NotConfigured { .. } => "not_configured",
            TurnDisposition::NotAttempted { .. } => "not_attempted",
            TurnDisposition::Unknown { .. } => "unknown",
        }
    }
}

/// The world, as admission needs to see it. Kept tiny on purpose: the gate
/// asks only what it must to let the world dispose, and never mutates.
pub trait AdmissionWorld {
    fn current_revision(&self) -> u64;
    /// Does this exact (verb, target) still hold for this actor, right now?
    fn precondition_holds(&self, actor_id: &str, verb: &str, target: &str) -> bool;
}

/// Why a dispatch was refused. The scheduler is bounded, and being at capacity
/// is reported, never silently queued or silently dropped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispatchRefusal {
    AtCapacity { max_in_flight: u32, in_flight: usize },
    DuplicateTurn { turn_id: String },
    DuplicateWakeSeq { wake_seq: u64 },
}

struct Slot {
    turn: Turn,
    landed: Option<InferenceObservation>,
    /// Arrival index. Recorded so a run can PROVE that arrival order and
    /// admission order differ — never used to order anything.
    landing_seq: Option<u64>,
}

pub struct AdmissionGate {
    tick: Tick,
    max_in_flight: u32,
    slots: Vec<Slot>,
    next_landing_seq: u64,
}

impl AdmissionGate {
    /// Build from the authored admission spec. The spec has already been
    /// validated to say `wake_queue` / `tick_boundary` /
    /// `reject_with_receipt`; this gate implements exactly that and has no
    /// other mode.
    pub fn new(spec: &AdmissionSpec) -> Self {
        Self {
            tick: Tick(0),
            max_in_flight: spec.max_in_flight,
            slots: Vec::new(),
            next_landing_seq: 0,
        }
    }

    pub fn at_tick(spec: &AdmissionSpec, tick: Tick) -> Self {
        let mut gate = Self::new(spec);
        gate.tick = tick;
        gate
    }

    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Advance the clock by one tick. THIS is the city's heartbeat now.
    pub fn advance_tick(&mut self) -> Tick {
        self.tick = Tick(self.tick.0.saturating_add(1));
        self.tick
    }

    pub fn set_tick(&mut self, tick: Tick) {
        self.tick = tick;
    }

    pub fn in_flight(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.landed.is_none())
            .count()
    }

    pub fn pending(&self) -> usize {
        self.slots.len()
    }

    /// Admit a turn into the in-flight set. Bounded by the authored
    /// `max_in_flight` — the number that replaces "one call in flight".
    pub fn dispatch(&mut self, turn: Turn) -> Result<(), DispatchRefusal> {
        if self.slots.iter().any(|slot| slot.turn.turn_id == turn.turn_id) {
            return Err(DispatchRefusal::DuplicateTurn {
                turn_id: turn.turn_id,
            });
        }
        if self.slots.iter().any(|slot| slot.turn.wake_seq == turn.wake_seq) {
            return Err(DispatchRefusal::DuplicateWakeSeq {
                wake_seq: turn.wake_seq,
            });
        }
        let in_flight = self.in_flight();
        if in_flight >= self.max_in_flight as usize {
            return Err(DispatchRefusal::AtCapacity {
                max_in_flight: self.max_in_flight,
                in_flight,
            });
        }
        self.slots.push(Slot {
            turn,
            landed: None,
            landing_seq: None,
        });
        // Keep the endogenous order materialised, so drain never has to
        // consult arrival order even by accident.
        self.slots.sort_by_key(|slot| slot.turn.wake_seq);
        Ok(())
    }

    /// Record that an inference returned. May be called in ANY order, from any
    /// executor, at any time. Landing does not admit anything.
    pub fn land(
        &mut self,
        turn_id: &str,
        observation: InferenceObservation,
    ) -> Result<(), String> {
        let seq = self.next_landing_seq;
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.turn.turn_id == turn_id)
            .ok_or_else(|| format!("no in-flight turn {turn_id}"))?;
        if slot.landed.is_some() {
            return Err(format!("turn {turn_id} already landed"));
        }
        slot.landed = Some(observation);
        slot.landing_seq = Some(seq);
        self.next_landing_seq += 1;
        Ok(())
    }

    /// The arrival order actually observed, for evidence. Never consulted by
    /// [`Self::drain`].
    pub fn landing_order(&self) -> Vec<(String, u64)> {
        let mut order: Vec<(String, u64)> = self
            .slots
            .iter()
            .filter_map(|slot| {
                slot.landing_seq
                    .map(|seq| (slot.turn.turn_id.clone(), seq))
            })
            .collect();
        order.sort_by_key(|(_, seq)| *seq);
        order
    }

    /// Drain admissible turns, SERIALLY, in wake order.
    ///
    /// Stops at the first slot that has neither landed nor timed out, so a
    /// faster provider can never move a later turn ahead of an earlier one.
    pub fn drain(&mut self, world: &dyn AdmissionWorld) -> Vec<TurnDisposition> {
        let mut dispositions = Vec::new();
        loop {
            let Some(slot) = self.slots.first() else { break };
            if slot.landed.is_none() {
                if self.tick.0 > slot.turn.deadline_tick.0 {
                    let slot = self.slots.remove(0);
                    dispositions.push(TurnDisposition::Unknown {
                        turn_id: slot.turn.turn_id.clone(),
                        actor_id: slot.turn.actor_id.clone(),
                        wake_seq: slot.turn.wake_seq,
                        reason: format!(
                            "deadline tick {} passed at tick {} with no inference landed; \
                             this turn was never measured",
                            slot.turn.deadline_tick.0, self.tick.0
                        ),
                    });
                    continue;
                }
                // Head-of-line: hold the city's order.
                break;
            }
            let slot = self.slots.remove(0);
            let observation = slot.landed.expect("checked above");
            dispositions.push(dispose(&slot.turn, observation, world));
        }
        dispositions
    }
}

/// The world disposes of one landed answer.
fn dispose(
    turn: &Turn,
    observation: InferenceObservation,
    world: &dyn AdmissionWorld,
) -> TurnDisposition {
    let attribution = Box::new(observation.attribution);
    match observation.outcome {
        InferenceOutcome::Refused { category, detail } => TurnDisposition::Refused {
            turn_id: turn.turn_id.clone(),
            actor_id: turn.actor_id.clone(),
            wake_seq: turn.wake_seq,
            category,
            detail,
            attribution,
        },
        InferenceOutcome::MeasurementFailed { reason } => TurnDisposition::MeasurementFailed {
            turn_id: turn.turn_id.clone(),
            actor_id: turn.actor_id.clone(),
            wake_seq: turn.wake_seq,
            reason,
            attribution,
        },
        InferenceOutcome::NotConfigured { missing, detail } => TurnDisposition::NotConfigured {
            turn_id: turn.turn_id.clone(),
            actor_id: turn.actor_id.clone(),
            wake_seq: turn.wake_seq,
            missing,
            detail,
            attribution,
        },
        InferenceOutcome::NotAttempted { reason } => TurnDisposition::NotAttempted {
            turn_id: turn.turn_id.clone(),
            actor_id: turn.actor_id.clone(),
            wake_seq: turn.wake_seq,
            reason,
            attribution,
        },
        InferenceOutcome::Answered { completion } => {
            let reject = |kind: RejectionKind, detail: String| TurnDisposition::Rejected {
                turn_id: turn.turn_id.clone(),
                actor_id: turn.actor_id.clone(),
                wake_seq: turn.wake_seq,
                kind,
                detail,
                attribution: attribution.clone(),
            };

            let verbs = matched_verbs(&completion, &turn.offered_verbs);
            let verb = match verbs.len() {
                1 => verbs[0].clone(),
                0 => {
                    return reject(
                        RejectionKind::NoOfferedVerb,
                        format!(
                            "completion names none of the offered verbs {:?}",
                            turn.offered_verbs
                        ),
                    )
                }
                _ => {
                    return reject(
                        RejectionKind::AmbiguousVerb,
                        format!("completion names {verbs:?}; a choice of many is not a choice"),
                    )
                }
            };

            let targets = matched_targets(&completion, &turn.offered_targets);
            let target = match targets.len() {
                1 => targets[0].clone(),
                0 => {
                    return reject(
                        RejectionKind::NoOfferedTarget,
                        format!(
                            "completion names none of the offered (proven) targets {:?}",
                            turn.offered_targets
                        ),
                    )
                }
                _ => {
                    return reject(
                        RejectionKind::AmbiguousTarget,
                        format!("completion names {targets:?}"),
                    )
                }
            };

            // The world disposes. The observation was frozen at
            // `observed_at_revision`; check the ground NOW.
            if !world.precondition_holds(&turn.actor_id, &verb, &target) {
                let moved = world.current_revision() > turn.observed_at_revision;
                let kind = if moved {
                    RejectionKind::StaleObservation
                } else {
                    RejectionKind::UnprovenTarget
                };
                return reject(
                    kind,
                    format!(
                        "precondition for {verb} on {target} does not hold at revision {} \
                         (observation was frozen at revision {})",
                        world.current_revision(),
                        turn.observed_at_revision
                    ),
                );
            }

            TurnDisposition::Proposal {
                turn_id: turn.turn_id.clone(),
                actor_id: turn.actor_id.clone(),
                wake_seq: turn.wake_seq,
                verb,
                target,
                justification: completion.trim().to_string(),
                attribution,
            }
        }
    }
}

/// Offered verbs that appear as whole words in the completion. Case
/// insensitive; no stemming, no fuzzy matching, no "closest verb" repair.
fn matched_verbs(completion: &str, offered: &[String]) -> Vec<String> {
    let lowered = completion.to_lowercase();
    let words: Vec<&str> = lowered
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .collect();
    offered
        .iter()
        .filter(|verb| {
            let verb = verb.to_lowercase();
            words.iter().any(|word| *word == verb)
        })
        .cloned()
        .collect()
}

/// Offered targets that appear in the completion. Targets are canonical ids
/// containing punctuation, so containment (not word split) is the honest test.
fn matched_targets(completion: &str, offered: &[String]) -> Vec<String> {
    offered
        .iter()
        .filter(|target| completion.contains(target.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::InferenceAttribution;

    struct World {
        revision: u64,
        holds: bool,
    }
    impl AdmissionWorld for World {
        fn current_revision(&self) -> u64 {
            self.revision
        }
        fn precondition_holds(&self, _actor: &str, _verb: &str, _target: &str) -> bool {
            self.holds
        }
    }

    fn spec(max_in_flight: u32) -> AdmissionSpec {
        AdmissionSpec {
            order: "wake_queue".into(),
            clock: "tick_boundary".into(),
            max_in_flight,
            stale_policy: "reject_with_receipt".into(),
        }
    }

    fn turn(id: &str, wake_seq: u64, deadline: u64) -> Turn {
        Turn {
            turn_id: id.into(),
            actor_id: format!("actor:{id}"),
            wake_seq,
            dispatched_at_tick: Tick(1),
            deadline_tick: Tick(deadline),
            observed_at_revision: 10,
            offered_verbs: vec!["inspect".into(), "connect".into()],
            offered_targets: vec!["thing:a".into(), "thing:b".into()],
        }
    }

    fn attribution(turn_id: &str) -> InferenceAttribution {
        InferenceAttribution {
            turn_id: turn_id.into(),
            actor_id: format!("actor:{turn_id}"),
            route_id: "r".into(),
            routing_id: "t".into(),
            routing_version: "1".into(),
            routing_source: "test".into(),
            attempts: Vec::new(),
            served_by: Some("p".into()),
            dispatched_at_tick: Tick(1),
            observed_at_tick: Tick(1),
            deadline_tick: Tick(9),
            observed_at_revision: 10,
            total_latency_ms: 1,
            prompt_bytes: 3,
            prompt_digest: String::new(),
            budget_charged: 1,
            budget_allowed: 10,
        }
    }

    fn answered(turn_id: &str, completion: &str) -> InferenceObservation {
        InferenceObservation {
            outcome: InferenceOutcome::Answered {
                completion: completion.into(),
            },
            attribution: attribution(turn_id),
        }
    }

    // =======================================================================
    // The load-bearing property: the inference is NOT the clock.
    // =======================================================================

    #[test]
    fn landing_order_does_not_change_admission_order() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        gate.dispatch(turn("t2", 2, 9)).unwrap();
        gate.dispatch(turn("t3", 3, 9)).unwrap();

        // Answers come back in the WORST order: last dispatched, first landed.
        // This is exactly what a fast remote provider next to a slow local one
        // would produce.
        gate.land("t3", answered("t3", "inspect thing:a")).unwrap();
        gate.land("t2", answered("t2", "connect thing:b")).unwrap();
        gate.land("t1", answered("t1", "inspect thing:b")).unwrap();

        assert_eq!(
            gate.landing_order()
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            vec!["t3", "t2", "t1"],
            "arrival order really was reversed"
        );

        let dispositions = gate.drain(&World {
            revision: 10,
            holds: true,
        });
        assert_eq!(
            dispositions
                .iter()
                .map(TurnDisposition::turn_id)
                .collect::<Vec<_>>(),
            vec!["t1", "t2", "t3"],
            "admission must follow the endogenous wake order, not arrival"
        );
        assert_eq!(
            dispositions
                .iter()
                .map(TurnDisposition::wake_seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn a_faster_provider_cannot_overtake_an_earlier_turn() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("slow", 1, 9)).unwrap();
        gate.dispatch(turn("fast", 2, 9)).unwrap();

        // The fast provider answers first...
        gate.land("fast", answered("fast", "inspect thing:a")).unwrap();
        let world = World {
            revision: 10,
            holds: true,
        };
        // ...and nothing is admitted, because turn 1 is still out.
        assert!(gate.drain(&world).is_empty());
        assert_eq!(gate.pending(), 2);

        // Only when the earlier turn lands does the city move, in its order.
        gate.land("slow", answered("slow", "connect thing:b")).unwrap();
        let dispositions = gate.drain(&world);
        assert_eq!(
            dispositions
                .iter()
                .map(TurnDisposition::turn_id)
                .collect::<Vec<_>>(),
            vec!["slow", "fast"]
        );
    }

    #[test]
    fn a_turn_that_never_lands_becomes_unknown_at_its_deadline_and_unblocks_the_queue() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("stuck", 1, 3)).unwrap();
        gate.dispatch(turn("ok", 2, 9)).unwrap();
        gate.land("ok", answered("ok", "inspect thing:a")).unwrap();

        let world = World {
            revision: 10,
            holds: true,
        };
        gate.set_tick(Tick(2));
        // Before the deadline the queue holds — head-of-line, on purpose.
        assert!(gate.drain(&world).is_empty());

        // The clock, not the network, releases it.
        gate.set_tick(Tick(4));
        let dispositions = gate.drain(&world);
        assert_eq!(dispositions.len(), 2);
        match &dispositions[0] {
            TurnDisposition::Unknown { turn_id, reason, .. } => {
                assert_eq!(turn_id, "stuck");
                assert!(reason.contains("never measured"), "{reason}");
            }
            other => panic!("expected unknown, got {}", other.label()),
        }
        assert_eq!(dispositions[1].turn_id(), "ok");
        assert_eq!(gate.pending(), 0, "the loop is live, not wedged");
    }

    #[test]
    fn unknown_is_a_different_state_from_measurement_failed() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("never", 1, 1)).unwrap();
        gate.dispatch(turn("failed", 2, 9)).unwrap();
        gate.land(
            "failed",
            InferenceObservation {
                outcome: InferenceOutcome::MeasurementFailed {
                    reason: "connect refused".into(),
                },
                attribution: attribution("failed"),
            },
        )
        .unwrap();
        gate.set_tick(Tick(5));
        let dispositions = gate.drain(&World {
            revision: 10,
            holds: true,
        });
        assert_eq!(dispositions[0].label(), "unknown");
        assert_eq!(dispositions[1].label(), "measurement_failed");
        assert_ne!(dispositions[0].label(), dispositions[1].label());
    }

    // =======================================================================
    // The world disposes
    // =======================================================================

    #[test]
    fn a_stale_answer_is_rejected_with_a_receipt_not_applied() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        gate.land("t1", answered("t1", "inspect thing:a")).unwrap();
        // The world moved while the inference was in flight, and the
        // precondition no longer holds.
        let dispositions = gate.drain(&World {
            revision: 12,
            holds: false,
        });
        match &dispositions[0] {
            TurnDisposition::Rejected { kind, detail, .. } => {
                assert_eq!(*kind, RejectionKind::StaleObservation);
                assert!(detail.contains("revision 12"), "{detail}");
            }
            other => panic!("expected rejection, got {}", other.label()),
        }
    }

    #[test]
    fn an_unproven_target_is_distinguished_from_a_stale_one() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        gate.land("t1", answered("t1", "inspect thing:a")).unwrap();
        // Same failing precondition, but the world did NOT move: the model
        // proposed something that was never true.
        let dispositions = gate.drain(&World {
            revision: 10,
            holds: false,
        });
        match &dispositions[0] {
            TurnDisposition::Rejected { kind, .. } => {
                assert_eq!(*kind, RejectionKind::UnprovenTarget)
            }
            other => panic!("expected rejection, got {}", other.label()),
        }
    }

    #[test]
    fn an_answer_naming_no_offered_verb_is_rejected_not_repaired() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        gate.land("t1", answered("t1", "I would demolish thing:a")).unwrap();
        let dispositions = gate.drain(&World {
            revision: 10,
            holds: true,
        });
        match &dispositions[0] {
            TurnDisposition::Rejected { kind, .. } => {
                assert_eq!(*kind, RejectionKind::NoOfferedVerb)
            }
            other => panic!("expected rejection, got {}", other.label()),
        }
    }

    #[test]
    fn an_answer_naming_two_verbs_is_ambiguous_never_first_wins() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        gate.land(
            "t1",
            answered("t1", "I could inspect thing:a, or connect it"),
        )
        .unwrap();
        let dispositions = gate.drain(&World {
            revision: 10,
            holds: true,
        });
        match &dispositions[0] {
            TurnDisposition::Rejected { kind, .. } => {
                assert_eq!(*kind, RejectionKind::AmbiguousVerb)
            }
            other => panic!("expected rejection, got {}", other.label()),
        }
    }

    #[test]
    fn an_answer_naming_an_unoffered_target_is_rejected() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        gate.land("t1", answered("t1", "inspect thing:zzz")).unwrap();
        let dispositions = gate.drain(&World {
            revision: 10,
            holds: true,
        });
        match &dispositions[0] {
            TurnDisposition::Rejected { kind, .. } => {
                assert_eq!(*kind, RejectionKind::NoOfferedTarget)
            }
            other => panic!("expected rejection, got {}", other.label()),
        }
    }

    #[test]
    fn a_verb_that_is_only_a_substring_does_not_count() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        // "inspecting" must not match the offered verb "inspect".
        gate.land("t1", answered("t1", "inspecting thing:a")).unwrap();
        let dispositions = gate.drain(&World {
            revision: 10,
            holds: true,
        });
        assert!(matches!(
            &dispositions[0],
            TurnDisposition::Rejected {
                kind: RejectionKind::NoOfferedVerb,
                ..
            }
        ));
    }

    // =======================================================================
    // Bounding
    // =======================================================================

    #[test]
    fn max_in_flight_bounds_the_scheduler_and_says_so() {
        let mut gate = AdmissionGate::new(&spec(2));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        gate.dispatch(turn("t2", 2, 9)).unwrap();
        assert_eq!(
            gate.dispatch(turn("t3", 3, 9)),
            Err(DispatchRefusal::AtCapacity {
                max_in_flight: 2,
                in_flight: 2
            })
        );
        // Landing frees capacity even before admission.
        gate.land("t1", answered("t1", "inspect thing:a")).unwrap();
        assert!(gate.dispatch(turn("t3", 3, 9)).is_ok());
    }

    #[test]
    fn duplicate_turns_and_wake_positions_are_refused() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        assert!(matches!(
            gate.dispatch(turn("t1", 2, 9)),
            Err(DispatchRefusal::DuplicateTurn { .. })
        ));
        assert!(matches!(
            gate.dispatch(turn("t2", 1, 9)),
            Err(DispatchRefusal::DuplicateWakeSeq { .. })
        ));
    }

    #[test]
    fn landing_an_unknown_or_repeated_turn_is_an_explicit_error() {
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("t1", 1, 9)).unwrap();
        assert!(gate.land("ghost", answered("ghost", "inspect thing:a")).is_err());
        gate.land("t1", answered("t1", "inspect thing:a")).unwrap();
        assert!(gate.land("t1", answered("t1", "inspect thing:a")).is_err());
    }

    #[test]
    fn dispatch_order_does_not_matter_only_wake_seq_does() {
        // Turns handed to the gate out of wake order still admit in wake order.
        let mut gate = AdmissionGate::new(&spec(8));
        gate.dispatch(turn("c", 3, 9)).unwrap();
        gate.dispatch(turn("a", 1, 9)).unwrap();
        gate.dispatch(turn("b", 2, 9)).unwrap();
        for id in ["c", "b", "a"] {
            gate.land(id, answered(id, "inspect thing:a")).unwrap();
        }
        let dispositions = gate.drain(&World {
            revision: 10,
            holds: true,
        });
        assert_eq!(
            dispositions
                .iter()
                .map(TurnDisposition::turn_id)
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }
}
