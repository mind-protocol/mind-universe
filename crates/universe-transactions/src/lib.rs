//! Tick-boundary atomic mutation over the authoritative event store.

use serde::{Deserialize, Serialize};
use universe_core::{Revision, Tick, UniverseError};
use universe_store::{
    apply_event, EntityRecord, EventRecord, RelationRecord, UniverseMutation, UniverseSnapshot,
    UniverseStore,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UniverseCommand {
    PutEntity { entity: EntityRecord },
    PutRelation { relation: RelationRecord },
}

impl UniverseCommand {
    fn into_mutation(self) -> UniverseMutation {
        match self {
            Self::PutEntity { entity } => UniverseMutation::PutEntity { entity },
            Self::PutRelation { relation } => UniverseMutation::PutRelation { relation },
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
        // Store v0 has one durable event per atomic append. Reject wider batches
        // rather than exposing a partially replayable transaction.
        if write_set.commands.len() != 1 {
            return Err(UniverseError::Validation(
                "transaction v0 requires exactly one command".into(),
            ));
        }
        let event = EventRecord::new(
            snapshot.universe,
            snapshot.revision,
            Tick(snapshot.tick.0 + 1),
            write_set.idempotency_key.clone(),
            write_set.commands[0].clone().into_mutation(),
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
        let command = self.write_set.commands.into_iter().next().unwrap();
        let event = EventRecord::new(
            snapshot.universe,
            snapshot.revision,
            boundary_tick,
            self.write_set.idempotency_key.clone(),
            command.into_mutation(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::{EntityKey, UniverseId};
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
    fn multi_command_write_set_is_rejected_honestly() {
        let snapshot = UniverseSnapshot::empty(UniverseId(9));
        let result = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "batch".into(),
                causal_ancestry: vec![],
                commands: vec![entity_command(10), entity_command(11)],
            },
        );
        assert!(matches!(result, Err(UniverseError::Validation(_))));
    }
}
