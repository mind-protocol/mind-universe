//! Deterministic bootstrap and tick-phase orchestration.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use universe_capabilities::EffectReceipt;
use universe_core::{Revision, Tick, UniverseError};
use universe_ir::{CodeDefinition, Value};
use universe_store::{load_genesis, UniverseSnapshot, UniverseStore};
use universe_transactions::{CommitReceipt, UniverseTransaction, UniverseWriteSet};
use universe_vm::{execute_program, ExecutionLimits, ExecutionReceipt, VmError, VmHost};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootState {
    Recovering,
    Ready,
    Degraded,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickPhase {
    Ingress,
    Execution,
    Commit,
    Physics,
    Observation,
    Publish,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMechanismKind {
    Executor,
    Transport,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeMechanism {
    pub kind: RuntimeMechanismKind,
    pub name: String,
    pub activations: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeInventory {
    pub mechanisms: Vec<RuntimeMechanism>,
}

pub trait PhaseHook {
    fn run(&mut self, phase: TickPhase, snapshot: &UniverseSnapshot) -> Result<(), UniverseError>;
}

pub struct Supervisor {
    store: UniverseStore,
    snapshot: UniverseSnapshot,
    state: BootState,
    pending: Vec<UniverseTransaction>,
    runtime_activations: BTreeMap<(RuntimeMechanismKind, String), u64>,
    observed_transport_receipts: BTreeSet<String>,
}

impl Supervisor {
    pub fn boot(
        store_root: impl AsRef<Path>,
        genesis_path: impl AsRef<Path>,
    ) -> Result<Self, UniverseError> {
        let store = UniverseStore::open(store_root)?;
        let snapshot = match store.load_snapshot() {
            Ok(checkpoint) => store.replay(checkpoint)?,
            Err(UniverseError::Io(_)) => {
                let genesis = load_genesis(genesis_path)?;
                store.checkpoint(&genesis)?;
                store.replay(genesis)?
            }
            Err(error) => return Err(error),
        };
        snapshot.validate()?;
        Ok(Self {
            store,
            snapshot,
            state: BootState::Ready,
            pending: Vec::new(),
            runtime_activations: BTreeMap::new(),
            observed_transport_receipts: BTreeSet::new(),
        })
    }

    pub fn state(&self) -> BootState {
        self.state
    }

    pub fn revision(&self) -> Revision {
        self.snapshot.revision
    }

    pub fn tick(&self) -> Tick {
        self.snapshot.tick
    }

    pub fn snapshot(&self) -> &UniverseSnapshot {
        &self.snapshot
    }

    pub fn enqueue(&mut self, transaction: UniverseTransaction) {
        self.pending.push(transaction);
    }

    /// Executes graph-owned behavior and delegates proposal translation to the
    /// caller. The supervisor contains no proposal-kind or ontology policy.
    pub fn execute_graph_program<F>(
        &mut self,
        code: &CodeDefinition,
        host: &mut impl VmHost,
        inputs: &BTreeMap<String, Value>,
        limits: ExecutionLimits,
        translate: F,
    ) -> Result<ExecutionReceipt, SupervisorExecutionError>
    where
        F: FnOnce(
            &ExecutionReceipt,
            &UniverseSnapshot,
        ) -> Result<Option<UniverseWriteSet>, UniverseError>,
    {
        let receipt = execute_program(
            code,
            host,
            inputs,
            self.snapshot.revision,
            self.snapshot.tick,
            limits,
        )?;
        self.record_activation(RuntimeMechanismKind::Executor, "universe-vm");
        if let Some(write_set) = translate(&receipt, &self.snapshot)? {
            let transaction = UniverseTransaction::prepare(&self.snapshot, write_set)?;
            self.enqueue(transaction);
        }
        Ok(receipt)
    }

    /// Records a transport only when an actual transport receipt exists.
    pub fn observe_transport_receipt(
        &mut self,
        transport_name: impl Into<String>,
        receipt_id: impl Into<String>,
        _receipt: &EffectReceipt,
    ) -> bool {
        if !self.observed_transport_receipts.insert(receipt_id.into()) {
            return false;
        }
        self.record_activation(RuntimeMechanismKind::Transport, transport_name);
        true
    }

    pub fn runtime_inventory(&self) -> RuntimeInventory {
        RuntimeInventory {
            mechanisms: self
                .runtime_activations
                .iter()
                .map(|((kind, name), activations)| RuntimeMechanism {
                    kind: kind.clone(),
                    name: name.clone(),
                    activations: *activations,
                })
                .collect(),
        }
    }

    fn record_activation(&mut self, kind: RuntimeMechanismKind, name: impl Into<String>) {
        *self
            .runtime_activations
            .entry((kind, name.into()))
            .or_default() += 1;
    }

    pub fn advance(
        &mut self,
        hook: &mut dyn PhaseHook,
    ) -> Result<Vec<CommitReceipt>, UniverseError> {
        if self.state != BootState::Ready {
            return Err(UniverseError::Validation("supervisor is not ready".into()));
        }
        for phase in [TickPhase::Ingress, TickPhase::Execution] {
            hook.run(phase, &self.snapshot)?;
        }
        let boundary_tick = Tick(self.snapshot.tick.0 + 1);
        hook.run(TickPhase::Commit, &self.snapshot)?;
        let pending = std::mem::take(&mut self.pending);
        let mut receipts = Vec::with_capacity(pending.len());
        for transaction in pending {
            receipts.push(transaction.commit(&self.store, &mut self.snapshot, boundary_tick)?);
        }
        for phase in [
            TickPhase::Physics,
            TickPhase::Observation,
            TickPhase::Publish,
        ] {
            hook.run(phase, &self.snapshot)?;
        }
        Ok(receipts)
    }

    pub fn independent_readback(&self) -> Result<UniverseSnapshot, UniverseError> {
        self.store.replay(self.store.load_snapshot()?)
    }
}

#[derive(Debug)]
pub enum SupervisorExecutionError {
    Vm(VmError),
    Universe(UniverseError),
}

impl From<VmError> for SupervisorExecutionError {
    fn from(value: VmError) -> Self {
        Self::Vm(value)
    }
}

impl From<UniverseError> for SupervisorExecutionError {
    fn from(value: UniverseError) -> Self {
        Self::Universe(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::{EntityKey, Revision};
    use universe_store::EntityRecord;
    use universe_testkit::minimal_snapshot;
    use universe_transactions::{UniverseCommand, UniverseWriteSet};

    #[derive(Default)]
    struct RecordingHook(Vec<TickPhase>);
    impl PhaseHook for RecordingHook {
        fn run(
            &mut self,
            phase: TickPhase,
            _snapshot: &UniverseSnapshot,
        ) -> Result<(), UniverseError> {
            self.0.push(phase);
            Ok(())
        }
    }

    #[test]
    fn boot_commit_and_fresh_store_readback_are_real() {
        let temp = tempfile::tempdir().unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let mut supervisor = Supervisor::boot(temp.path(), genesis).unwrap();
        assert_eq!(supervisor.snapshot(), &minimal_snapshot());
        let free_key = EntityKey(
            supervisor
                .snapshot()
                .entities
                .iter()
                .map(|entity| entity.key.0)
                .max()
                .unwrap_or(0)
                + 1,
        );
        let transaction = UniverseTransaction::prepare(
            supervisor.snapshot(),
            UniverseWriteSet {
                base_revision: Revision(0),
                idempotency_key: "result-moment".into(),
                causal_ancestry: vec!["graph-read-correlation".into()],
                commands: vec![UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: free_key,
                        generation: 0,
                        symbol: 0,
                        content: None,
                    },
                }],
            },
        )
        .unwrap();
        supervisor.enqueue(transaction);
        let mut hook = RecordingHook::default();
        supervisor.advance(&mut hook).unwrap();
        assert_eq!(
            hook.0,
            vec![
                TickPhase::Ingress,
                TickPhase::Execution,
                TickPhase::Commit,
                TickPhase::Physics,
                TickPhase::Observation,
                TickPhase::Publish,
            ]
        );
        let readback = supervisor.independent_readback().unwrap();
        assert_eq!(readback.revision, Revision(1));
        assert!(readback.entities.iter().any(|e| e.key == free_key));

        let transport_receipt = EffectReceipt::TransportSucceeded {
            response: b"measured".to_vec(),
        };
        assert!(supervisor.observe_transport_receipt("safe.echo", "effect-1", &transport_receipt));
        assert!(!supervisor.observe_transport_receipt("safe.echo", "effect-1", &transport_receipt));
        assert!(supervisor.observe_transport_receipt("safe.echo", "effect-2", &transport_receipt));
        assert_eq!(
            supervisor.runtime_inventory(),
            RuntimeInventory {
                mechanisms: vec![RuntimeMechanism {
                    kind: RuntimeMechanismKind::Transport,
                    name: "safe.echo".into(),
                    activations: 2,
                }],
            }
        );
    }
}
