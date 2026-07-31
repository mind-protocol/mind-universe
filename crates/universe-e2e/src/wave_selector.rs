//! A `GraphWaveSelector`: a serial wake-queue over graph-resolved constructs.
//!
//! This is the seam that lets the GENERIC [`Supervisor::advance_driving_physics_wave`]
//! run a construct without the supervisor knowing any construct exists. It holds
//! a table of constructs already resolved from committed graph content (see
//! [`crate::construct_registry`]), a shared execution budget, and a `VecDeque`
//! wake-queue of construct ids.
//!
//! The wake-queue models the physics floor's discipline exactly: a construct is
//! DORMANT and costs nothing until a physics event crosses its sensor, at which
//! point the driver calls [`GraphWaveSelector::wake`] to enqueue it. On each
//! tick the supervisor calls [`PhysicsWaveSelector::select`], which POPS one
//! queued construct and hands back its resolved wave inputs — one woken construct
//! per turn, drained serially. An empty queue selects `None`: the tick runs no
//! wave. `select` only reads the snapshot and clones pre-resolved inputs; it
//! never mutates — a selector is never a channel for a store write.

use std::collections::{BTreeMap, VecDeque};

use universe_core::{EntityKey, UniverseError};
use universe_physics::AtomExecutionBudget;
use universe_store::UniverseSnapshot;
use universe_supervisor::{PhysicsWaveInputs, PhysicsWaveSelector};

use crate::construct_resolver::ResolvedConstruct;

/// A serial wake-queue selector over constructs resolved from the graph.
///
/// The queue is the whole control-flow story: dormant constructs sit outside it
/// (zero cost), a physics event enqueues one, and the serial loop drains it one
/// construct per tick. It re-arms trivially — a construct woken again is just
/// enqueued again.
pub struct GraphWaveSelector {
    /// Constructs keyed by their stable id (the `code` node [`EntityKey`]).
    table: BTreeMap<EntityKey, ResolvedConstruct>,
    /// The shared, caller-supplied execution budget every wave is bounded by.
    /// Graph authority the caller brings — never a literal buried in the seam.
    budget: AtomExecutionBudget,
    /// Ids enqueued by [`Self::wake`], drained one-per-`select` in FIFO order.
    queue: VecDeque<EntityKey>,
}

impl GraphWaveSelector {
    /// A fresh selector with an empty table and empty queue, bounded by `budget`.
    pub fn new(budget: AtomExecutionBudget) -> Self {
        Self {
            table: BTreeMap::new(),
            budget,
            queue: VecDeque::new(),
        }
    }

    /// Register a resolved construct under its stable id (its `code` node key).
    pub fn register(&mut self, id: EntityKey, resolved: ResolvedConstruct) {
        self.table.insert(id, resolved);
    }

    /// Enqueue a construct to wake on the next `select`.
    ///
    /// This models `physics-event -> wake-queue enqueue`: the driver calls it
    /// when a physics event crosses the construct's sensor. It records intent
    /// only — the wave itself runs later, when the serial loop drains the queue.
    pub fn wake(&mut self, id: EntityKey) {
        self.queue.push_back(id);
    }

    /// Whether a construct is registered under `id`.
    pub fn is_registered(&self, id: EntityKey) -> bool {
        self.table.contains_key(&id)
    }

    /// The current wake-queue depth (0 == every construct is dormant).
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
}

impl PhysicsWaveSelector for GraphWaveSelector {
    /// Pop one queued construct and return its resolved wave inputs, or `None`
    /// when the queue is empty (a tick with no wave). Reads only the snapshot's
    /// existence; never mutates it.
    fn select(
        &mut self,
        _snapshot: &UniverseSnapshot,
    ) -> Result<Option<PhysicsWaveInputs>, UniverseError> {
        let Some(id) = self.queue.pop_front() else {
            return Ok(None);
        };
        let resolved = self.table.get(&id).ok_or_else(|| {
            UniverseError::Validation(format!("woken construct {id} is not registered"))
        })?;
        Ok(Some(PhysicsWaveInputs {
            sensor_cluster: resolved.sensor_cluster.clone(),
            deposit_bindings: resolved.deposit_bindings.clone(),
            construct_cluster: resolved.construct_cluster.clone(),
            effect_bindings: resolved.effect_bindings.clone(),
            budget: self.budget,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::UniverseId;
    use universe_physics::LocalAtomCluster;

    fn budget() -> AtomExecutionBudget {
        AtomExecutionBudget {
            max_atoms: 8,
            max_bonds: 8,
            max_steps: 8,
            max_total_energy: 1_000,
        }
    }

    fn empty_resolved() -> ResolvedConstruct {
        ResolvedConstruct {
            sensor_cluster: LocalAtomCluster {
                atoms: Vec::new(),
                bonds: Vec::new(),
                injections: Vec::new(),
            },
            deposit_bindings: Vec::new(),
            construct_cluster: LocalAtomCluster {
                atoms: Vec::new(),
                bonds: Vec::new(),
                injections: Vec::new(),
            },
            effect_bindings: Vec::new(),
            atom_keys: Default::default(),
            bond_keys: Default::default(),
        }
    }

    /// An empty queue selects nothing; a woken construct is drained exactly once
    /// and re-arms only on a fresh `wake`.
    #[test]
    fn drains_one_per_select_and_rearms() {
        let snapshot = UniverseSnapshot::empty(UniverseId(1));
        let id = EntityKey(0xD005);
        let mut selector = GraphWaveSelector::new(budget());
        selector.register(id, empty_resolved());

        // Dormant: empty queue -> None.
        assert!(selector.select(&snapshot).unwrap().is_none());
        assert_eq!(selector.queue_len(), 0);

        // Woken: one enqueue -> exactly one wave, then drained.
        selector.wake(id);
        assert_eq!(selector.queue_len(), 1);
        assert!(selector.select(&snapshot).unwrap().is_some());
        assert_eq!(selector.queue_len(), 0);
        assert!(selector.select(&snapshot).unwrap().is_none());

        // Re-arms on a fresh wake.
        selector.wake(id);
        assert!(selector.select(&snapshot).unwrap().is_some());
        assert!(selector.select(&snapshot).unwrap().is_none());
    }

    /// Waking an unregistered id is an honest error, not a silent no-op.
    #[test]
    fn unregistered_wake_is_an_error() {
        let snapshot = UniverseSnapshot::empty(UniverseId(1));
        let mut selector = GraphWaveSelector::new(budget());
        selector.wake(EntityKey(0xBEEF));
        assert!(selector.select(&snapshot).is_err());
    }
}
