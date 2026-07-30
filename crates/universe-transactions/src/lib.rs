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
}
