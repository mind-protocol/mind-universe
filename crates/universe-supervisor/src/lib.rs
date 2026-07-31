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
    ExecutionRequestState, TriggerEvent, TriggerEventKind, TriggerEventPayload, TriggerSubscription,
    Value,
};
use universe_physics::{
    execute_local_atom_cluster, fired_atoms, map_relation_physical_delta,
    resolve_physics_event_deposits, AtomBond, AtomExecutionBudget, AtomExecutionReceipt,
    AtomInjectionRequest, AtomRun, AtomSpec, BondPolarity, LocalAtomCluster, PhysicsEventDeposit,
    RelationBindingReadback, RelationPhysicalDelta, RelationPhysicsBudget, RelationPhysicsReceipt,
    UniversePhysics,
};
use universe_store::{load_genesis, ContentRef, UniverseSnapshot, UniverseStore};
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

    /// Removes and returns every queued request whose eligibility tick has
    /// arrived (`eligible_at <= now`), in the queue's existing eligibility order,
    /// leaving not-yet-eligible requests in place. This is the drain half of the
    /// wake-queue: "the physics step fills a wake-queue; the serial loop drains
    /// it." Draining a request stamps `last_measured` for its subscription at
    /// `now`, so its declared cooldown applies to the next firing — a drained
    /// request has genuinely been taken up for execution, not merely observed.
    /// The queue is kept sorted by [`Self::ingest_event`], so the returned order
    /// is the same stable eligibility order the scheduler admits with.
    pub fn drain_eligible(&mut self, now: Tick) -> Vec<ExecutionRequest> {
        let mut drained = Vec::new();
        let mut remaining = Vec::with_capacity(self.queue.len());
        for queued in std::mem::take(&mut self.queue) {
            if queued.eligible_at.0 <= now.0 {
                self.last_measured.insert(
                    (
                        queued.request.subscription,
                        queued.request.subscription_revision,
                    ),
                    now,
                );
                drained.push(queued.request);
            } else {
                remaining.push(queued);
            }
        }
        self.queue = remaining;
        drained
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

/// A caller-supplied (graph-authored) declaration binding a fired construct Atom
/// to the [`EffectIntent`] it emits as a CANDIDATE.
///
/// The supervisor holds no capability policy of its own: when the bound Atom
/// crosses its threshold in the Physics phase, the exact declared intent is
/// surfaced as a candidate. It is never invented here, and — critically — never
/// committed here. Only the 4-verb write-path may commit it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicsEffectBinding {
    pub atom: EntityKey,
    pub candidate: EffectIntent,
}

/// The measured outcome of one bounded Physics-phase deposit wave.
///
/// Every field is evidence, not a self-declared success. The wave runs
/// post-Commit against the committed snapshot and never advances or mutates it.
#[derive(Clone, Debug)]
pub struct PhysicsDepositOutcome {
    /// The committed tick this wave observed. Framing only; unchanged by the wave.
    pub observed_at_tick: Tick,
    /// Measured physical evidence for the sensor cluster run.
    pub sensor: AtomExecutionReceipt,
    /// The runtime deposits the bridge routed from sensor events onto the
    /// construct's trigger Atoms. In-memory runtime state only; nothing committed.
    pub deposits: Vec<AtomInjectionRequest>,
    /// Measured physical evidence for the construct cluster run (with deposits).
    pub construct: AtomExecutionReceipt,
    /// Construct Atoms that crossed their threshold this wave.
    pub fired_construct_atoms: BTreeSet<EntityKey>,
    /// CANDIDATE effects: one per effect-binding whose Atom fired, in binding
    /// order. Proposals only — the 4-verb write-path disposes of them.
    pub candidate_effects: Vec<EffectIntent>,
}

/// The graph-authored inputs for one bounded Physics-phase deposit wave.
///
/// The supervisor holds NO cluster-selection policy: it runs exactly the wave a
/// caller-supplied [`PhysicsWaveSelector`] returns for the committed snapshot.
/// Cluster selection (e.g. a bounded `cluster_from_space`), the DepositBond
/// bindings, the effect bindings, and the budget are all authority the caller
/// brings — never a literal buried in the supervisor.
#[derive(Clone, Debug)]
pub struct PhysicsWaveInputs {
    pub sensor_cluster: LocalAtomCluster,
    pub deposit_bindings: Vec<PhysicsEventDeposit>,
    pub construct_cluster: LocalAtomCluster,
    pub effect_bindings: Vec<PhysicsEffectBinding>,
    pub budget: AtomExecutionBudget,
}

/// A caller-supplied seam that selects the bounded Physics-phase wave inputs for
/// the committed snapshot, or `None` for a tick with no wave.
///
/// This is the generic seam that lets [`Supervisor::advance_driving_physics_wave`]
/// run the wave WITHOUT the supervisor depending on any cluster-selection crate:
/// the supervisor calls `select`, the caller decides which construct (if any)
/// wakes this tick. It reads only the committed snapshot — a selector must never
/// be a channel for mutation.
pub trait PhysicsWaveSelector {
    fn select(
        &mut self,
        snapshot: &UniverseSnapshot,
    ) -> Result<Option<PhysicsWaveInputs>, UniverseError>;
}

/// The default selector: never selects a wave. It keeps the plain
/// [`Supervisor::advance`] path byte-for-byte identical to its pre-wave
/// behaviour (no wave runs), so existing callers are unaffected.
pub struct NoPhysicsWave;

impl PhysicsWaveSelector for NoPhysicsWave {
    fn select(
        &mut self,
        _snapshot: &UniverseSnapshot,
    ) -> Result<Option<PhysicsWaveInputs>, UniverseError> {
        Ok(None)
    }
}

/// Map an executed construct-cluster [`AtomRun`] to one `AtomFired` wake event
/// per atom that crossed its threshold this wave, in solver order.
///
/// This is the IN-TICK producer half of the physics-fire → wake-queue bridge.
/// The supervisor owns the universe clock, so — unlike the standalone
/// `wake_bridge` producer used for isolated proofs, which stamps the solver's
/// step tick — both `occurred_at` and `observed_at` are stamped with the
/// UNIVERSE tick the wave observed (`observed_tick`). This is load-bearing: fed
/// into the live tick, a firing dated by the solver-local step tick (which
/// starts at 0) would read as arbitrarily old against the universe clock and be
/// rejected as `StaleEvent` by the compiler's freshness check. `event_id` still
/// derives from the solver step and atom so each firing yields a distinct event.
/// A firing is an observed physical fact, so the evidence is `Measured`. Fields
/// are empty for now (the fired atom is the subject); populating them with the
/// firing evidence is the C4-LIVE producer seam.
pub fn wake_events_from_construct_run(
    run: &AtomRun,
    source_revision: Revision,
    observed_tick: Tick,
) -> Vec<TriggerEvent> {
    let mut events = Vec::new();
    for step in &run.steps {
        for atom in &step.fired {
            events.push(TriggerEvent {
                event_id: format!("atom-fired:{}:{:#x}", step.tick.0, atom.0),
                kind: TriggerEventKind::AtomFired,
                source_revision,
                occurred_at: observed_tick,
                observed_at: observed_tick,
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

/// The caller-supplied (graph / cognition authority) driver for the in-tick
/// trigger drain. When the serial loop drains a now-eligible wake request, the
/// driver resolves the request's pinned CodeDefinition, runs it against its OWN
/// VM host, and translates the measured receipt into a CANDIDATE write-set.
///
/// The supervisor owns none of this: it holds no VM host, no code-resolution
/// policy, and no proposal-to-write-set mapping. **The model proposes** (resolve
/// + execute + translate); **the world disposes** (the tick prepares and commits
/// the returned write-set at the boundary). This is the C4-LIVE seam frozen now
/// so real invocation dispatch slots in without touching `advance_inner`.
pub trait TriggerTickDriver {
    /// Resolve the CodeDefinition pinned by a drained request. The request pins
    /// `code_definition` / `code_revision` / `code_hash`; a production resolver
    /// hydrates the definition from the committed snapshot and verifies the hash
    /// so a later code revision cannot mutate an in-flight request. (Snapshot
    /// hydration and hash verification are the C4-LIVE tightening.)
    fn resolve_code(
        &mut self,
        request: &ExecutionRequest,
    ) -> Result<CodeDefinition, UniverseError>;

    /// Run the resolved program against the driver's own VM host, returning the
    /// measured [`ExecutionReceipt`]. Returning the receipt (rather than taking a
    /// host from the supervisor) keeps the supervisor generic over cognition — it
    /// never holds the host.
    fn execute(
        &mut self,
        code: &CodeDefinition,
        inputs: &BTreeMap<String, Value>,
        limits: ExecutionLimits,
        revision: Revision,
        tick: Tick,
    ) -> Result<ExecutionReceipt, UniverseError>;

    /// Translate the measured receipt into a candidate write-set (or `None`).
    /// Commit authority never moves into the supervisor: this only PROPOSES; the
    /// tick prepares and commits what is returned.
    fn translate(
        &mut self,
        request: &ExecutionRequest,
        receipt: &ExecutionReceipt,
        snapshot: &UniverseSnapshot,
    ) -> Result<Option<UniverseWriteSet>, UniverseError>;
}

/// The default driver: no trigger ever drains through it. With an empty
/// subscription set nothing is ingested and nothing is drained, so its methods
/// are never reached on the plain [`Supervisor::advance`] and
/// [`Supervisor::advance_driving_physics_wave`] paths — keeping them byte-for-byte
/// identical to their pre-slice behaviour. It returns an error rather than
/// fabricate a result, so any accidental invocation fails loudly.
pub struct NoTriggers;

impl TriggerTickDriver for NoTriggers {
    fn resolve_code(
        &mut self,
        _request: &ExecutionRequest,
    ) -> Result<CodeDefinition, UniverseError> {
        Err(UniverseError::Validation(
            "no trigger driver is configured".into(),
        ))
    }

    fn execute(
        &mut self,
        _code: &CodeDefinition,
        _inputs: &BTreeMap<String, Value>,
        _limits: ExecutionLimits,
        _revision: Revision,
        _tick: Tick,
    ) -> Result<ExecutionReceipt, UniverseError> {
        Err(UniverseError::Validation(
            "no trigger driver is configured".into(),
        ))
    }

    fn translate(
        &mut self,
        _request: &ExecutionRequest,
        _receipt: &ExecutionReceipt,
        _snapshot: &UniverseSnapshot,
    ) -> Result<Option<UniverseWriteSet>, UniverseError> {
        Err(UniverseError::Validation(
            "no trigger driver is configured".into(),
        ))
    }
}

/// The measured outcome of one advanced tick: the ordered commit receipts plus
/// the optional Physics-phase deposit wave the selector drove. `physics_wave` is
/// `Some` only when the selector returned inputs and the wave ran; its candidate
/// effects are proposals only — never committed by the tick.
#[derive(Clone, Debug)]
pub struct TickOutcome {
    pub commits: Vec<CommitReceipt>,
    pub physics_wave: Option<PhysicsDepositOutcome>,
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
    /// The wake-queue: bounded, deterministic scheduler for graph-owned trigger
    /// subscriptions. It must be OWNED so a firing ingested in tick N survives to
    /// be drained in tick N+1.
    trigger_scheduler: TriggerScheduler,
    /// The graph-authored subscription set the scheduler matches wake events
    /// against. Empty by default, so the whole trigger path is a no-op until a
    /// construct declares its subscription via [`Supervisor::register_subscription`].
    trigger_subscriptions: Vec<TriggerSubscription>,
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
        // Native hard caps around graph-owned trigger budgets. These bound
        // scheduler resource use (trusted computing base); they never choose
        // which events are interesting or which behavior executes.
        let trigger_scheduler = TriggerScheduler::new(TriggerSchedulerLimits {
            max_backlog: 1024,
            max_requests_per_tick: 256,
            max_fuel_per_tick: 1_048_576,
            max_mutations_per_tick: 1024,
            max_tracked_idempotency_keys: 65_536,
        })?;
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
            trigger_scheduler,
            trigger_subscriptions: Vec::new(),
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

    /// Admits a graph-authored trigger subscription into the wake-queue's match
    /// set. Future physics firings are ingested against the registered set; an
    /// empty set (the default) makes the entire trigger path a no-op, so callers
    /// that register nothing remain byte-for-byte unaffected.
    pub fn register_subscription(&mut self, subscription: TriggerSubscription) {
        self.trigger_subscriptions.push(subscription);
    }

    /// Wake requests enqueued on the wake-queue awaiting a future eligible tick.
    /// Distinct from [`Self::pending_commit_backlog`] (prepared transactions
    /// awaiting the commit boundary).
    pub fn trigger_backlog(&self) -> u32 {
        self.trigger_scheduler.backlog()
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

    /// Run one bounded Physics-phase deposit wave — the wiring of the
    /// physics-event -> Atom-deposit bridge into the supervisor's inert Physics
    /// hook.
    ///
    /// The wave: (1) runs a bounded sensor cluster; (2) collects the Atoms that
    /// crossed their threshold as significant physics events; (3) routes them
    /// through the declared [`PhysicsEventDeposit`] bindings onto the construct's
    /// downstream trigger Atoms — the DepositBond fire; (4) runs the construct
    /// cluster with those deposits landed as injections, so a dormant construct
    /// self-wakes; and (5) surfaces a CANDIDATE [`EffectIntent`] for each declared
    /// effect Atom that fired.
    ///
    /// It takes `&self`: it reads the committed snapshot's tick for framing and
    /// NEVER mutates the store or snapshot. A fired effect Atom becomes a
    /// candidate proposal — it is returned, not committed. Clusters and `budget`
    /// are caller-supplied from graph authority (a bounded selection such as
    /// `cluster_from_space`, and the same budget source as
    /// `execute_runtime_bond_artifact`); no budget is a buried literal here.
    pub fn run_physics_deposit_phase(
        &self,
        sensor_cluster: LocalAtomCluster,
        deposit_bindings: &[PhysicsEventDeposit],
        mut construct_cluster: LocalAtomCluster,
        effect_bindings: &[PhysicsEffectBinding],
        budget: AtomExecutionBudget,
    ) -> Result<PhysicsDepositOutcome, UniverseError> {
        // (1)-(2): run the sensor and collect its threshold crossings as events.
        let sensor = execute_local_atom_cluster(sensor_cluster, budget)?;
        let events = fired_atoms(&sensor);

        // (3): route events onto downstream construct trigger Atoms. A physics
        // event never mutates the store; the deposit is transient runtime state.
        let deposits = resolve_physics_event_deposits(deposit_bindings, &events)?;

        // (4): land the deposits as injections and run the construct cluster so
        // it self-wakes if a deposit crosses a threshold.
        construct_cluster
            .injections
            .extend(deposits.iter().cloned());
        let construct = execute_local_atom_cluster(construct_cluster, budget)?;
        let fired_construct_atoms = fired_atoms(&construct);

        // (5): a fired effect Atom becomes a CANDIDATE effect — never committed here.
        let candidate_effects = effect_bindings
            .iter()
            .filter(|binding| fired_construct_atoms.contains(&binding.atom))
            .map(|binding| binding.candidate.clone())
            .collect();

        Ok(PhysicsDepositOutcome {
            observed_at_tick: self.snapshot.tick,
            sensor,
            deposits,
            construct,
            fired_construct_atoms,
            candidate_effects,
        })
    }

    pub fn advance(
        &mut self,
        hook: &mut dyn PhaseHook,
    ) -> Result<Vec<CommitReceipt>, UniverseError> {
        Ok(self
            .advance_inner(hook, &mut NoPhysicsWave, &mut NoTriggers)?
            .commits)
    }

    /// Advance one tick AND drive the Physics-phase deposit wave from a
    /// caller-supplied selector.
    ///
    /// This is the tick-advance mechanism running the wave itself: during
    /// [`TickPhase::Physics`] — after the commit boundary, against the committed
    /// snapshot — it asks `selector` for the bounded wave inputs and, if any, runs
    /// [`Self::run_physics_deposit_phase`]. The supervisor stays generic: it holds
    /// no cluster-selection literal and never names a construct; the selector is
    /// the only authority that decides which construct wakes. A
    /// [`PhysicsDepositOutcome`] surfaces the fired-construct evidence and the
    /// CANDIDATE effects. Because the wave takes `&self` and commits nothing, the
    /// committed store is untouched by it — a PhysicsEvent never mutates the store.
    pub fn advance_driving_physics_wave(
        &mut self,
        hook: &mut dyn PhaseHook,
        selector: &mut dyn PhysicsWaveSelector,
    ) -> Result<TickOutcome, UniverseError> {
        self.advance_inner(hook, selector, &mut NoTriggers)
    }

    /// Advance one tick AND close the self-wake loop end to end: drain the
    /// wake-queue's now-eligible trigger requests into THIS tick's commit, and
    /// ingest the firings the caller-driven Physics wave produces onto the
    /// wake-queue for a FUTURE tick.
    ///
    /// This is the heartbeat CLAUDE.md names: "the physics step fills a
    /// wake-queue; the serial loop drains it, so dormant constructs cost
    /// nothing." A construct wakes on a physics firing (Physics phase, tick N)
    /// and its consequent Moment commits at the next tick's boundary (Ingress
    /// drain, tick N+1) — an honest, load-bearing two-tick latency, because the
    /// firing is observed strictly after this tick's commit boundary has closed.
    ///
    /// The supervisor stays generic: it holds no cluster policy (the `selector`
    /// decides which construct wakes), no VM host / code resolution / proposal
    /// mapping (the `driver`), and no subscription authority (registered via
    /// [`Self::register_subscription`]). The model proposes; the world disposes.
    pub fn advance_draining_triggers<D: TriggerTickDriver>(
        &mut self,
        hook: &mut dyn PhaseHook,
        selector: &mut dyn PhysicsWaveSelector,
        driver: &mut D,
    ) -> Result<TickOutcome, UniverseError> {
        self.advance_inner(hook, selector, driver)
    }

    fn advance_inner<D: TriggerTickDriver>(
        &mut self,
        hook: &mut dyn PhaseHook,
        selector: &mut dyn PhysicsWaveSelector,
        driver: &mut D,
    ) -> Result<TickOutcome, UniverseError> {
        if self.state != BootState::Ready {
            return Err(UniverseError::Validation("supervisor is not ready".into()));
        }
        for phase in [TickPhase::Ingress, TickPhase::Execution] {
            hook.run(phase, &self.snapshot)?;
        }
        // Ingress work (slice-2): drain the wake-queue's now-eligible trigger
        // requests BEFORE the commit boundary, so a self-woken construct's
        // write-set lands in THIS tick's commit. Each drained request runs its
        // pinned program on the caller's host (`driver.execute`); the caller
        // translates the measured receipt into a candidate write-set, which the
        // supervisor merely prepares and enqueues into `self.pending`. With an
        // empty subscription set the queue is always empty, so this loop never
        // runs and the tick stays byte-identical (consumer guard).
        for request in self.trigger_scheduler.drain_eligible(self.snapshot.tick) {
            let code = driver.resolve_code(&request)?;
            let inputs = match &request.trigger.evidence {
                Epistemic::Measured(payload) => payload.fields.clone(),
                _ => BTreeMap::new(),
            };
            let limits = ExecutionLimits {
                fuel: request.budgets.fuel,
                max_proposals: request.budgets.max_mutations,
            };
            let receipt = driver.execute(
                &code,
                &inputs,
                limits,
                self.snapshot.revision,
                self.snapshot.tick,
            )?;
            self.record_activation(RuntimeMechanismKind::Executor, "universe-vm");
            if let Some(write_set) = driver.translate(&request, &receipt, &self.snapshot)? {
                let transaction = UniverseTransaction::prepare(&self.snapshot, write_set)?;
                self.enqueue(transaction);
            }
        }
        let boundary_tick = Tick(self.snapshot.tick.0 + 1);
        hook.run(TickPhase::Commit, &self.snapshot)?;
        let pending = std::mem::take(&mut self.pending);
        let mut receipts = Vec::with_capacity(pending.len());
        for transaction in pending {
            receipts.push(transaction.commit(&self.store, &mut self.snapshot, boundary_tick)?);
        }
        // TickPhase::Physics — drive the wave generically from the selector,
        // against the just-committed snapshot, then run the phase hook. The wave
        // reads the snapshot (&self) and commits nothing: candidate effects are
        // proposals the 4-verb write-path disposes of, never mutations here.
        let physics_wave = match selector.select(&self.snapshot)? {
            Some(inputs) => Some(self.run_physics_deposit_phase(
                inputs.sensor_cluster,
                &inputs.deposit_bindings,
                inputs.construct_cluster,
                &inputs.effect_bindings,
                inputs.budget,
            )?),
            None => None,
        };
        // Post-Physics ingest (slice-2): map the construct wave's firings into
        // AtomFired wake events and ingest them against the registered
        // subscriptions, filling the wake-queue for a FUTURE tick to drain. This
        // runs strictly AFTER this tick's commit, which is exactly why the
        // earliest a firing can commit a Moment is the next tick's boundary — the
        // two-tick latency is exposed here, never hidden. Guarded twice: no wave
        // ⇒ this block is skipped (producer guard); no subscriptions ⇒ zero
        // enqueue (consumer guard). Either guard alone keeps the tick a no-op.
        if let Some(outcome) = &physics_wave {
            if !self.trigger_subscriptions.is_empty() {
                let events = wake_events_from_construct_run(
                    &outcome.construct.run,
                    self.snapshot.revision,
                    outcome.observed_at_tick,
                );
                for event in &events {
                    let _ingress = self.trigger_scheduler.ingest_event(
                        &self.trigger_subscriptions,
                        event,
                        self.snapshot.revision,
                        self.snapshot.tick,
                    );
                }
            }
        }
        for phase in [
            TickPhase::Physics,
            TickPhase::Observation,
            TickPhase::Publish,
        ] {
            hook.run(phase, &self.snapshot)?;
        }
        Ok(TickOutcome {
            commits: receipts,
            physics_wave,
        })
    }

    pub fn independent_readback(&self) -> Result<UniverseSnapshot, UniverseError> {
        self.store.replay(self.store.load_snapshot()?)
    }

    /// Reads a content value back from the durable content segment given its
    /// [`ContentRef`]. A thin passthrough so a read path outside the supervisor
    /// (e.g. an independent readback of a just-committed entity) can hydrate the
    /// content its entities reference, without a second store handle.
    pub fn read_content(&self, content: &ContentRef) -> Result<serde_json::Value, UniverseError> {
        self.store.read_content(content)
    }

    /// Appends a content value to the durable content segment, returning its
    /// [`ContentRef`]. A thin passthrough so a write path outside the supervisor
    /// (e.g. a headless adapter assembling a write set) can attach content to the
    /// entities it commits, without a second store handle.
    pub fn append_content(
        &self,
        value: &serde_json::Value,
    ) -> Result<ContentRef, UniverseError> {
        self.store.append_content(value)
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

    #[test]
    fn physics_phase_deposit_wakes_a_construct_and_only_proposes_the_effect() {
        let temp = tempfile::tempdir().unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let supervisor = Supervisor::boot(temp.path(), genesis).unwrap();

        // Independent readback of the committed store BEFORE the wave.
        let before = supervisor.independent_readback().unwrap();

        // A sensor Atom seeded to its threshold: an intersection-enter that
        // fires on its own (full Rapier sensor-collider geometry is out of
        // scope; the seeded firing SIMULATES the reported crossing).
        let sensor_atom = EntityKey(101);
        let sensor_cluster = LocalAtomCluster {
            atoms: vec![AtomSpec {
                key: sensor_atom,
                threshold: 100,
                seed_energy: 100,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            }],
            bonds: Vec::new(),
            injections: Vec::new(),
        };

        // A dormant construct trigger Atom (also the effect Atom): threshold 100,
        // no seed. It cannot fire until the sensor event is routed onto it.
        let trigger_atom = EntityKey(202);
        let construct_cluster = LocalAtomCluster {
            atoms: vec![AtomSpec {
                key: trigger_atom,
                threshold: 100,
                seed_energy: 0,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            }],
            bonds: Vec::new(),
            injections: Vec::new(),
        };

        // The DepositBond: sensor event -> +100 onto the construct trigger.
        let deposit_bindings = vec![PhysicsEventDeposit {
            trigger: sensor_atom,
            target: trigger_atom,
            weight: 100,
        }];

        // The declared effect: when the trigger fires, propose this `notify`.
        let candidate = EffectIntent {
            capability: "notify".into(),
            idempotency_key: "house-alarm-entry".into(),
            payload: b"entry".to_vec(),
            deadline_tick: Tick(10),
            causal_ancestry: vec!["physics-event".into()],
        };
        let effect_bindings = vec![PhysicsEffectBinding {
            atom: trigger_atom,
            candidate: candidate.clone(),
        }];

        // Budget stands in for graph authority (same source as
        // `execute_runtime_bond_artifact`'s `plan.budgets`).
        let budget = AtomExecutionBudget {
            max_atoms: 1,
            max_bonds: 1,
            max_steps: 2,
            max_total_energy: 100,
        };

        let outcome = supervisor
            .run_physics_deposit_phase(
                sensor_cluster,
                &deposit_bindings,
                construct_cluster,
                &effect_bindings,
                budget,
            )
            .unwrap();

        // The sensor fired -> the event routed onto the trigger.
        assert!(outcome.sensor.terminal_starved.is_empty());
        assert_eq!(outcome.deposits.len(), 1);
        assert_eq!(outcome.deposits[0].atom, trigger_atom);
        assert_eq!(outcome.deposits[0].energy, 100);

        // The construct self-woke and fired at its threshold.
        assert!(outcome.fired_construct_atoms.contains(&trigger_atom));

        // The fire yields exactly the declared CANDIDATE effect — proposed only.
        assert_eq!(outcome.candidate_effects, vec![candidate]);
        assert_eq!(outcome.observed_at_tick, before.tick);

        // Nothing was committed: no transaction was enqueued by the wave...
        assert!(supervisor.pending.is_empty());
        // ...and the committed store is byte-identical before and after.
        let after = supervisor.independent_readback().unwrap();
        assert_eq!(before, after);
        assert_eq!(
            serde_json::to_vec(&before).unwrap(),
            serde_json::to_vec(&after).unwrap()
        );
    }

    #[test]
    fn physics_phase_without_a_matching_event_proposes_no_effect() {
        let temp = tempfile::tempdir().unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let supervisor = Supervisor::boot(temp.path(), genesis).unwrap();
        let before = supervisor.independent_readback().unwrap();

        // A sensor Atom that does NOT reach its threshold: no event.
        let sensor_atom = EntityKey(101);
        let sensor_cluster = LocalAtomCluster {
            atoms: vec![AtomSpec {
                key: sensor_atom,
                threshold: 100,
                seed_energy: 10,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            }],
            bonds: Vec::new(),
            injections: Vec::new(),
        };
        let trigger_atom = EntityKey(202);
        let construct_cluster = LocalAtomCluster {
            atoms: vec![AtomSpec {
                key: trigger_atom,
                threshold: 100,
                seed_energy: 0,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            }],
            bonds: Vec::new(),
            injections: Vec::new(),
        };
        let outcome = supervisor
            .run_physics_deposit_phase(
                sensor_cluster,
                &[PhysicsEventDeposit {
                    trigger: sensor_atom,
                    target: trigger_atom,
                    weight: 100,
                }],
                construct_cluster,
                &[PhysicsEffectBinding {
                    atom: trigger_atom,
                    candidate: EffectIntent {
                        capability: "notify".into(),
                        idempotency_key: "unfired".into(),
                        payload: Vec::new(),
                        deadline_tick: Tick(10),
                        causal_ancestry: Vec::new(),
                    },
                }],
                AtomExecutionBudget {
                    max_atoms: 1,
                    max_bonds: 1,
                    max_steps: 2,
                    max_total_energy: 100,
                },
            )
            .unwrap();

        // No sensor firing -> no deposit -> no construct firing -> no candidate.
        assert!(outcome.deposits.is_empty());
        assert!(outcome.fired_construct_atoms.is_empty());
        assert!(outcome.candidate_effects.is_empty());
        assert_eq!(before, supervisor.independent_readback().unwrap());
    }

    #[test]
    fn advance_alone_drives_the_wave_and_wakes_a_construct_without_mutating_the_store() {
        use std::collections::BTreeMap;
        use std::fs;
        use std::path::{Path, PathBuf};

        // The caller-supplied selector carrying house-alarm-shaped wave inputs.
        // THIS is the only authority that decides a construct wakes this tick;
        // the supervisor holds no cluster literal and never names the construct.
        // It reads solely the committed snapshot — never a mutation channel.
        struct AlarmWave {
            sensor_atom: EntityKey,
            trigger_atom: EntityKey,
            candidate: EffectIntent,
            selected: u32,
        }
        impl PhysicsWaveSelector for AlarmWave {
            fn select(
                &mut self,
                _snapshot: &UniverseSnapshot,
            ) -> Result<Option<PhysicsWaveInputs>, UniverseError> {
                self.selected += 1;
                Ok(Some(PhysicsWaveInputs {
                    // A sensor Atom seeded to its threshold: it fires on its own,
                    // standing in for the reported intersection-enter.
                    sensor_cluster: LocalAtomCluster {
                        atoms: vec![AtomSpec {
                            key: self.sensor_atom,
                            threshold: 100,
                            seed_energy: 100,
                            required_supports: Vec::new(),
                            inhibition_threshold: None,
                        }],
                        bonds: Vec::new(),
                        injections: Vec::new(),
                    },
                    deposit_bindings: vec![PhysicsEventDeposit {
                        trigger: self.sensor_atom,
                        target: self.trigger_atom,
                        weight: 100,
                    }],
                    // The dormant construct trigger: cannot fire until the routed
                    // deposit crosses its threshold.
                    construct_cluster: LocalAtomCluster {
                        atoms: vec![AtomSpec {
                            key: self.trigger_atom,
                            threshold: 100,
                            seed_energy: 0,
                            required_supports: Vec::new(),
                            inhibition_threshold: None,
                        }],
                        bonds: Vec::new(),
                        injections: Vec::new(),
                    },
                    effect_bindings: vec![PhysicsEffectBinding {
                        atom: self.trigger_atom,
                        candidate: self.candidate.clone(),
                    }],
                    budget: AtomExecutionBudget {
                        max_atoms: 1,
                        max_bonds: 1,
                        max_steps: 2,
                        max_total_energy: 100,
                    },
                }))
            }
        }

        fn read_all_files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
            let mut out = BTreeMap::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(current) = stack.pop() {
                for entry in fs::read_dir(&current).unwrap() {
                    let entry = entry.unwrap();
                    let path = entry.path();
                    if entry.file_type().unwrap().is_dir() {
                        stack.push(path);
                    } else {
                        out.insert(path.clone(), fs::read(&path).unwrap());
                    }
                }
            }
            out
        }

        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        fs::create_dir_all(&store_dir).unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let mut supervisor = Supervisor::boot(&store_dir, &genesis).unwrap();
        let revision_before = supervisor.revision();
        let bytes_before = read_all_files(&store_dir);

        let candidate = EffectIntent {
            capability: "notify".into(),
            idempotency_key: "advance-driven-entry".into(),
            payload: b"entry".to_vec(),
            deadline_tick: Tick(10),
            causal_ancestry: vec!["physics-event".into()],
        };
        let mut selector = AlarmWave {
            sensor_atom: EntityKey(101),
            trigger_atom: EntityKey(202),
            candidate: candidate.clone(),
            selected: 0,
        };

        // advance ALONE drives the wave — NO manual run_physics_deposit_phase call.
        let mut hook = RecordingHook::default();
        let outcome = supervisor
            .advance_driving_physics_wave(&mut hook, &mut selector)
            .unwrap();

        // The selector was consulted exactly once, and the six phases ran in order
        // (the wave runs inside TickPhase::Physics).
        assert_eq!(selector.selected, 1);
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

        // The wave ran and the construct self-woke on the routed deposit.
        let wave = outcome
            .physics_wave
            .expect("advance must have driven a Physics-phase wave");
        assert_eq!(wave.deposits.len(), 1);
        assert_eq!(wave.deposits[0].atom, EntityKey(202));
        assert!(wave.fired_construct_atoms.contains(&EntityKey(202)));
        // Exactly the declared CANDIDATE effect surfaced — a proposal only.
        assert_eq!(wave.candidate_effects, vec![candidate]);
        // Nothing was committed by the tick (no pending transactions).
        assert!(outcome.commits.is_empty());

        // The committed store is BYTE-IDENTICAL before/after: a PhysicsEvent (and
        // the candidate it surfaces) never mutates the committed store.
        let bytes_after = read_all_files(&store_dir);
        assert_eq!(
            bytes_before, bytes_after,
            "advance's Physics wave must not mutate the committed store"
        );
        assert_eq!(
            supervisor.independent_readback().unwrap().revision,
            revision_before
        );

        // Plain advance (the default NoPhysicsWave selector) drives NO wave, so
        // existing callers are unaffected.
        let plain = supervisor.advance(&mut RecordingHook::default()).unwrap();
        assert!(plain.is_empty());
    }

    /// Invariant #1 (byte-identical): a tick with NO wave and NO subscription —
    /// the plain `advance` path through the now-generic `advance_inner` — must
    /// leave the committed store byte-for-byte identical. Both slice-2 blocks are
    /// no-ops here: the drain sees an empty queue, and the post-Physics ingest is
    /// skipped because the (NoPhysicsWave) selector returns no wave.
    #[test]
    fn no_subscription_no_wave_tick_is_byte_identical() {
        use std::collections::BTreeMap as FileMap;
        use std::fs;
        use std::path::{Path, PathBuf};

        fn read_all_files(dir: &Path) -> FileMap<PathBuf, Vec<u8>> {
            let mut out = FileMap::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(current) = stack.pop() {
                for entry in fs::read_dir(&current).unwrap() {
                    let entry = entry.unwrap();
                    let path = entry.path();
                    if entry.file_type().unwrap().is_dir() {
                        stack.push(path);
                    } else {
                        out.insert(path.clone(), fs::read(&path).unwrap());
                    }
                }
            }
            out
        }

        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        fs::create_dir_all(&store_dir).unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let mut supervisor = Supervisor::boot(&store_dir, &genesis).unwrap();

        let revision_before = supervisor.revision();
        let bytes_before = read_all_files(&store_dir);

        // No subscription registered; default NoPhysicsWave/NoTriggers path.
        let commits = supervisor.advance(&mut RecordingHook::default()).unwrap();

        assert!(commits.is_empty(), "a no-op tick commits nothing");
        assert_eq!(supervisor.revision(), revision_before, "revision unchanged");
        assert_eq!(supervisor.trigger_backlog(), 0, "wake-queue stays empty");
        assert_eq!(
            read_all_files(&store_dir),
            bytes_before,
            "a no-wave no-subscription tick must be byte-identical to before"
        );
    }

    /// The heartbeat proof: a physics wave fires a construct's trigger atom in
    /// tick N; the firing is ingested as an `AtomFired` wake event; the NEXT tick
    /// drains the wake-queue, runs the pinned graph program, and commits a Moment
    /// — all inside `advance_draining_triggers`. The 2-tick fire→commit latency is
    /// asserted, not hidden: after tick N the Moment is ABSENT (backlog 1); only
    /// after tick N+1 does it commit at revision+1, carrying the trigger-hop
    /// causal token that proves it is genuinely derived from the firing.
    #[test]
    fn wake_event_commits_a_moment_two_ticks_after_firing() {
        use universe_ir::{
            Operator, TriggerBudgets, TriggerControls, TriggerEvidenceRequirement,
        };
        use universe_vm::execute_program;

        // A trivial VM host: the wake program runs no queries, so every query
        // method is unreachable. It declares no capabilities (the program needs
        // none).
        struct NullHost;
        impl VmHost for NullHost {
            fn is_cancelled(&self) -> bool {
                false
            }
            fn capabilities(&self) -> BTreeSet<String> {
                BTreeSet::new()
            }
            fn open_query(
                &mut self,
                _: &universe_ir::QuerySpec,
                _: &Value,
                _: &Value,
            ) -> Result<Value, String> {
                Err("wake program issues no query".into())
            }
            fn await_query(&mut self, _: &Value) -> Result<Value, String> {
                Err("wake program issues no query".into())
            }
            fn follow_one(&mut self, _: &Value, _: &Value) -> Result<Value, String> {
                Err("wake program follows nothing".into())
            }
            fn entity_symbol(&mut self, _: &Value) -> Result<Value, String> {
                Err("wake program reads no symbol".into())
            }
            fn hydrate(&mut self, _: &[Value], _: u32) -> Result<Vec<Value>, String> {
                Err("wake program hydrates nothing".into())
            }
        }

        // The caller's cognition authority: resolve the pinned code, run it on its
        // OWN host, translate the receipt into the Moment write-set. The
        // supervisor owns none of this.
        struct WakeDriver {
            host: NullHost,
            code: CodeDefinition,
            code_key: EntityKey,
            moment_key: EntityKey,
        }
        impl TriggerTickDriver for WakeDriver {
            fn resolve_code(
                &mut self,
                request: &ExecutionRequest,
            ) -> Result<CodeDefinition, UniverseError> {
                // Slice-2 resolver: pinned to the one known construct code node.
                // (C4-LIVE swaps in snapshot hydration + code_hash verification.)
                if request.code_definition != self.code_key {
                    return Err(UniverseError::Validation("unknown code definition".into()));
                }
                Ok(self.code.clone())
            }
            fn execute(
                &mut self,
                code: &CodeDefinition,
                inputs: &BTreeMap<String, Value>,
                limits: ExecutionLimits,
                revision: Revision,
                tick: Tick,
            ) -> Result<ExecutionReceipt, UniverseError> {
                execute_program(code, &mut self.host, inputs, revision, tick, limits)
                    .map_err(|error| {
                        UniverseError::Validation(format!("wake program failed: {error}"))
                    })
            }
            fn translate(
                &mut self,
                request: &ExecutionRequest,
                receipt: &ExecutionReceipt,
                snapshot: &UniverseSnapshot,
            ) -> Result<Option<UniverseWriteSet>, UniverseError> {
                // The program genuinely ran and proposed exactly one command, so
                // the Moment is DERIVED, never fabricated. Its causal ancestry
                // carries the trigger-hop token, threading the subscription +
                // event + request identity into the commit so the Moment is
                // provably a consequence of the firing that woke the construct.
                if receipt.proposals.len() != 1 {
                    return Err(UniverseError::Validation(
                        "wake program did not propose exactly one command".into(),
                    ));
                }
                Ok(Some(UniverseWriteSet {
                    base_revision: snapshot.revision,
                    idempotency_key: format!("wake-moment:{}", request.request_id),
                    causal_ancestry: request.descendant_causal_tokens(),
                    commands: vec![UniverseCommand::PutEntity {
                        entity: EntityRecord {
                            key: self.moment_key,
                            generation: 0,
                            symbol: 0,
                            content: None,
                        },
                    }],
                }))
            }
        }

        // A one-shot house-alarm-shaped wave: a seeded sensor atom fires, its
        // event routes +100 onto a dormant construct trigger, which then crosses
        // its threshold and fires. It fires ONLY on the first tick, so the
        // wake-queue receives exactly one request.
        struct OneShotAlarm {
            sensor: EntityKey,
            trigger: EntityKey,
            fired: bool,
        }
        impl PhysicsWaveSelector for OneShotAlarm {
            fn select(
                &mut self,
                _snapshot: &UniverseSnapshot,
            ) -> Result<Option<PhysicsWaveInputs>, UniverseError> {
                if self.fired {
                    return Ok(None);
                }
                self.fired = true;
                Ok(Some(PhysicsWaveInputs {
                    sensor_cluster: LocalAtomCluster {
                        atoms: vec![AtomSpec {
                            key: self.sensor,
                            threshold: 100,
                            seed_energy: 100,
                            required_supports: Vec::new(),
                            inhibition_threshold: None,
                        }],
                        bonds: Vec::new(),
                        injections: Vec::new(),
                    },
                    deposit_bindings: vec![PhysicsEventDeposit {
                        trigger: self.sensor,
                        target: self.trigger,
                        weight: 100,
                    }],
                    construct_cluster: LocalAtomCluster {
                        atoms: vec![AtomSpec {
                            key: self.trigger,
                            threshold: 100,
                            seed_energy: 0,
                            required_supports: Vec::new(),
                            inhibition_threshold: None,
                        }],
                        bonds: Vec::new(),
                        injections: Vec::new(),
                    },
                    effect_bindings: Vec::new(),
                    budget: AtomExecutionBudget {
                        max_atoms: 1,
                        max_bonds: 1,
                        max_steps: 2,
                        max_total_energy: 100,
                    },
                }))
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        std::fs::create_dir_all(&store_dir).unwrap();
        let genesis = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let mut supervisor = Supervisor::boot(&store_dir, &genesis).unwrap();

        // The Moment key the woken construct's program mints (next free key).
        let moment_key = EntityKey(
            supervisor
                .snapshot()
                .entities
                .iter()
                .map(|entity| entity.key.0)
                .max()
                .unwrap_or(0)
                + 1,
        );
        let code_key = EntityKey(0x6002);

        // The construct's graph program: propose one Moment record and return it.
        // It reads no inputs (AtomFired carries none yet), so it needs no host
        // query — the smallest genuine "proposes a command" program.
        let code = CodeDefinition {
            ir_version: 0,
            revision: Revision(1),
            required_capabilities: Vec::new(),
            operators: vec![
                Operator::Constant {
                    value: Value::Entity(moment_key),
                    output: 0,
                },
                Operator::MakeRecord {
                    fields: vec![("moment".into(), 0)],
                    output: 1,
                },
                Operator::Propose {
                    command: 1,
                    output: 2,
                },
                Operator::Return { value: 2 },
            ],
        };

        // The graph-authored subscription: wake on AtomFired, run the construct's
        // pinned code. debounce/cooldown are 0 so the request is eligible as soon
        // as the wake-queue is drained on the NEXT tick — the two-tick latency
        // comes from phase ordering (ingest is post-commit, drain is pre-commit),
        // NOT from a tuned delay.
        let subscription = TriggerSubscription {
            contract_version: 0,
            subscription: EntityKey(0x6001),
            revision: Revision(7),
            enabled: true,
            event_kinds: vec![TriggerEventKind::AtomFired],
            code_definition: code_key,
            code_revision: Revision(1),
            code_hash: "a".repeat(64),
            evidence_requirement: TriggerEvidenceRequirement::Measured,
            max_event_age_ticks: 8,
            budgets: TriggerBudgets {
                fuel: 64,
                max_mutations: 4,
                max_ticks: 3,
            },
            controls: TriggerControls {
                cooldown_ticks: 0,
                debounce_ticks: 0,
                max_causal_depth: 4,
                max_firings_per_tick: 3,
            },
            idempotency_namespace: "wake-slice2".into(),
        };
        supervisor.register_subscription(subscription);

        let mut hook = RecordingHook::default();
        let mut selector = OneShotAlarm {
            sensor: EntityKey(101),
            trigger: EntityKey(202),
            fired: false,
        };
        let mut driver = WakeDriver {
            host: NullHost,
            code,
            code_key,
            moment_key,
        };

        let tick_before = supervisor.tick();
        let revision_before = supervisor.revision();

        // --- Tick N: the wave fires the construct trigger; the firing ingests one
        // AtomFired wake event. Nothing commits (the drain ran first, on an empty
        // queue), so the Moment is ABSENT and the store revision is unchanged —
        // the firing is only observed AFTER this tick's commit boundary closed.
        let out_n = supervisor
            .advance_draining_triggers(&mut hook, &mut selector, &mut driver)
            .unwrap();
        let wave = out_n
            .physics_wave
            .expect("tick N drove the one-shot wave");
        assert!(
            wave.fired_construct_atoms.contains(&EntityKey(202)),
            "the dormant construct self-woke on the routed deposit and fired"
        );
        assert!(
            out_n.commits.is_empty(),
            "tick N commits nothing — the firing is observed post-commit"
        );
        assert_eq!(
            supervisor.trigger_backlog(),
            1,
            "the firing enqueued exactly one wake request"
        );
        let after_n = supervisor.independent_readback().unwrap();
        assert_eq!(
            after_n.revision, revision_before,
            "no Moment one tick after firing — the latency is real"
        );
        assert!(
            !after_n.entities.iter().any(|e| e.key == moment_key),
            "the Moment is absent one tick after firing"
        );

        // --- Tick N+1: the drain pulls the now-eligible request, runs the pinned
        // program, and commits the Moment at THIS tick's boundary. Two advance()
        // calls: the honest two-tick fire→commit latency.
        let out_n1 = supervisor
            .advance_draining_triggers(&mut hook, &mut selector, &mut driver)
            .unwrap();
        assert_eq!(
            supervisor.trigger_backlog(),
            0,
            "the wake request drained on tick N+1"
        );
        assert_eq!(
            out_n1.commits.len(),
            1,
            "the drained request committed exactly one Moment"
        );
        let (commit_tick, ancestry) = match &out_n1.commits[0] {
            CommitReceipt::Committed {
                tick,
                causal_ancestry,
                ..
            } => (*tick, causal_ancestry.clone()),
            other => panic!("expected a committed Moment, got {other:?}"),
        };
        assert_eq!(
            commit_tick,
            Tick(tick_before.0 + 1),
            "the Moment commits at tick N+1"
        );
        // The Moment is genuinely derived from the firing: its causal ancestry
        // carries the trigger-hop token naming the subscription that woke it.
        assert!(
            ancestry
                .iter()
                .any(|token| token.starts_with("trigger-hop:v0:")
                    && token.contains("00000000000000000000000000006001")),
            "the committed Moment carries the trigger-hop causal token: {ancestry:?}"
        );

        // Independent readback: the Moment is now durably present at revision+1.
        let readback = supervisor.independent_readback().unwrap();
        assert_eq!(readback.revision, Revision(revision_before.0 + 1));
        assert!(
            readback.entities.iter().any(|e| e.key == moment_key),
            "the woken construct's Moment is independently observable after N+1"
        );

        // One VM executor activation was recorded for the drained program.
        assert!(supervisor.runtime_inventory().mechanisms.iter().any(|m| {
            m.kind == RuntimeMechanismKind::Executor && m.name == "universe-vm" && m.activations == 1
        }));

        // Measured-facts trace (visible under `--nocapture`): the heartbeat, honest.
        println!("-- heartbeat: physics fire -> wake-queue -> drain -> Moment --");
        println!(
            "tick N   : construct atom {:#x} FIRED in Physics; ingested 1 AtomFired; backlog=1; commits={}; Moment absent; revision={}",
            EntityKey(202).0,
            out_n.commits.len(),
            after_n.revision.0
        );
        println!(
            "tick N+1 : drained 1 request; ran pinned program (1 proposal); backlog=0; commits={}; Moment committed at tick {} revision {}",
            out_n1.commits.len(),
            commit_tick.0,
            readback.revision.0
        );
        println!("causal   : committed Moment ancestry = {ancestry:?}");
        println!("=> a construct self-woke from a physics firing and committed a Moment IN advance(), two ticks later (latency exposed, not hidden).");
    }
}
