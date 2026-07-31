//! Tick-boundary atomic mutation over the authoritative event store.

use serde::{Deserialize, Serialize};
use universe_core::{Revision, Tick, UniverseError};
use universe_store::{
    apply_event, EntityRecord, EventRecord, RelationRecord, UniverseMutation, UniverseSnapshot,
    UniverseStore, MAX_EVENT_MUTATIONS,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UniverseCommand {
    InternSymbols {
        symbols: Vec<String>,
    },
    PutEntity {
        entity: EntityRecord,
    },
    /// Replace the content of an EXISTING entity, preserving its stable key.
    /// The new record must carry a strictly greater `generation` than the
    /// record it supersedes. Entity keys are otherwise append-only; this is the
    /// only path that revises an entity's content in place, and it never
    /// changes the key, so every relation that references the entity survives.
    SupersedeEntity {
        entity: EntityRecord,
    },
    PutRelation {
        relation: RelationRecord,
    },
    TombstoneRelation {
        relation: universe_core::RelationKey,
        generation: u32,
    },
}

impl UniverseCommand {
    fn into_mutation(self) -> UniverseMutation {
        match self {
            Self::InternSymbols { symbols } => UniverseMutation::InternSymbols { symbols },
            Self::PutEntity { entity } => UniverseMutation::PutEntity { entity },
            Self::SupersedeEntity { entity } => UniverseMutation::SupersedeEntity { entity },
            Self::PutRelation { relation } => UniverseMutation::PutRelation { relation },
            Self::TombstoneRelation {
                relation,
                generation,
            } => UniverseMutation::TombstoneRelation {
                relation,
                generation,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UniverseWriteSet {
    pub base_revision: Revision,
    pub idempotency_key: String,
    pub causal_ancestry: Vec<String>,
    pub commands: Vec<UniverseCommand>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UniverseTransaction {
    write_set: UniverseWriteSet,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommitReceipt {
    Committed {
        previous_revision: Revision,
        revision: Revision,
        tick: Tick,
        idempotency_key: String,
        causal_ancestry: Vec<String>,
    },
    AlreadyCommitted {
        revision: Revision,
        tick: Tick,
        idempotency_key: String,
    },
}

/// Epistemic proof, carried by every rejection, of whether the batch reached
/// durable storage. A rejected batch MUST publish nothing, so its receipt
/// carries [`StoreEffect::None`]: the event log was not appended and the
/// in-memory snapshot was not advanced.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreEffect {
    /// Observed: no event was appended and the snapshot is unchanged.
    None,
    /// Observed: the event was durably appended and the snapshot advanced.
    Published,
}

/// The stage at which a batch failed its checks. Kept distinct so a receipt
/// never collapses "rejected before anything was built" into "rejected after
/// the whole batch was simulated against a candidate snapshot".
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    /// The write set violated a basic input contract before any candidate was
    /// built (for example, the boundary tick did not advance the clock).
    InputContract,
    /// Assembling the durable `EventRecord` rejected the batch (for example, a
    /// command count over budget or an empty batch).
    EventAssembly,
    /// Applying the batch to a candidate snapshot rejected a mutation (for
    /// example, a missing relation endpoint or a stale tombstone generation).
    Apply,
    /// The candidate snapshot as a whole failed a structural invariant after
    /// the batch was applied (for example, a duplicate key).
    PostApplyInvariant,
}

/// A structured, serializable, epistemically-honest receipt for a batch that
/// did NOT commit. It is a sibling of [`CommitReceipt`] rather than one of its
/// variants so that existing consumers matching only success states keep
/// compiling; a rejection is a different kind of outcome, not a kind of commit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rejection", rename_all = "snake_case")]
pub enum RejectionReceipt {
    /// The write set targeted a base revision that no longer matches the
    /// authoritative snapshot. Carries the observed expected-vs-actual
    /// revisions rather than a rendered string.
    RevisionConflict {
        idempotency_key: String,
        expected_base_revision: Revision,
        actual_revision: Revision,
        store_effect: StoreEffect,
    },
    /// A check rejected the batch before publication. Carries the specific
    /// failed invariant reported by the store and the stage it failed at, so
    /// the cause is inspectable without re-running the batch.
    ValidationFailure {
        idempotency_key: String,
        stage: ValidationStage,
        invariant: String,
        store_effect: StoreEffect,
    },
    /// The batch passed every in-memory check but durable append could not be
    /// confirmed, so the whole candidate is discarded and nothing is published.
    /// Carries how many commands were discarded and why persistence failed
    /// (a `measurement_failed` condition, not proof of absence).
    RolledBack {
        idempotency_key: String,
        discarded_commands: usize,
        reason: String,
        store_effect: StoreEffect,
    },
}

/// Total outcome of a structured commit attempt: either the batch was applied
/// (committed now, or already committed by an earlier idempotent attempt), or
/// it was rejected with a structured, inspectable cause.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitOutcome {
    Applied(CommitReceipt),
    Rejected(RejectionReceipt),
}

impl UniverseTransaction {
    pub fn prepare(
        snapshot: &UniverseSnapshot,
        write_set: UniverseWriteSet,
    ) -> Result<Self, UniverseError> {
        if write_set.base_revision != snapshot.revision {
            return Err(UniverseError::RevisionConflict {
                expected: write_set.base_revision,
                actual: snapshot.revision,
            });
        }
        if write_set.idempotency_key.trim().is_empty() {
            return Err(UniverseError::Validation(
                "transaction idempotency key is empty".into(),
            ));
        }
        if write_set.commands.is_empty() {
            return Err(UniverseError::Validation(
                "transaction requires at least one command".into(),
            ));
        }
        if write_set.commands.len() > MAX_EVENT_MUTATIONS {
            return Err(UniverseError::BudgetExhausted(format!(
                "transaction has {} commands, limit is {}",
                write_set.commands.len(),
                MAX_EVENT_MUTATIONS
            )));
        }
        let mutation = batch_mutation(write_set.commands.clone());
        let event = EventRecord::new(
            snapshot.universe,
            snapshot.revision,
            Tick(snapshot.tick.0 + 1),
            write_set.idempotency_key.clone(),
            mutation,
        )?;
        let mut candidate = snapshot.clone();
        apply_event(&mut candidate, &event)?;
        candidate.validate()?;
        Ok(Self { write_set })
    }

    /// Durably appends first, then publishes the mutation in memory.
    /// Callers invoke this only at a supervisor-owned tick boundary.
    pub fn commit(
        self,
        store: &UniverseStore,
        snapshot: &mut UniverseSnapshot,
        boundary_tick: Tick,
    ) -> Result<CommitReceipt, UniverseError> {
        if boundary_tick.0 <= snapshot.tick.0 {
            return Err(UniverseError::Validation(
                "commit tick must advance the Universe clock".into(),
            ));
        }
        if snapshot
            .event_keys
            .contains(&self.write_set.idempotency_key)
        {
            return Ok(CommitReceipt::AlreadyCommitted {
                revision: snapshot.revision,
                tick: snapshot.tick,
                idempotency_key: self.write_set.idempotency_key,
            });
        }
        if self.write_set.base_revision != snapshot.revision {
            return Err(UniverseError::RevisionConflict {
                expected: self.write_set.base_revision,
                actual: snapshot.revision,
            });
        }
        let mutation = batch_mutation(self.write_set.commands);
        let event = EventRecord::new(
            snapshot.universe,
            snapshot.revision,
            boundary_tick,
            self.write_set.idempotency_key.clone(),
            mutation,
        )?;
        let mut candidate = snapshot.clone();
        apply_event(&mut candidate, &event)?;
        candidate.validate()?;
        store.append_event(&event)?;
        let previous_revision = snapshot.revision;
        *snapshot = candidate;
        Ok(CommitReceipt::Committed {
            previous_revision,
            revision: snapshot.revision,
            tick: snapshot.tick,
            idempotency_key: self.write_set.idempotency_key,
            causal_ancestry: self.write_set.causal_ancestry,
        })
    }

    /// Structured, epistemically-honest sibling of [`UniverseTransaction::commit`].
    ///
    /// GIVEN a prepared transaction and the authoritative snapshot
    /// WHEN the batch is committed at a supervisor-owned tick boundary
    /// THEN the outcome is either [`CommitOutcome::Applied`] (committed now, or
    ///   already committed by an earlier idempotent attempt) or
    ///   [`CommitOutcome::Rejected`] with a structured cause — revision
    ///   conflict, validation failure, or rollback.
    ///
    /// A rejected batch appends nothing to the durable log and leaves the
    /// snapshot unchanged; every rejection receipt asserts this with
    /// [`StoreEffect::None`]. Idempotency is preserved: a replayed key yields
    /// [`CommitReceipt::AlreadyCommitted`] under `Applied`. This method does not
    /// invent success on failure and never collapses an unknown persistence
    /// result into a committed state.
    pub fn commit_receipt(
        self,
        store: &UniverseStore,
        snapshot: &mut UniverseSnapshot,
        boundary_tick: Tick,
    ) -> CommitOutcome {
        let idempotency_key = self.write_set.idempotency_key.clone();
        if boundary_tick.0 <= snapshot.tick.0 {
            return CommitOutcome::Rejected(RejectionReceipt::ValidationFailure {
                idempotency_key,
                stage: ValidationStage::InputContract,
                invariant: "commit tick must advance the Universe clock".into(),
                store_effect: StoreEffect::None,
            });
        }
        if snapshot.event_keys.contains(&idempotency_key) {
            return CommitOutcome::Applied(CommitReceipt::AlreadyCommitted {
                revision: snapshot.revision,
                tick: snapshot.tick,
                idempotency_key,
            });
        }
        if self.write_set.base_revision != snapshot.revision {
            return CommitOutcome::Rejected(RejectionReceipt::RevisionConflict {
                idempotency_key,
                expected_base_revision: self.write_set.base_revision,
                actual_revision: snapshot.revision,
                store_effect: StoreEffect::None,
            });
        }
        let discarded_commands = self.write_set.commands.len();
        let causal_ancestry = self.write_set.causal_ancestry;
        let mutation = batch_mutation(self.write_set.commands);
        let event = match EventRecord::new(
            snapshot.universe,
            snapshot.revision,
            boundary_tick,
            idempotency_key.clone(),
            mutation,
        ) {
            Ok(event) => event,
            Err(error) => {
                return CommitOutcome::Rejected(RejectionReceipt::ValidationFailure {
                    idempotency_key,
                    stage: ValidationStage::EventAssembly,
                    invariant: error.to_string(),
                    store_effect: StoreEffect::None,
                });
            }
        };
        let mut candidate = snapshot.clone();
        if let Err(error) = apply_event(&mut candidate, &event) {
            return CommitOutcome::Rejected(RejectionReceipt::ValidationFailure {
                idempotency_key,
                stage: ValidationStage::Apply,
                invariant: error.to_string(),
                store_effect: StoreEffect::None,
            });
        }
        if let Err(error) = candidate.validate() {
            return CommitOutcome::Rejected(RejectionReceipt::ValidationFailure {
                idempotency_key,
                stage: ValidationStage::PostApplyInvariant,
                invariant: error.to_string(),
                store_effect: StoreEffect::None,
            });
        }
        if let Err(error) = store.append_event(&event) {
            return CommitOutcome::Rejected(RejectionReceipt::RolledBack {
                idempotency_key,
                discarded_commands,
                reason: error.to_string(),
                store_effect: StoreEffect::None,
            });
        }
        let previous_revision = snapshot.revision;
        *snapshot = candidate;
        CommitOutcome::Applied(CommitReceipt::Committed {
            previous_revision,
            revision: snapshot.revision,
            tick: snapshot.tick,
            idempotency_key,
            causal_ancestry,
        })
    }
}

fn batch_mutation(commands: Vec<UniverseCommand>) -> UniverseMutation {
    let mut mutations = commands
        .into_iter()
        .map(UniverseCommand::into_mutation)
        .collect::<Vec<_>>();
    if mutations.len() == 1 {
        mutations.pop().expect("one mutation exists")
    } else {
        UniverseMutation::Batch { mutations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::{EntityKey, RelationKey, UniverseId};
    use universe_store::UniverseSnapshot;

    fn entity_command(key: u128) -> UniverseCommand {
        UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: EntityKey(key),
                generation: 0,
                symbol: 0,
                content: None,
            },
        }
    }

    #[test]
    fn commit_is_durable_before_independent_replay() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut snapshot = UniverseSnapshot::empty(UniverseId(9));
        snapshot.symbols.push("thing".into());
        store.checkpoint(&snapshot).unwrap();
        let tx = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "moment-1".into(),
                causal_ancestry: vec!["read-1".into()],
                commands: vec![entity_command(10)],
            },
        )
        .unwrap();
        let receipt = tx.commit(&store, &mut snapshot, Tick(1)).unwrap();
        assert!(matches!(receipt, CommitReceipt::Committed { .. }));

        let independent = store.replay(store.load_snapshot().unwrap()).unwrap();
        assert_eq!(independent.revision, Revision(1));
        assert!(independent.entities.iter().any(|e| e.key == EntityKey(10)));
    }

    #[test]
    fn multi_command_write_set_commits_as_one_revision_and_replays_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut snapshot = UniverseSnapshot::empty(UniverseId(9));
        snapshot.symbols.push("thing".into());
        store.checkpoint(&snapshot).unwrap();
        let tx = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "batch".into(),
                causal_ancestry: vec![],
                commands: vec![entity_command(10), entity_command(11)],
            },
        )
        .unwrap();
        tx.commit(&store, &mut snapshot, Tick(1)).unwrap();
        assert_eq!(snapshot.revision, Revision(1));
        assert_eq!(snapshot.entities.len(), 2);

        let independent = UniverseStore::open(temp.path())
            .unwrap()
            .replay(store.load_snapshot().unwrap())
            .unwrap();
        assert_eq!(independent, snapshot);
    }

    #[test]
    fn symbols_and_referring_records_commit_in_the_same_revision() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut snapshot = UniverseSnapshot::empty(UniverseId(9));
        store.checkpoint(&snapshot).unwrap();
        let plan = snapshot
            .plan_symbol_interning(&["behavior_bond".into(), "SOURCE_ATOM".into()])
            .unwrap();
        let entity_symbol = plan.assignments["behavior_bond"];
        let tx = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "symbols-and-record".into(),
                causal_ancestry: vec!["changeset-1".into()],
                commands: vec![
                    UniverseCommand::InternSymbols {
                        symbols: plan.additions,
                    },
                    UniverseCommand::PutEntity {
                        entity: EntityRecord {
                            key: EntityKey(10),
                            generation: 0,
                            symbol: entity_symbol,
                            content: None,
                        },
                    },
                ],
            },
        )
        .unwrap();
        tx.commit(&store, &mut snapshot, Tick(1)).unwrap();

        let independent = UniverseStore::open(temp.path())
            .unwrap()
            .replay(store.load_snapshot().unwrap())
            .unwrap();
        assert_eq!(independent.revision, Revision(1));
        assert_eq!(independent.symbol_id("behavior_bond"), Some(entity_symbol));
        assert_eq!(independent.entities[0].symbol, entity_symbol);
    }

    #[test]
    fn relation_tombstone_is_generation_checked_and_durably_replayed() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut snapshot = UniverseSnapshot::empty(UniverseId(10));
        snapshot.symbols.push("LINK".into());
        snapshot.entities = vec![
            EntityRecord {
                key: EntityKey(1),
                generation: 0,
                symbol: 0,
                content: None,
            },
            EntityRecord {
                key: EntityKey(2),
                generation: 0,
                symbol: 0,
                content: None,
            },
        ];
        snapshot.relations.push(RelationRecord {
            key: RelationKey(1),
            generation: 4,
            source: EntityKey(1),
            target: EntityKey(2),
            predicate: 0,
            content: None,
        });
        store.checkpoint(&snapshot).unwrap();

        let stale = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "stale-tombstone".into(),
                causal_ancestry: vec![],
                commands: vec![UniverseCommand::TombstoneRelation {
                    relation: RelationKey(1),
                    generation: 3,
                }],
            },
        );
        assert!(matches!(
            stale,
            Err(UniverseError::Validation(message))
                if message == "relation tombstone generation is stale"
        ));

        let transaction = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "valid-tombstone".into(),
                causal_ancestry: vec!["measured-observation".into()],
                commands: vec![UniverseCommand::TombstoneRelation {
                    relation: RelationKey(1),
                    generation: 4,
                }],
            },
        )
        .unwrap();
        transaction.commit(&store, &mut snapshot, Tick(1)).unwrap();
        assert!(snapshot.relations.is_empty());

        let independent = UniverseStore::open(temp.path())
            .unwrap()
            .replay(store.load_snapshot().unwrap())
            .unwrap();
        assert_eq!(independent.revision, Revision(1));
        assert!(independent.relations.is_empty());
    }

    #[test]
    fn commit_receipt_applies_then_reports_already_committed() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut snapshot = UniverseSnapshot::empty(UniverseId(9));
        snapshot.symbols.push("thing".into());
        store.checkpoint(&snapshot).unwrap();
        let tx = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "moment-1".into(),
                causal_ancestry: vec!["read-1".into()],
                commands: vec![entity_command(10)],
            },
        )
        .unwrap();
        let outcome = tx.commit_receipt(&store, &mut snapshot, Tick(1));
        assert!(matches!(
            outcome,
            CommitOutcome::Applied(CommitReceipt::Committed { .. })
        ));
        assert_eq!(snapshot.revision, Revision(1));

        // Replaying the identical key must not double-apply; it reports the
        // prior commit rather than inventing a second one.
        let replay = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(1),
                idempotency_key: "moment-1".into(),
                causal_ancestry: vec![],
                commands: vec![entity_command(10)],
            },
        )
        .unwrap();
        let outcome = replay.commit_receipt(&store, &mut snapshot, Tick(2));
        assert!(matches!(
            outcome,
            CommitOutcome::Applied(CommitReceipt::AlreadyCommitted { .. })
        ));
        assert_eq!(snapshot.revision, Revision(1));
        assert_eq!(snapshot.entities.len(), 1);
    }

    #[test]
    fn revision_conflict_receipt_publishes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut snapshot = UniverseSnapshot::empty(UniverseId(9));
        snapshot.symbols.push("thing".into());
        store.checkpoint(&snapshot).unwrap();

        // Two write sets prepared against the same base revision 0.
        let winner = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "winner".into(),
                causal_ancestry: vec![],
                commands: vec![entity_command(10)],
            },
        )
        .unwrap();
        let loser = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "loser".into(),
                causal_ancestry: vec![],
                commands: vec![entity_command(11)],
            },
        )
        .unwrap();

        // The winner advances the snapshot to revision 1.
        assert!(matches!(
            winner.commit_receipt(&store, &mut snapshot, Tick(1)),
            CommitOutcome::Applied(CommitReceipt::Committed { .. })
        ));
        assert_eq!(snapshot.revision, Revision(1));

        // The loser's base revision (0) no longer matches the snapshot (1).
        let outcome = loser.commit_receipt(&store, &mut snapshot, Tick(2));
        match outcome {
            CommitOutcome::Rejected(RejectionReceipt::RevisionConflict {
                idempotency_key,
                expected_base_revision,
                actual_revision,
                store_effect,
            }) => {
                assert_eq!(idempotency_key, "loser");
                assert_eq!(expected_base_revision, Revision(0));
                assert_eq!(actual_revision, Revision(1));
                assert_eq!(store_effect, StoreEffect::None);
            }
            other => panic!("expected revision conflict, got {other:?}"),
        }

        // No partial mutation is visible: the loser's entity never appeared,
        // and an independent replay agrees at revision 1.
        assert_eq!(snapshot.revision, Revision(1));
        assert!(!snapshot.entities.iter().any(|e| e.key == EntityKey(11)));
        let independent = UniverseStore::open(temp.path())
            .unwrap()
            .replay(store.load_snapshot().unwrap())
            .unwrap();
        assert_eq!(independent.revision, Revision(1));
        assert!(!independent.entities.iter().any(|e| e.key == EntityKey(11)));
    }

    #[test]
    fn validation_failure_receipt_publishes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut snapshot = UniverseSnapshot::empty(UniverseId(9));
        store.checkpoint(&snapshot).unwrap();

        // A batch whose base revision matches but whose command violates a
        // structural invariant: tombstoning a relation that does not exist.
        // Constructed directly to exercise commit-time defense-in-depth
        // validation (the path a caller hits when the world changed under a
        // transaction that was prepared elsewhere).
        let tx = UniverseTransaction {
            write_set: UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "invalid".into(),
                causal_ancestry: vec![],
                commands: vec![UniverseCommand::TombstoneRelation {
                    relation: RelationKey(99),
                    generation: 0,
                }],
            },
        };

        let outcome = tx.commit_receipt(&store, &mut snapshot, Tick(1));
        match outcome {
            CommitOutcome::Rejected(RejectionReceipt::ValidationFailure {
                idempotency_key,
                stage,
                invariant,
                store_effect,
            }) => {
                assert_eq!(idempotency_key, "invalid");
                assert_eq!(stage, ValidationStage::Apply);
                assert_eq!(
                    invariant,
                    "validation failed: relation tombstone target is absent"
                );
                assert_eq!(store_effect, StoreEffect::None);
            }
            other => panic!("expected validation failure, got {other:?}"),
        }

        // No partial mutation is visible: the snapshot did not advance and the
        // durable log holds no such event on independent replay.
        assert_eq!(snapshot.revision, Revision(0));
        assert!(snapshot.relations.is_empty());
        let independent = UniverseStore::open(temp.path())
            .unwrap()
            .replay(store.load_snapshot().unwrap())
            .unwrap();
        assert_eq!(independent.revision, Revision(0));
        assert!(independent.relations.is_empty());
    }

    #[test]
    fn rollback_receipt_publishes_nothing_when_durable_append_fails() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut snapshot = UniverseSnapshot::empty(UniverseId(9));
        snapshot.symbols.push("thing".into());
        store.checkpoint(&snapshot).unwrap();

        // Sabotage durable append by occupying the event-log path with a
        // directory, so the append cannot open it for writing. This exercises
        // the rollback path: every in-memory check passes, then persistence
        // cannot be confirmed, so the batch is discarded.
        std::fs::create_dir(temp.path().join("events.jsonl")).unwrap();

        let tx = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "rolled-back".into(),
                causal_ancestry: vec![],
                commands: vec![entity_command(10), entity_command(11)],
            },
        )
        .unwrap();

        let outcome = tx.commit_receipt(&store, &mut snapshot, Tick(1));
        match outcome {
            CommitOutcome::Rejected(RejectionReceipt::RolledBack {
                idempotency_key,
                discarded_commands,
                reason,
                store_effect,
            }) => {
                assert_eq!(idempotency_key, "rolled-back");
                assert_eq!(discarded_commands, 2);
                assert!(!reason.is_empty(), "rollback must record why it failed");
                assert_eq!(store_effect, StoreEffect::None);
            }
            other => panic!("expected rollback, got {other:?}"),
        }

        // No partial mutation is visible in memory: the snapshot did not
        // advance and neither entity was published.
        assert_eq!(snapshot.revision, Revision(0));
        assert!(snapshot.entities.is_empty());
    }
}
