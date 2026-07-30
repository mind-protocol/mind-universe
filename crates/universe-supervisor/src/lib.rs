//! Deterministic bootstrap and tick-phase orchestration.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use universe_capabilities::{CapabilityHost, EffectExecutionReceipt, EffectIntent, EffectReceipt};
use universe_compiler::{build_execution_request, RuntimeBondArtifact, RuntimeBondPlan};
use universe_core::{EntityKey, Epistemic, Revision, Tick, UniverseError};
use universe_ir::{
    BehaviorLogicKind, CodeDefinition, ExecutionRequest, ExecutionRequestReceipt,
    ExecutionRequestState, TriggerEvent, TriggerSubscription, Value,
};
use universe_physics::{
    execute_local_atom_cluster, map_relation_physical_delta, AtomBond, AtomExecutionBudget,
    AtomExecutionReceipt, AtomSpec, BondPolarity, LocalAtomCluster, RelationBindingReadback,
    RelationPhysicalDelta, RelationPhysicsBudget, RelationPhysicsReceipt, UniversePhysics,
};
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

/// A single measured health level. This is only ever attached to a dimension
/// through [`Epistemic`], so an unmonitored dimension is `NotMeasured` rather
/// than silently `Nominal`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthLevel {
    Nominal,
    Degraded,
    Failed,
}

/// Seven independent health dimensions, each carrying its own epistemic status.
///
/// The supervisor never collapses these into one opaque score, and it never
/// reports a level for a dimension it does not actually measure. A dimension is
/// `Measured` only when the supervisor holds direct evidence for it; otherwise
/// it is `NotMeasured`. Thresholds and richer signals must arrive from graph
/// data and dedicated observers, not from native policy baked in here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SupervisorHealth {
    /// Is the tick loop advancing? The supervisor is driven synchronously and
    /// installs no heartbeat monitor, so this stays `NotMeasured`.
    pub liveness: Epistemic<HealthLevel>,
    /// Can the supervisor accept and advance work? Derived directly from the
    /// boot state, which the supervisor owns and therefore measures.
    pub readiness: Epistemic<HealthLevel>,
    /// Does the in-memory snapshot still match durable storage? No independent
    /// readback is performed inside status reporting, so this is `NotMeasured`.
    pub data_integrity: Epistemic<HealthLevel>,
    /// Are graph programs executing successfully? Activations are counted but
    /// success/failure evidence is not yet tracked, so this is `NotMeasured`.
    pub execution: Epistemic<HealthLevel>,
    /// Physics phase health is owned by the physics subsystem, not the
    /// supervisor, so this is `NotMeasured` here.
    pub physics: Epistemic<HealthLevel>,
    /// Effect/transport health, derived from real observed transport receipts.
    ///
    /// This is `NotMeasured` until at least one genuine transport receipt has
    /// been observed. Once evidence exists the level is a structural fact about
    /// the observed outcome set, not a tunable threshold: every observed
    /// transport succeeded is `Nominal`; a mix of observed successes and
    /// failures is `Degraded`; every observed transport failed is `Failed`.
    /// `Nominal` therefore reflects positive success evidence for each observed
    /// transport, never mere absence of an error.
    pub effect: Epistemic<HealthLevel>,
    /// Semantic / Behavior Loop health is measured by the compiler's Loop-health
    /// evidence, not by the supervisor, so this is `NotMeasured` here.
    pub semantic_loop: Epistemic<HealthLevel>,
}

/// An honest, read-only status surface: measured facts the supervisor owns plus
/// separated, epistemically-tagged health dimensions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SupervisorStatus {
    pub state: BootState,
    pub revision: Revision,
    pub tick: Tick,
    /// Transactions enqueued for the next tick boundary but not yet committed.
    /// This is the supervisor-owned commit backlog, distinct from any trigger
    /// scheduler backlog.
    pub pending_commit_backlog: u32,
    pub health: SupervisorHealth,
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

/// Native hard limits around graph-owned trigger budgets.
///
/// These limits contain scheduler resource use. They do not choose which
/// events are interesting or which behavior should execute.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerSchedulerLimits {
    pub max_backlog: u32,
    pub max_requests_per_tick: u32,
    pub max_fuel_per_tick: u64,
    pub max_mutations_per_tick: u32,
    pub max_tracked_idempotency_keys: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerLifecycleState {
    Accepted,
    Executing,
    Measured,
    Failed,
    Stale,
    Unknown,
    Quarantined,
    Duplicate,
    Debounced,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "issue", rename_all = "snake_case")]
pub enum TriggerSchedulerIssue {
    CompilerRejected,
    BacklogBudgetExhausted {
        maximum: u32,
    },
    IdempotencyBudgetExhausted {
        maximum: u32,
    },
    Duplicate {
        idempotency_key: String,
    },
    Cooldown {
        eligible_at: Tick,
    },
    DebouncedBy {
        replacement_request_id: String,
    },
    PerSubscriptionStorm {
        maximum_firings: u32,
        tick: Tick,
    },
    DeadlineBeforeEligibility {
        eligible_at: Tick,
        deadline: Tick,
    },
    TickRequestBudgetExhausted {
        maximum: u32,
    },
    TickFuelBudgetExhausted {
        remaining: u64,
        requested: u64,
    },
    TickMutationBudgetExhausted {
        remaining: u32,
        requested: u32,
    },
    StartingRevisionStale {
        requested: Revision,
        current: Revision,
    },
    DeadlineElapsed {
        deadline: Tick,
        current: Tick,
    },
    CodeUnavailable {
        code_definition: EntityKey,
        code_revision: Revision,
    },
    VmRejected {
        reason: String,
    },
    TranslationFailed {
        reason: String,
    },
}

/// One explicit scheduler transition. A VM result is attached only to the
/// final measured transition; accepted/executing are never presented as
/// evidence that behavior completed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerLifecycleReceipt {
    pub request_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub subscription: EntityKey,
    pub subscription_revision: Revision,
    pub event_id: String,
    pub tick: Tick,
    pub state: Epistemic<TriggerLifecycleState>,
    pub issue: Option<TriggerSchedulerIssue>,
    pub execution: Option<ExecutionReceipt>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerIngressReceipt {
    pub materializations: Vec<ExecutionRequestReceipt>,
    pub transitions: Vec<TriggerLifecycleReceipt>,
    pub backlog: u32,
}

#[derive(Clone, Debug)]
struct QueuedTrigger {
    request: ExecutionRequest,
    eligible_at: Tick,
}

/// Deterministic, bounded scheduler for graph-owned trigger subscriptions.
pub struct TriggerScheduler {
    limits: TriggerSchedulerLimits,
    queue: Vec<QueuedTrigger>,
    seen_idempotency_keys: BTreeSet<String>,
    last_measured: BTreeMap<(EntityKey, Revision), Tick>,
    ingress_tick: Option<Tick>,
    firings_this_tick: BTreeMap<(EntityKey, Revision), u32>,
}

impl TriggerScheduler {
    pub fn new(limits: TriggerSchedulerLimits) -> Result<Self, UniverseError> {
        for (field, value) in [
            ("max_backlog", limits.max_backlog as u64),
            ("max_requests_per_tick", limits.max_requests_per_tick as u64),
            ("max_fuel_per_tick", limits.max_fuel_per_tick),
            (
                "max_mutations_per_tick",
                limits.max_mutations_per_tick as u64,
            ),
            (
                "max_tracked_idempotency_keys",
                limits.max_tracked_idempotency_keys as u64,
            ),
        ] {
            if value == 0 {
                return Err(UniverseError::Validation(format!(
                    "trigger scheduler {field} must be non-zero"
                )));
            }
        }
        Ok(Self {
            limits,
            queue: Vec::new(),
            seen_idempotency_keys: BTreeSet::new(),
            last_measured: BTreeMap::new(),
            ingress_tick: None,
            firings_this_tick: BTreeMap::new(),
        })
    }

    pub fn backlog(&self) -> u32 {
        self.queue.len() as u32
    }

    pub fn contains_request(&self, request_id: &str) -> bool {
        self.queue
            .iter()
            .any(|queued| queued.request.request_id == request_id)
    }

    /// Materializes execution requests through the compiler contract and
    /// admits them in stable subscription order.
    pub fn ingest_event(
        &mut self,
        subscriptions: &[TriggerSubscription],
        event: &TriggerEvent,
        starting_universe_revision: Revision,
        issued_at_tick: Tick,
    ) -> TriggerIngressReceipt {
        if self.ingress_tick != Some(issued_at_tick) {
            self.ingress_tick = Some(issued_at_tick);
            self.firings_this_tick.clear();
        }

        let mut ordered = subscriptions.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|subscription| (subscription.subscription, subscription.revision));
        let mut materializations = Vec::with_capacity(ordered.len());
        let mut transitions = Vec::new();

        for subscription in ordered {
            let mut materialization = build_execution_request(
                subscription,
                event,
                starting_universe_revision,
                issued_at_tick,
            );
            let Some(request) = materialization.request.clone() else {
                transitions.push(TriggerLifecycleReceipt {
                    request_id: None,
                    idempotency_key: materialization.idempotency_key.clone(),
                    subscription: materialization.subscription,
                    subscription_revision: materialization.subscription_revision,
                    event_id: materialization.event_id.clone(),
                    tick: issued_at_tick,
                    state: Epistemic::Measured(TriggerLifecycleState::Quarantined),
                    issue: Some(TriggerSchedulerIssue::CompilerRejected),
                    execution: None,
                });
                materializations.push(materialization);
                continue;
            };

            if self
                .seen_idempotency_keys
                .contains(&request.idempotency_key)
            {
                materialization.state = Epistemic::Measured(ExecutionRequestState::Duplicate);
                materialization.request = None;
                transitions.push(lifecycle_receipt(
                    &request,
                    issued_at_tick,
                    TriggerLifecycleState::Duplicate,
                    Some(TriggerSchedulerIssue::Duplicate {
                        idempotency_key: request.idempotency_key.clone(),
                    }),
                    None,
                ));
                materializations.push(materialization);
                continue;
            }

            if self.seen_idempotency_keys.len() >= self.limits.max_tracked_idempotency_keys as usize
            {
                materialization.state = Epistemic::Measured(ExecutionRequestState::Quarantined);
                materialization.request = None;
                transitions.push(lifecycle_receipt(
                    &request,
                    issued_at_tick,
                    TriggerLifecycleState::Quarantined,
                    Some(TriggerSchedulerIssue::IdempotencyBudgetExhausted {
                        maximum: self.limits.max_tracked_idempotency_keys,
                    }),
                    None,
                ));
                materializations.push(materialization);
                continue;
            }

            let firing_key = (request.subscription, request.subscription_revision);
            let firings = self.firings_this_tick.entry(firing_key).or_default();
            if *firings >= subscription.controls.max_firings_per_tick {
                materialization.state = Epistemic::Measured(ExecutionRequestState::Quarantined);
                materialization.request = None;
                transitions.push(lifecycle_receipt(
                    &request,
                    issued_at_tick,
                    TriggerLifecycleState::Quarantined,
                    Some(TriggerSchedulerIssue::PerSubscriptionStorm {
                        maximum_firings: subscription.controls.max_firings_per_tick,
                        tick: issued_at_tick,
                    }),
                    None,
                ));
                materializations.push(materialization);
                continue;
            }

            if self.queue.len() >= self.limits.max_backlog as usize {
                materialization.state = Epistemic::Measured(ExecutionRequestState::Quarantined);
                materialization.request = None;
                transitions.push(lifecycle_receipt(
                    &request,
                    issued_at_tick,
                    TriggerLifecycleState::Quarantined,
                    Some(TriggerSchedulerIssue::BacklogBudgetExhausted {
                        maximum: self.limits.max_backlog,
                    }),
                    None,
                ));
                materializations.push(materialization);
                continue;
            }

            let mut eligible_at = Tick(
                issued_at_tick
                    .0
                    .saturating_add(subscription.controls.debounce_ticks as u64),
            );
            if let Some(last) = self.last_measured.get(&firing_key) {
                eligible_at = eligible_at.max(Tick(
                    last.0
                        .saturating_add(subscription.controls.cooldown_ticks as u64),
                ));
            }
            if eligible_at.0 > request.deadline_tick.0 {
                materialization.state = Epistemic::Measured(ExecutionRequestState::CoolingDown);
                materialization.request = None;
                transitions.push(lifecycle_receipt(
                    &request,
                    issued_at_tick,
                    TriggerLifecycleState::Quarantined,
                    Some(TriggerSchedulerIssue::DeadlineBeforeEligibility {
                        eligible_at,
                        deadline: request.deadline_tick,
                    }),
                    None,
                ));
                materializations.push(materialization);
                continue;
            }

            if subscription.controls.debounce_ticks > 0 {
                if let Some(index) = self.queue.iter().position(|queued| {
                    queued.request.subscription == request.subscription
                        && queued.request.subscription_revision == request.subscription_revision
                        && issued_at_tick.0
                            <= queued
                                .request
                                .issued_at_tick
                                .0
                                .saturating_add(subscription.controls.debounce_ticks as u64)
                }) {
                    let superseded = self.queue.remove(index).request;
                    transitions.push(lifecycle_receipt(
                        &superseded,
                        issued_at_tick,
                        TriggerLifecycleState::Debounced,
                        Some(TriggerSchedulerIssue::DebouncedBy {
                            replacement_request_id: request.request_id.clone(),
                        }),
                        None,
                    ));
                }
            }

            self.seen_idempotency_keys
                .insert(request.idempotency_key.clone());
            *firings += 1;
            self.queue.push(QueuedTrigger {
                request: request.clone(),
                eligible_at,
            });
            self.queue.sort_by(|left, right| {
                (
                    left.eligible_at,
                    left.request.subscription,
                    left.request.subscription_revision,
                    &left.request.trigger.event_id,
                    &left.request.request_id,
                )
                    .cmp(&(
                        right.eligible_at,
                        right.request.subscription,
                        right.request.subscription_revision,
                        &right.request.trigger.event_id,
                        &right.request.request_id,
                    ))
            });
            transitions.push(lifecycle_receipt(
                &request,
                issued_at_tick,
                TriggerLifecycleState::Accepted,
                None,
                None,
            ));
            materializations.push(materialization);
        }

        TriggerIngressReceipt {
            materializations,
            transitions,
            backlog: self.backlog(),
        }
    }
}

fn lifecycle_receipt(
    request: &ExecutionRequest,
    tick: Tick,
    state: TriggerLifecycleState,
    issue: Option<TriggerSchedulerIssue>,
    execution: Option<ExecutionReceipt>,
) -> TriggerLifecycleReceipt {
    TriggerLifecycleReceipt {
        request_id: Some(request.request_id.clone()),
        idempotency_key: Some(request.idempotency_key.clone()),
        subscription: request.subscription,
        subscription_revision: request.subscription_revision,
        event_id: request.trigger.event_id.clone(),
        tick,
        state: Epistemic::Measured(state),
        issue,
        execution,
    }
}

/// Execution evidence pinned to the exact graph-compiled physical artifact.
///
/// Compilation and physical execution are separate receipts: a valid compiled
/// plan is not evidence that the local physical situation actually ran.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeBondExecutionReceipt {
    pub artifact_hash: String,
    pub plan: RuntimeBondPlan,
    pub physical: AtomExecutionReceipt,
    pub wake_cost: u32,
    pub lifetime_ticks_used: u64,
    pub within_lifetime: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommittedRelationPhysicsReceipt {
    pub commit_revision: Revision,
    pub commit_tick: Tick,
    pub commit_idempotency_key: String,
    pub physical: RelationPhysicsReceipt,
    pub independent_readback: Vec<RelationBindingReadback>,
}

/// Applies graph-resolved relation physics only after the corresponding truth
/// mutation has a commit receipt. Predicate identity remains provenance; the
/// already-resolved physical binding selects the generic primitive.
pub fn apply_committed_relation_physics(
    physics: &mut UniversePhysics,
    commit: &CommitReceipt,
    deltas: Vec<RelationPhysicalDelta>,
    budget: RelationPhysicsBudget,
) -> Result<CommittedRelationPhysicsReceipt, UniverseError> {
    let (commit_revision, commit_tick, commit_idempotency_key) = match commit {
        CommitReceipt::Committed {
            revision,
            tick,
            idempotency_key,
            ..
        }
        | CommitReceipt::AlreadyCommitted {
            revision,
            tick,
            idempotency_key,
        } => (*revision, *tick, idempotency_key.clone()),
    };
    let mut commands = Vec::with_capacity(deltas.len());
    for delta in deltas {
        if delta.provenance.universe_revision != commit_revision {
            return Err(UniverseError::Validation(
                "relation physics provenance revision differs from commit receipt".into(),
            ));
        }
        if delta.provenance.source_event != commit_idempotency_key {
            return Err(UniverseError::Validation(
                "relation physics provenance event differs from commit receipt".into(),
            ));
        }
        commands.push(map_relation_physical_delta(delta)?);
    }
    let relation_keys = commands
        .iter()
        .map(|command| command.relation)
        .collect::<BTreeSet<_>>();
    let physical = physics.apply_relation_commands_at_tick(commit_tick, commands, budget);
    let independent_readback = relation_keys
        .into_iter()
        .filter_map(|relation| physics.relation_binding(relation))
        .collect();
    Ok(CommittedRelationPhysicsReceipt {
        commit_revision,
        commit_tick,
        commit_idempotency_key,
        physical,
        independent_readback,
    })
}

/// Generic compiler-to-physics bridge. It interprets only the trusted primitive
/// logic kind resolved by the compiler; ontology predicate identities remain
/// provenance and never select native behavior.
pub fn execute_runtime_bond_artifact(
    artifact: &RuntimeBondArtifact,
) -> Result<RuntimeBondExecutionReceipt, UniverseError> {
    artifact
        .verify()
        .map_err(|error| UniverseError::Validation(error.to_string()))?;
    let plan = &artifact.plan;
    if plan.source.atom == plan.target.atom {
        return Err(UniverseError::Validation(
            "single-Bond runtime plan requires distinct source and target Atoms".into(),
        ));
    }

    let bond_key = universe_core::RelationKey(plan.behavior_bond.0);
    let polarity = match plan.logic_kind {
        BehaviorLogicKind::Support => BondPolarity::Support,
        BehaviorLogicKind::Inhibit => BondPolarity::Inhibit,
        BehaviorLogicKind::Neutral => BondPolarity::Neutral,
    };
    let target_required_supports = match polarity {
        BondPolarity::Support => vec![bond_key],
        BondPolarity::Inhibit | BondPolarity::Neutral => Vec::new(),
    };
    let cluster = LocalAtomCluster {
        atoms: vec![
            AtomSpec {
                key: plan.source.atom,
                threshold: plan.source.threshold,
                seed_energy: plan.source.seed_energy,
                required_supports: Vec::new(),
                inhibition_threshold: plan.source.inhibition_threshold,
            },
            AtomSpec {
                key: plan.target.atom,
                threshold: plan.target.threshold,
                seed_energy: plan.target.seed_energy,
                required_supports: target_required_supports,
                inhibition_threshold: plan.target.inhibition_threshold,
            },
        ],
        bonds: vec![AtomBond {
            key: bond_key,
            source: plan.source.atom,
            target: plan.target.atom,
            polarity,
            energy: plan.transfer_energy,
        }],
        injections: Vec::new(),
    };
    let max_steps = plan.budgets.max_steps.min(plan.budgets.lifetime_ticks);
    let physical = execute_local_atom_cluster(
        cluster,
        AtomExecutionBudget {
            max_atoms: plan.budgets.max_atoms,
            max_bonds: plan.budgets.max_bonds,
            max_steps,
            max_total_energy: plan.budgets.max_total_energy,
        },
    )?;
    let lifetime_ticks_used = physical
        .run
        .end_tick
        .0
        .saturating_sub(physical.run.start_tick.0);
    Ok(RuntimeBondExecutionReceipt {
        artifact_hash: artifact.artifact_hash.clone(),
        plan: plan.clone(),
        physical,
        wake_cost: 0,
        lifetime_ticks_used,
        within_lifetime: lifetime_ticks_used <= u64::from(plan.budgets.lifetime_ticks),
    })
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
    /// Count of distinct observed transport receipts whose outcome was a
    /// success. Owned evidence backing the `effect` health dimension.
    observed_transport_successes: u64,
    /// Count of distinct observed transport receipts whose outcome was a
    /// failure. Owned evidence backing the `effect` health dimension.
    observed_transport_failures: u64,
    processed_effect_receipts: BTreeSet<String>,
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
            observed_transport_successes: 0,
            observed_transport_failures: 0,
            processed_effect_receipts: BTreeSet::new(),
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

    /// Transactions enqueued for the next tick boundary but not yet committed.
    pub fn pending_commit_backlog(&self) -> u32 {
        self.pending.len() as u32
    }

    /// Reports honest supervisor status: the tick/revision/state/backlog the
    /// supervisor directly owns, plus seven separated health dimensions. Every
    /// dimension for which the supervisor holds no evidence is `NotMeasured`; no
    /// dimension is reported `Nominal` merely because no error was seen.
    pub fn status(&self) -> SupervisorStatus {
        let readiness = match self.state {
            // Ready to accept and advance work.
            BootState::Ready => Epistemic::Measured(HealthLevel::Nominal),
            // Reachable but serving in a reduced mode.
            BootState::Degraded => Epistemic::Measured(HealthLevel::Degraded),
            // Not currently able to serve.
            BootState::Recovering | BootState::Blocked => Epistemic::Measured(HealthLevel::Failed),
        };
        SupervisorStatus {
            state: self.state,
            revision: self.snapshot.revision,
            tick: self.snapshot.tick,
            pending_commit_backlog: self.pending_commit_backlog(),
            health: SupervisorHealth {
                liveness: Epistemic::NotMeasured,
                readiness,
                data_integrity: Epistemic::NotMeasured,
                execution: Epistemic::NotMeasured,
                physics: Epistemic::NotMeasured,
                effect: self.effect_health(),
                semantic_loop: Epistemic::NotMeasured,
            },
        }
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

    /// Records a transport only when an actual transport receipt exists, and
    /// retains its measured outcome as evidence for the `effect` health
    /// dimension. Duplicate receipt ids are ignored so a single transport is
    /// never double-counted.
    pub fn observe_transport_receipt(
        &mut self,
        transport_name: impl Into<String>,
        receipt_id: impl Into<String>,
        receipt: &EffectReceipt,
    ) -> bool {
        if !self.observed_transport_receipts.insert(receipt_id.into()) {
            return false;
        }
        match receipt {
            EffectReceipt::TransportSucceeded { .. } => self.observed_transport_successes += 1,
            EffectReceipt::TransportFailed { .. } => self.observed_transport_failures += 1,
        }
        self.record_activation(RuntimeMechanismKind::Transport, transport_name);
        true
    }

    /// Derives effect/transport health from real observed transport outcomes.
    ///
    /// Returns `NotMeasured` until at least one genuine transport receipt has
    /// been observed. The level is a structural fact about the observed outcome
    /// set — not a tunable threshold — so no organization-specific policy lives
    /// here.
    fn effect_health(&self) -> Epistemic<HealthLevel> {
        match (
            self.observed_transport_successes,
            self.observed_transport_failures,
        ) {
            (0, 0) => Epistemic::NotMeasured,
            (_, 0) => Epistemic::Measured(HealthLevel::Nominal),
            (0, _) => Epistemic::Measured(HealthLevel::Failed),
            (_, _) => Epistemic::Measured(HealthLevel::Degraded),
        }
    }

    /// Executes a capability through its real adapter and lets graph-owned
    /// translation turn the measured receipt into a write set. Duplicate
    /// idempotency keys return the original receipt without enqueueing a second
    /// mutation or counting another transport activation.
    pub fn execute_effect<F>(
        &mut self,
        capability_host: &mut CapabilityHost,
        intent: &EffectIntent,
        translate: F,
    ) -> Result<EffectExecutionReceipt, UniverseError>
    where
        F: FnOnce(
            &EffectExecutionReceipt,
            &UniverseSnapshot,
        ) -> Result<Option<UniverseWriteSet>, UniverseError>,
    {
        let receipt = capability_host.execute_measured(self.snapshot.tick, intent)?;
        if self
            .processed_effect_receipts
            .contains(&receipt.idempotency_key)
        {
            return Ok(receipt);
        }

        let transaction = translate(&receipt, &self.snapshot)?
            .map(|write_set| UniverseTransaction::prepare(&self.snapshot, write_set))
            .transpose()?;

        if receipt.transport_attempted {
            self.observe_transport_receipt(
                receipt.capability.clone(),
                receipt.idempotency_key.clone(),
                &receipt.outcome,
            );
        }
        if let Some(transaction) = transaction {
            self.enqueue(transaction);
        }
        self.processed_effect_receipts
            .insert(receipt.idempotency_key.clone());
        Ok(receipt)
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
    use universe_capabilities::{EffectAdapter, EffectIntent};
    use universe_compiler::{compile_behavior_bond, BehaviorCompilationStatus};
    use universe_core::{EntityKey, RelationKey, Revision};
    use universe_ir::BehaviorBond;
    use universe_physics::{
        AtomConvergence, PhysicsBudget, PhysicsCommand, RelationBatchStatus,
        RelationBindingProvenance, RelationBindingStatus, RelationPhysicalAction,
        ResolvedRelationPhysicalBinding,
    };
    use universe_store::{EntityRecord, RelationRecord};
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

    struct Echo;
    impl EffectAdapter for Echo {
        fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
            Ok(payload.to_vec())
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

    #[test]
    fn status_reports_owned_facts_and_refuses_to_fabricate_unmeasured_health() {
        let temp = tempfile::tempdir().unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let mut supervisor = Supervisor::boot(temp.path(), genesis).unwrap();

        let status = supervisor.status();
        // Facts the supervisor directly owns are reported as measured facts.
        assert_eq!(status.state, BootState::Ready);
        assert_eq!(status.revision, supervisor.revision());
        assert_eq!(status.tick, supervisor.tick());
        assert_eq!(status.pending_commit_backlog, 0);
        // Readiness is derived from the owned boot state and is measured.
        assert_eq!(
            status.health.readiness,
            Epistemic::Measured(HealthLevel::Nominal)
        );
        // Every dimension the supervisor does not measure stays NotMeasured; it
        // is never fabricated as Nominal because no error was observed.
        for dimension in [
            &status.health.liveness,
            &status.health.data_integrity,
            &status.health.execution,
            &status.health.physics,
            &status.health.effect,
            &status.health.semantic_loop,
        ] {
            assert_eq!(dimension, &Epistemic::NotMeasured);
        }

        // The owned commit backlog reflects real enqueued work.
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
                base_revision: supervisor.revision(),
                idempotency_key: "status-backlog-probe".into(),
                causal_ancestry: vec!["status-observer".into()],
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
        assert_eq!(supervisor.status().pending_commit_backlog, 1);

        // Status serializes and round-trips, preserving epistemic tags.
        let encoded = serde_json::to_value(&supervisor.status()).unwrap();
        let decoded: SupervisorStatus = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, supervisor.status());
        assert_eq!(decoded.health.liveness, Epistemic::NotMeasured);

        supervisor.advance(&mut RecordingHook::default()).unwrap();
        assert_eq!(supervisor.status().pending_commit_backlog, 0);
    }

    #[test]
    fn effect_health_is_measured_only_from_observed_transport_outcomes() {
        let temp = tempfile::tempdir().unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");

        // No transport observed yet: effect health is NotMeasured, never
        // fabricated as Nominal from the mere absence of a failure.
        let mut supervisor = Supervisor::boot(temp.path(), genesis.clone()).unwrap();
        assert_eq!(supervisor.status().health.effect, Epistemic::NotMeasured);

        // A single observed success is positive evidence for that transport, so
        // the dimension becomes Measured(Nominal).
        let success = EffectReceipt::TransportSucceeded {
            response: b"ok".to_vec(),
        };
        assert!(supervisor.observe_transport_receipt("safe.echo", "effect-ok", &success));
        assert_eq!(
            supervisor.status().health.effect,
            Epistemic::Measured(HealthLevel::Nominal)
        );
        // A duplicate receipt id is ignored, so evidence is not double-counted
        // and the level does not change.
        assert!(!supervisor.observe_transport_receipt("safe.echo", "effect-ok", &success));
        assert_eq!(
            supervisor.status().health.effect,
            Epistemic::Measured(HealthLevel::Nominal)
        );

        // One observed failure alongside a success is a mixed outcome set, a
        // structural Degraded — not derived from any tuned threshold.
        let failure = EffectReceipt::TransportFailed {
            reason: "adapter refused".into(),
        };
        assert!(supervisor.observe_transport_receipt("safe.echo", "effect-bad", &failure));
        assert_eq!(
            supervisor.status().health.effect,
            Epistemic::Measured(HealthLevel::Degraded)
        );

        // A separate supervisor whose only observed transport failed reports
        // Measured(Failed): there is zero successful-transport evidence.
        let mut only_failures = Supervisor::boot(temp.path(), genesis).unwrap();
        assert!(only_failures.observe_transport_receipt("safe.echo", "only-bad", &failure));
        assert_eq!(
            only_failures.status().health.effect,
            Epistemic::Measured(HealthLevel::Failed)
        );
    }

    #[test]
    fn measured_effect_receipt_is_reinjected_and_read_back_once() {
        let temp = tempfile::tempdir().unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let mut supervisor = Supervisor::boot(temp.path(), genesis).unwrap();
        let receipt_entity = EntityKey(
            supervisor
                .snapshot()
                .entities
                .iter()
                .map(|entity| entity.key.0)
                .max()
                .unwrap_or(0)
                + 1,
        );
        let intent = EffectIntent {
            capability: "safe.echo".into(),
            idempotency_key: "effect-with-readback".into(),
            payload: b"transport-result".to_vec(),
            deadline_tick: Tick(1),
            causal_ancestry: vec!["decision-1".into()],
        };
        let mut capability_host = CapabilityHost::default();
        capability_host.register("safe.echo", Box::new(Echo));
        let content_store = UniverseStore::open(temp.path()).unwrap();

        let receipt = supervisor
            .execute_effect(&mut capability_host, &intent, |receipt, snapshot| {
                assert!(receipt.transport_attempted);
                assert_eq!(
                    receipt.outcome,
                    EffectReceipt::TransportSucceeded {
                        response: b"transport-result".to_vec()
                    }
                );
                let content = content_store
                    .append_content(&serde_json::to_value(receipt).unwrap())
                    .unwrap();
                Ok(Some(UniverseWriteSet {
                    base_revision: snapshot.revision,
                    idempotency_key: "reinject-effect-with-readback".into(),
                    causal_ancestry: receipt.causal_ancestry.clone(),
                    commands: vec![UniverseCommand::PutEntity {
                        entity: EntityRecord {
                            key: receipt_entity,
                            generation: 0,
                            symbol: 0,
                            content: Some(content),
                        },
                    }],
                }))
            })
            .unwrap();
        let duplicate = supervisor
            .execute_effect(&mut capability_host, &intent, |_receipt, _snapshot| {
                panic!("duplicate receipt must not be translated twice")
            })
            .unwrap();
        assert_eq!(duplicate, receipt);

        let mut hook = RecordingHook::default();
        let commit_receipts = supervisor.advance(&mut hook).unwrap();
        assert_eq!(commit_receipts.len(), 1);
        let readback = supervisor.independent_readback().unwrap();
        assert!(readback
            .entities
            .iter()
            .any(|entity| entity.key == receipt_entity));
        let independently_read_receipt = readback
            .entities
            .iter()
            .find(|entity| entity.key == receipt_entity)
            .and_then(|entity| entity.content.as_ref())
            .map(|content| content_store.read_content(content).unwrap())
            .map(|value| serde_json::from_value::<EffectExecutionReceipt>(value).unwrap())
            .unwrap();
        assert_eq!(independently_read_receipt, receipt);
        assert_eq!(
            supervisor.runtime_inventory(),
            RuntimeInventory {
                mechanisms: vec![RuntimeMechanism {
                    kind: RuntimeMechanismKind::Transport,
                    name: "safe.echo".into(),
                    activations: 1,
                }],
            }
        );
    }

    #[test]
    fn committed_relation_delta_applies_at_its_tick_with_independent_readback() {
        let temp = tempfile::tempdir().unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let mut supervisor = Supervisor::boot(temp.path(), genesis).unwrap();
        let source = supervisor.snapshot().entities[0].key;
        let target = supervisor.snapshot().entities[1].key;
        let relation = RelationKey(
            supervisor
                .snapshot()
                .relations
                .iter()
                .map(|record| record.key.0)
                .max()
                .unwrap_or(0)
                + 1,
        );
        let transaction = UniverseTransaction::prepare(
            supervisor.snapshot(),
            UniverseWriteSet {
                base_revision: supervisor.revision(),
                idempotency_key: "graph-relation-physical-binding".into(),
                causal_ancestry: vec!["resolved-graph-mapping".into()],
                commands: vec![UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: relation,
                        generation: 0,
                        source,
                        target,
                        predicate: 0,
                        content: None,
                    },
                }],
            },
        )
        .unwrap();
        supervisor.enqueue(transaction);
        let commit = supervisor
            .advance(&mut RecordingHook::default())
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let delta = RelationPhysicalDelta {
            idempotency_key: "relation-physical-command".into(),
            relation,
            source,
            target,
            semantic_predicate: EntityKey(900),
            action: RelationPhysicalAction::Add,
            binding: Some(ResolvedRelationPhysicalBinding::NoSolverObject),
            provenance: RelationBindingProvenance {
                universe_revision: supervisor.revision(),
                mapping_revision: Revision(7),
                profile: EntityKey(901),
                profile_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .into(),
                source_event: "graph-relation-physical-binding".into(),
            },
        };
        let mut physics = UniversePhysics::new(
            1.0 / 60.0,
            PhysicsBudget {
                max_active_bodies: 4,
            },
        )
        .unwrap();
        let step = physics.apply(vec![PhysicsCommand::Step]);
        assert_eq!(step.tick, supervisor.tick());
        let receipt = apply_committed_relation_physics(
            &mut physics,
            &commit,
            vec![delta.clone()],
            RelationPhysicsBudget {
                max_commands: 1,
                max_active_bindings: 1,
                max_active_joints: 0,
                max_wake_cost: 0,
            },
        )
        .unwrap();
        assert_eq!(receipt.commit_revision, supervisor.revision());
        assert_eq!(receipt.commit_tick, supervisor.tick());
        assert_eq!(receipt.physical.status, RelationBatchStatus::Applied);
        assert_eq!(receipt.independent_readback.len(), 1);
        assert_eq!(
            receipt.independent_readback[0].status,
            RelationBindingStatus::Active
        );
        assert_eq!(receipt.independent_readback[0].relation, relation);

        let mut wrong_provenance = delta;
        wrong_provenance.provenance.source_event = "uncommitted-event".into();
        let error = apply_committed_relation_physics(
            &mut physics,
            &commit,
            vec![wrong_provenance],
            RelationPhysicsBudget {
                max_commands: 1,
                max_active_bindings: 1,
                max_active_joints: 0,
                max_wake_cost: 0,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            UniverseError::Validation(message)
                if message == "relation physics provenance event differs from commit receipt"
        ));
        assert_eq!(physics.active_relation_binding_count(), 1);
    }

    #[test]
    fn graph_compiled_behavior_executes_as_bounded_physics() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/graph-ir/behavior-bond.json");
        let bond: BehaviorBond = serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        let compilation = compile_behavior_bond(&bond);
        assert_eq!(
            compilation.receipt.status,
            BehaviorCompilationStatus::Compiled
        );
        let artifact = compilation.artifact.unwrap();
        let receipt = execute_runtime_bond_artifact(&artifact).unwrap();

        assert_eq!(receipt.artifact_hash, artifact.artifact_hash);
        assert_eq!(receipt.plan.predicate, bond.predicate.unwrap());
        assert_eq!(receipt.physical.convergence, AtomConvergence::Quiescent);
        assert!(receipt.physical.energy.conserved);
        assert!(receipt.physical.containment.within_budget);
        assert!(receipt.physical.release.ephemeral_state_released);
        assert!(receipt.within_lifetime);
        assert!(receipt.physical.terminal_starved.is_empty());
        let fired = receipt
            .physical
            .run
            .steps
            .iter()
            .flat_map(|step| step.fired.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(fired, vec![bond.source.unwrap(), bond.target.unwrap()]);

        let mut alternate_predicate = bond.clone();
        alternate_predicate.predicate =
            Some(EntityKey(bond.predicate.unwrap().0.saturating_add(1)));
        let alternate_artifact = compile_behavior_bond(&alternate_predicate)
            .artifact
            .unwrap();
        let alternate_receipt = execute_runtime_bond_artifact(&alternate_artifact).unwrap();
        assert_ne!(alternate_receipt.artifact_hash, receipt.artifact_hash);
        assert_eq!(alternate_receipt.physical, receipt.physical);

        let temp = tempfile::tempdir().unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let mut supervisor = Supervisor::boot(temp.path(), genesis).unwrap();
        let receipt_entity = EntityKey(
            supervisor
                .snapshot()
                .entities
                .iter()
                .map(|entity| entity.key.0)
                .max()
                .unwrap_or(0)
                + 1,
        );
        let content_store = UniverseStore::open(temp.path()).unwrap();
        let content = content_store
            .append_content(&serde_json::to_value(&receipt).unwrap())
            .unwrap();
        let transaction = UniverseTransaction::prepare(
            supervisor.snapshot(),
            UniverseWriteSet {
                base_revision: supervisor.revision(),
                idempotency_key: "runtime-bond-execution-receipt".into(),
                causal_ancestry: vec![
                    compilation.receipt.behavior_hash,
                    artifact.artifact_hash.clone(),
                ],
                commands: vec![UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: receipt_entity,
                        generation: 0,
                        symbol: 0,
                        content: Some(content),
                    },
                }],
            },
        )
        .unwrap();
        supervisor.enqueue(transaction);
        supervisor.advance(&mut RecordingHook::default()).unwrap();

        let readback = supervisor.independent_readback().unwrap();
        let stored_content = readback
            .entities
            .iter()
            .find(|entity| entity.key == receipt_entity)
            .and_then(|entity| entity.content.as_ref())
            .unwrap();
        let independently_read_receipt: RuntimeBondExecutionReceipt =
            serde_json::from_value(content_store.read_content(stored_content).unwrap()).unwrap();
        assert_eq!(independently_read_receipt, receipt);
        RuntimeBondArtifact {
            artifact_hash: independently_read_receipt.artifact_hash,
            plan: independently_read_receipt.plan,
        }
        .verify()
        .unwrap();
    }
}
