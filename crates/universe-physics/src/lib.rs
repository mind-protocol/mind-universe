//! Deterministic, bounded physical residency for Universe entities.

use rapier3d::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use universe_core::{
    EntityKey, Epistemic, HandleKind, PackedHandle, RelationKey, Revision, Tick, UniverseError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Residency {
    Dormant,
    Hot,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhysicalState {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
}

impl PhysicalState {
    fn validate(self) -> Result<(), UniverseError> {
        if self
            .position
            .into_iter()
            .chain(self.velocity)
            .all(f32::is_finite)
        {
            Ok(())
        } else {
            Err(UniverseError::Validation(
                "physical state contains NaN or infinity".into(),
            ))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PhysicsCommand {
    Materialize {
        entity: EntityKey,
        generation: u32,
        state: PhysicalState,
    },
    Release {
        entity: EntityKey,
    },
    Step,
}

impl PhysicsCommand {
    fn sort_key(&self) -> (u8, EntityKey) {
        match self {
            Self::Materialize { entity, .. } => (0, *entity),
            Self::Release { entity } => (1, *entity),
            Self::Step => (2, EntityKey(0)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PhysicsEvent {
    Materialized {
        entity: EntityKey,
    },
    Released {
        entity: EntityKey,
        state: PhysicalState,
    },
    Stepped {
        tick: Tick,
        active_bodies: usize,
    },
    Rejected {
        entity: Option<EntityKey>,
        reason: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhysicsDelta {
    pub tick: Tick,
    pub events: Vec<PhysicsEvent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsBudget {
    pub max_active_bodies: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedRelationPhysicalBinding {
    SpringJoint {
        rest_length: f32,
        stiffness: f32,
        damping: f32,
        contacts_enabled: bool,
        wake_up: bool,
    },
    NoSolverObject,
}

impl ResolvedRelationPhysicalBinding {
    fn validate(&self) -> Result<(), UniverseError> {
        match self {
            Self::SpringJoint {
                rest_length,
                stiffness,
                damping,
                ..
            } if !rest_length.is_finite()
                || !stiffness.is_finite()
                || !damping.is_finite()
                || *rest_length <= 0.0
                || *stiffness <= 0.0
                || *damping < 0.0 =>
            {
                Err(UniverseError::Validation(
                    "SpringJoint profile must contain finite positive length/stiffness and non-negative damping"
                        .into(),
                ))
            }
            _ => Ok(()),
        }
    }

    fn creates_joint(&self) -> bool {
        matches!(self, Self::SpringJoint { .. })
    }

    fn wake_cost(&self) -> u32 {
        match self {
            Self::SpringJoint { wake_up: true, .. } => 2,
            Self::SpringJoint { wake_up: false, .. } | Self::NoSolverObject => 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationBindingProvenance {
    pub universe_revision: Revision,
    pub mapping_revision: Revision,
    pub profile: EntityKey,
    pub profile_hash: String,
    pub source_event: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationPhysicalAction {
    Add,
    Replace,
    Tombstone,
    Release,
}

/// A graph-resolved relation mutation. Predicate identity is provenance only;
/// `binding` already contains the primitive selected by graph authority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelationPhysicalDelta {
    pub idempotency_key: String,
    pub relation: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub semantic_predicate: EntityKey,
    pub action: RelationPhysicalAction,
    pub binding: Option<ResolvedRelationPhysicalBinding>,
    pub provenance: RelationBindingProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelationPhysicsCommand {
    pub idempotency_key: String,
    pub relation: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub semantic_predicate: EntityKey,
    pub action: RelationPhysicalAction,
    pub binding: Option<ResolvedRelationPhysicalBinding>,
    pub provenance: RelationBindingProvenance,
}

pub fn map_relation_physical_delta(
    delta: RelationPhysicalDelta,
) -> Result<RelationPhysicsCommand, UniverseError> {
    let command = RelationPhysicsCommand {
        idempotency_key: delta.idempotency_key,
        relation: delta.relation,
        source: delta.source,
        target: delta.target,
        semantic_predicate: delta.semantic_predicate,
        action: delta.action,
        binding: delta.binding,
        provenance: delta.provenance,
    };
    validate_relation_command(&command)?;
    Ok(command)
}

fn validate_relation_command(command: &RelationPhysicsCommand) -> Result<(), UniverseError> {
    if command.idempotency_key.trim().is_empty() {
        return Err(UniverseError::Validation(
            "relation physics idempotency key must not be empty".into(),
        ));
    }
    if command.provenance.profile_hash.trim().is_empty()
        || command.provenance.source_event.trim().is_empty()
    {
        return Err(UniverseError::Validation(
            "relation physics provenance must include profile hash and source event".into(),
        ));
    }
    match command.action {
        RelationPhysicalAction::Add | RelationPhysicalAction::Replace => {
            let binding = command.binding.as_ref().ok_or_else(|| {
                UniverseError::Validation("add/replace requires a resolved physical binding".into())
            })?;
            binding.validate()?;
            if binding.creates_joint() && command.source == command.target {
                return Err(UniverseError::Validation(
                    "solver relation binding requires distinct endpoints".into(),
                ));
            }
        }
        RelationPhysicalAction::Tombstone | RelationPhysicalAction::Release => {
            if command.binding.is_some() {
                return Err(UniverseError::Validation(
                    "tombstone/release must not carry a replacement binding".into(),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationPhysicsBudget {
    pub max_commands: u32,
    pub max_active_bindings: u32,
    pub max_active_joints: u32,
    pub max_wake_cost: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationBindingStatus {
    Active,
    Released,
    Tombstoned,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelationBindingReadback {
    pub relation: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub semantic_predicate: EntityKey,
    pub status: RelationBindingStatus,
    pub binding: ResolvedRelationPhysicalBinding,
    pub provenance: RelationBindingProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationCommandOutcome {
    Added,
    Replaced,
    Tombstoned,
    Released,
    AlreadyApplied,
    Unchanged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationCommandEvidence {
    pub relation: RelationKey,
    pub idempotency_key: String,
    pub outcome: RelationCommandOutcome,
    pub provenance: RelationBindingProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationBatchStatus {
    Applied,
    Rejected,
    RolledBack,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationRollbackEvidence {
    pub attempted: bool,
    pub scope: Vec<RelationKey>,
    pub restored: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationPhysicsReceipt {
    pub tick: Tick,
    pub status: RelationBatchStatus,
    pub commands: Vec<RelationCommandEvidence>,
    pub active_bindings_before: u32,
    pub active_bindings_after: u32,
    pub active_joints_before: u32,
    pub active_joints_after: u32,
    pub wake_cost: u32,
    pub rollback: RelationRollbackEvidence,
    pub error: Option<String>,
}

/// A homogeneous unit interpreted physically as an energy gate and logically
/// as a proposition. The native law knows no ontology-specific predicate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomSpec {
    pub key: EntityKey,
    pub threshold: u64,
    #[serde(default)]
    pub seed_energy: u64,
    #[serde(default)]
    pub required_supports: Vec<RelationKey>,
    #[serde(default)]
    pub inhibition_threshold: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BondPolarity {
    Support,
    Inhibit,
    Neutral,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomBond {
    pub key: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub polarity: BondPolarity,
    pub energy: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomState {
    pub support_energy: u64,
    pub inhibition_energy: u64,
    pub received_supports: BTreeSet<RelationKey>,
    pub fired_at: Option<Tick>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomInjection {
    pub atom: EntityKey,
    pub energy: u64,
    pub at_tick: Tick,
    pub provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomTransfer {
    pub bond: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub polarity: BondPolarity,
    pub energy: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomStep {
    pub tick: Tick,
    pub fired: Vec<EntityKey>,
    pub starved: Vec<EntityKey>,
    pub transfers: Vec<AtomTransfer>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomRun {
    pub start_tick: Tick,
    pub end_tick: Tick,
    pub steps: Vec<AtomStep>,
    pub terminal_starved: Vec<EntityKey>,
    pub quiescent: bool,
    pub budget_exhausted: bool,
    pub initial_energy: u64,
    pub injected_energy: u64,
    pub stored_energy: u64,
    pub energy_conserved: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomExecutionBudget {
    pub max_atoms: u32,
    pub max_bonds: u32,
    pub max_steps: u32,
    pub max_total_energy: u64,
}

impl AtomExecutionBudget {
    fn validate(self) -> Result<(), UniverseError> {
        if self.max_atoms == 0
            || self.max_bonds == 0
            || self.max_steps == 0
            || self.max_total_energy == 0
        {
            return Err(UniverseError::Validation(
                "Atom execution budgets must be positive".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomInjectionRequest {
    pub atom: EntityKey,
    pub energy: u64,
    pub provenance: String,
}

/// A complete, already-local physical working set.
///
/// Selecting this cluster is the responsibility of a bounded local query. The
/// physics host never expands it or consults ontology names.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalAtomCluster {
    pub atoms: Vec<AtomSpec>,
    pub bonds: Vec<AtomBond>,
    #[serde(default)]
    pub injections: Vec<AtomInjectionRequest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomConvergence {
    Quiescent,
    StepBudgetExhausted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomEnergyLedger {
    pub initial: u64,
    pub injected: u64,
    pub stored: u64,
    pub allowed: u64,
    pub conserved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomContainmentEvidence {
    pub atom_count: u32,
    pub bond_count: u32,
    pub executed_steps: u32,
    pub max_atoms: u32,
    pub max_bonds: u32,
    pub max_steps: u32,
    pub within_budget: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomReleaseEvidence {
    /// The event-driven solver is scoped to this call and no mutable runtime
    /// state escapes after the receipt is constructed.
    pub ephemeral_state_released: bool,
    pub retained_runtime_atoms: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomLifetimeEvidence {
    pub ticks_used: u64,
    pub tick_limit: u64,
    pub within_limit: bool,
}

/// Measured physical evidence. It deliberately makes no semantic-health claim.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomExecutionReceipt {
    pub atoms: Vec<EntityKey>,
    pub bonds: Vec<RelationKey>,
    pub convergence: AtomConvergence,
    pub terminal_starved: Vec<EntityKey>,
    pub energy: AtomEnergyLedger,
    pub containment: AtomContainmentEvidence,
    pub lifetime: AtomLifetimeEvidence,
    pub release: AtomReleaseEvidence,
    pub run: AtomRun,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomPhysicalHealthCheck {
    Pass,
    Fail,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomPhysicalEnvelopeState {
    WithinEnvelope,
    OutsideEnvelope,
}

/// Independent epistemic classification of physical execution evidence.
///
/// This record speaks only about solver convergence, energy accounting,
/// runtime containment, lifetime, and release. It is never semantic truth,
/// justification validity, objective achievement, or Loop health.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomPhysicalHealthEvidence {
    pub convergence: Epistemic<AtomPhysicalHealthCheck>,
    pub energy_conservation: Epistemic<AtomPhysicalHealthCheck>,
    pub containment: Epistemic<AtomPhysicalHealthCheck>,
    pub lifetime: Epistemic<AtomPhysicalHealthCheck>,
    pub release: Epistemic<AtomPhysicalHealthCheck>,
    pub envelope: Epistemic<AtomPhysicalEnvelopeState>,
}

pub enum AtomPhysicalHealthObservation<'a> {
    Executed(&'a AtomExecutionReceipt),
    Failed(&'a UniverseError),
    Unknown,
}

pub fn assess_atom_physical_health(
    observation: AtomPhysicalHealthObservation<'_>,
) -> AtomPhysicalHealthEvidence {
    match observation {
        AtomPhysicalHealthObservation::Executed(receipt) => {
            let convergence = check(receipt.convergence == AtomConvergence::Quiescent);
            let energy_conservation = check(receipt.energy.conserved);
            let containment = check(receipt.containment.within_budget);
            let lifetime = check(receipt.lifetime.within_limit);
            let release = check(
                receipt.release.ephemeral_state_released
                    && receipt.release.retained_runtime_atoms == 0,
            );
            let within_envelope = [
                &convergence,
                &energy_conservation,
                &containment,
                &lifetime,
                &release,
            ]
            .into_iter()
            .all(|state| matches!(state, Epistemic::Measured(AtomPhysicalHealthCheck::Pass)));
            AtomPhysicalHealthEvidence {
                convergence,
                energy_conservation,
                containment,
                lifetime,
                release,
                envelope: Epistemic::Measured(if within_envelope {
                    AtomPhysicalEnvelopeState::WithinEnvelope
                } else {
                    AtomPhysicalEnvelopeState::OutsideEnvelope
                }),
            }
        }
        AtomPhysicalHealthObservation::Failed(error) => {
            let reason = error.to_string();
            if matches!(error, UniverseError::BudgetExhausted(_)) {
                AtomPhysicalHealthEvidence {
                    convergence: Epistemic::NotMeasured,
                    energy_conservation: Epistemic::NotMeasured,
                    containment: Epistemic::Measured(AtomPhysicalHealthCheck::Fail),
                    lifetime: Epistemic::NotMeasured,
                    release: Epistemic::NotMeasured,
                    envelope: Epistemic::MeasurementFailed { reason },
                }
            } else {
                AtomPhysicalHealthEvidence {
                    convergence: measurement_failed(&reason),
                    energy_conservation: measurement_failed(&reason),
                    containment: measurement_failed(&reason),
                    lifetime: measurement_failed(&reason),
                    release: measurement_failed(&reason),
                    envelope: Epistemic::MeasurementFailed { reason },
                }
            }
        }
        AtomPhysicalHealthObservation::Unknown => AtomPhysicalHealthEvidence {
            convergence: Epistemic::Unknown,
            energy_conservation: Epistemic::Unknown,
            containment: Epistemic::Unknown,
            lifetime: Epistemic::Unknown,
            release: Epistemic::Unknown,
            envelope: Epistemic::Unknown,
        },
    }
}

fn check(value: bool) -> Epistemic<AtomPhysicalHealthCheck> {
    Epistemic::Measured(if value {
        AtomPhysicalHealthCheck::Pass
    } else {
        AtomPhysicalHealthCheck::Fail
    })
}

fn measurement_failed(reason: &str) -> Epistemic<AtomPhysicalHealthCheck> {
    Epistemic::MeasurementFailed {
        reason: reason.into(),
    }
}

/// Execute one bounded local cluster and release its transient solver state.
///
/// Ontology-to-runtime compilation happens before this primitive. This
/// function consumes only explicit physical atoms, bonds, injections, and
/// budgets, so no canonical predicate can influence native dispatch.
pub fn execute_local_atom_cluster(
    cluster: LocalAtomCluster,
    budget: AtomExecutionBudget,
) -> Result<AtomExecutionReceipt, UniverseError> {
    budget.validate()?;
    if cluster.atoms.is_empty() {
        return Err(UniverseError::Validation(
            "local Atom cluster must contain at least one Atom".into(),
        ));
    }
    let atom_count = u32::try_from(cluster.atoms.len())
        .map_err(|_| UniverseError::BudgetExhausted("local Atom count exceeds u32".into()))?;
    let bond_count = u32::try_from(cluster.bonds.len())
        .map_err(|_| UniverseError::BudgetExhausted("local Bond count exceeds u32".into()))?;
    if atom_count > budget.max_atoms {
        return Err(UniverseError::BudgetExhausted(
            "local Atom count exceeds execution budget".into(),
        ));
    }
    if bond_count > budget.max_bonds {
        return Err(UniverseError::BudgetExhausted(
            "local Bond count exceeds execution budget".into(),
        ));
    }

    let initial_energy = cluster.atoms.iter().try_fold(0u64, |total, atom| {
        total
            .checked_add(atom.seed_energy)
            .ok_or_else(|| UniverseError::Validation("initial Atom energy overflow".into()))
    })?;
    let injected_energy = cluster
        .injections
        .iter()
        .try_fold(0u64, |total, injection| {
            total
                .checked_add(injection.energy)
                .ok_or_else(|| UniverseError::Validation("injected Atom energy overflow".into()))
        })?;
    let total_energy = initial_energy
        .checked_add(injected_energy)
        .ok_or_else(|| UniverseError::Validation("Atom execution energy overflow".into()))?;
    if total_energy > budget.max_total_energy {
        return Err(UniverseError::BudgetExhausted(
            "local Atom energy exceeds execution budget".into(),
        ));
    }

    let mut atom_keys: Vec<_> = cluster.atoms.iter().map(|atom| atom.key).collect();
    atom_keys.sort();
    let mut bond_keys: Vec<_> = cluster.bonds.iter().map(|bond| bond.key).collect();
    bond_keys.sort();
    let mut dynamics = AtomDynamics::new(cluster.atoms, cluster.bonds)?;
    for injection in cluster.injections {
        dynamics.inject(injection.atom, injection.energy, injection.provenance)?;
    }
    let max_steps = usize::try_from(budget.max_steps)
        .map_err(|_| UniverseError::Validation("max_steps does not fit usize".into()))?;
    let run = dynamics.run_until_quiescent(max_steps)?;
    let convergence = if run.quiescent {
        AtomConvergence::Quiescent
    } else {
        AtomConvergence::StepBudgetExhausted
    };
    let containment = AtomContainmentEvidence {
        atom_count,
        bond_count,
        executed_steps: u32::try_from(run.steps.len())
            .map_err(|_| UniverseError::Validation("executed step count exceeds u32".into()))?,
        max_atoms: budget.max_atoms,
        max_bonds: budget.max_bonds,
        max_steps: budget.max_steps,
        within_budget: atom_count <= budget.max_atoms
            && bond_count <= budget.max_bonds
            && run.steps.len() <= max_steps
            && run.stored_energy <= budget.max_total_energy,
    };
    let terminal_starved = run.terminal_starved.clone();
    let energy = AtomEnergyLedger {
        initial: run.initial_energy,
        injected: run.injected_energy,
        stored: run.stored_energy,
        allowed: budget.max_total_energy,
        conserved: run.energy_conserved,
    };
    let ticks_used = run.end_tick.0.saturating_sub(run.start_tick.0);
    let tick_limit = u64::from(budget.max_steps);
    Ok(AtomExecutionReceipt {
        atoms: atom_keys,
        bonds: bond_keys,
        convergence,
        terminal_starved,
        energy,
        containment,
        lifetime: AtomLifetimeEvidence {
            ticks_used,
            tick_limit,
            within_limit: ticks_used <= tick_limit,
        },
        release: AtomReleaseEvidence {
            ephemeral_state_released: true,
            retained_runtime_atoms: 0,
        },
        run,
    })
}

/// Deterministic, event-driven Atom dynamics.
///
/// Each bond can conduct once, when its source first fires. Support is an AND
/// gate when `required_supports` names exact incoming bonds. Inhibition blocks
/// firing without erasing support. Neutral bonds preserve graph semantics but
/// transport no energy. All energy uses integers and is accounted explicitly.
#[derive(Clone, Debug)]
pub struct AtomDynamics {
    atoms: BTreeMap<EntityKey, AtomSpec>,
    bonds: BTreeMap<RelationKey, AtomBond>,
    outgoing: BTreeMap<EntityKey, Vec<RelationKey>>,
    states: BTreeMap<EntityKey, AtomState>,
    injections: Vec<AtomInjection>,
    initial_energy: u64,
    tick: Tick,
}

impl AtomDynamics {
    pub fn new(atoms: Vec<AtomSpec>, bonds: Vec<AtomBond>) -> Result<Self, UniverseError> {
        let atom_count = atoms.len();
        let atoms: BTreeMap<_, _> = atoms.into_iter().map(|atom| (atom.key, atom)).collect();
        if atoms.len() != atom_count {
            return Err(UniverseError::Validation("duplicate Atom key".into()));
        }
        if atoms.values().any(|atom| {
            atom.threshold == 0
                || atom.inhibition_threshold == Some(0)
                || atom.required_supports.iter().collect::<BTreeSet<_>>().len()
                    != atom.required_supports.len()
        }) {
            return Err(UniverseError::Validation(
                "Atom thresholds must be positive and required supports unique".into(),
            ));
        }

        let bond_count = bonds.len();
        let bonds: BTreeMap<_, _> = bonds.into_iter().map(|bond| (bond.key, bond)).collect();
        if bonds.len() != bond_count {
            return Err(UniverseError::Validation("duplicate Bond key".into()));
        }
        for bond in bonds.values() {
            if !atoms.contains_key(&bond.source) || !atoms.contains_key(&bond.target) {
                return Err(UniverseError::Validation(
                    "Bond endpoint does not name an Atom".into(),
                ));
            }
            match bond.polarity {
                BondPolarity::Neutral if bond.energy != 0 => {
                    return Err(UniverseError::Validation(
                        "neutral Bond must transport zero energy".into(),
                    ));
                }
                BondPolarity::Support | BondPolarity::Inhibit if bond.energy == 0 => {
                    return Err(UniverseError::Validation(
                        "supporting or inhibiting Bond must transport energy".into(),
                    ));
                }
                _ => {}
            }
        }
        for atom in atoms.values() {
            for required in &atom.required_supports {
                let bond = bonds.get(required).ok_or_else(|| {
                    UniverseError::Validation("required support Bond does not exist".into())
                })?;
                if bond.target != atom.key || bond.polarity != BondPolarity::Support {
                    return Err(UniverseError::Validation(
                        "required support must be an incoming supporting Bond".into(),
                    ));
                }
            }
        }

        let mut outgoing: BTreeMap<EntityKey, Vec<RelationKey>> = BTreeMap::new();
        for bond in bonds.values() {
            outgoing.entry(bond.source).or_default().push(bond.key);
        }
        for keys in outgoing.values_mut() {
            keys.sort();
        }
        let initial_energy = atoms.values().try_fold(0u64, |total, atom| {
            total
                .checked_add(atom.seed_energy)
                .ok_or_else(|| UniverseError::Validation("initial Atom energy overflow".into()))
        })?;
        let states = atoms
            .values()
            .map(|atom| {
                (
                    atom.key,
                    AtomState {
                        support_energy: atom.seed_energy,
                        inhibition_energy: 0,
                        received_supports: BTreeSet::new(),
                        fired_at: None,
                    },
                )
            })
            .collect();
        Ok(Self {
            atoms,
            bonds,
            outgoing,
            states,
            injections: Vec::new(),
            initial_energy,
            tick: Tick(0),
        })
    }

    pub fn inject(
        &mut self,
        atom: EntityKey,
        energy: u64,
        provenance: impl Into<String>,
    ) -> Result<AtomInjection, UniverseError> {
        if energy == 0 {
            return Err(UniverseError::Validation(
                "Atom injection must carry positive energy".into(),
            ));
        }
        let state = self
            .states
            .get_mut(&atom)
            .ok_or_else(|| UniverseError::Validation("injection Atom does not exist".into()))?;
        state.support_energy = state
            .support_energy
            .checked_add(energy)
            .ok_or_else(|| UniverseError::Validation("Atom injection overflow".into()))?;
        let injection = AtomInjection {
            atom,
            energy,
            at_tick: self.tick,
            provenance: provenance.into(),
        };
        self.injections.push(injection.clone());
        Ok(injection)
    }

    pub fn run_until_quiescent(&mut self, max_steps: usize) -> Result<AtomRun, UniverseError> {
        if max_steps == 0 {
            return Err(UniverseError::Validation(
                "Atom run budget must be positive".into(),
            ));
        }
        let start_tick = self.tick;
        let mut steps = Vec::new();
        let mut quiescent = false;
        let mut terminal_starved = Vec::new();
        for _ in 0..max_steps {
            let step = self.step()?;
            if step.fired.is_empty() {
                terminal_starved = step.starved;
                quiescent = true;
                break;
            }
            steps.push(step);
        }
        let injected_energy = self.injections.iter().try_fold(0u64, |total, injection| {
            total
                .checked_add(injection.energy)
                .ok_or_else(|| UniverseError::Validation("injected energy overflow".into()))
        })?;
        let stored_energy = self.stored_energy()?;
        let expected = self
            .initial_energy
            .checked_add(injected_energy)
            .ok_or_else(|| UniverseError::Validation("energy ledger overflow".into()))?;
        Ok(AtomRun {
            start_tick,
            end_tick: self.tick,
            steps,
            terminal_starved,
            quiescent,
            budget_exhausted: !quiescent,
            initial_energy: self.initial_energy,
            injected_energy,
            stored_energy,
            energy_conserved: expected == stored_energy,
        })
    }

    fn step(&mut self) -> Result<AtomStep, UniverseError> {
        let mut fired = Vec::new();
        let mut starved = Vec::new();
        for (key, atom) in &self.atoms {
            let state = &self.states[key];
            if state.fired_at.is_some()
                || state.support_energy < atom.threshold
                || !atom
                    .required_supports
                    .iter()
                    .all(|support| state.received_supports.contains(support))
                || atom
                    .inhibition_threshold
                    .is_some_and(|threshold| state.inhibition_energy >= threshold)
            {
                continue;
            }
            let output_energy = self.outgoing.get(key).into_iter().flatten().try_fold(
                0u64,
                |total, relation| {
                    total
                        .checked_add(self.bonds[relation].energy)
                        .ok_or_else(|| {
                            UniverseError::Validation("outgoing Bond energy overflow".into())
                        })
                },
            )?;
            if output_energy > state.support_energy {
                starved.push(*key);
            } else {
                fired.push(*key);
            }
        }
        if fired.is_empty() {
            return Ok(AtomStep {
                tick: self.tick,
                fired,
                starved,
                transfers: Vec::new(),
            });
        }

        self.tick.0 = self
            .tick
            .0
            .checked_add(1)
            .ok_or_else(|| UniverseError::Validation("Atom tick overflow".into()))?;
        for key in &fired {
            self.states.get_mut(key).unwrap().fired_at = Some(self.tick);
        }

        let mut transfers = Vec::new();
        for source in &fired {
            for relation in self.outgoing.get(source).into_iter().flatten() {
                let bond = &self.bonds[relation];
                if bond.energy > 0 {
                    let source_state = self.states.get_mut(source).unwrap();
                    source_state.support_energy -= bond.energy;
                }
                match bond.polarity {
                    BondPolarity::Support => {
                        let target = self.states.get_mut(&bond.target).unwrap();
                        target.support_energy = target
                            .support_energy
                            .checked_add(bond.energy)
                            .ok_or_else(|| {
                                UniverseError::Validation("Atom support overflow".into())
                            })?;
                        target.received_supports.insert(bond.key);
                    }
                    BondPolarity::Inhibit => {
                        let target = self.states.get_mut(&bond.target).unwrap();
                        target.inhibition_energy = target
                            .inhibition_energy
                            .checked_add(bond.energy)
                            .ok_or_else(|| {
                                UniverseError::Validation("Atom inhibition overflow".into())
                            })?;
                    }
                    BondPolarity::Neutral => {}
                }
                transfers.push(AtomTransfer {
                    bond: bond.key,
                    source: bond.source,
                    target: bond.target,
                    polarity: bond.polarity,
                    energy: bond.energy,
                });
            }
        }
        Ok(AtomStep {
            tick: self.tick,
            fired,
            starved,
            transfers,
        })
    }

    fn stored_energy(&self) -> Result<u64, UniverseError> {
        self.states.values().try_fold(0u64, |total, state| {
            total
                .checked_add(state.support_energy)
                .and_then(|value| value.checked_add(state.inhibition_energy))
                .ok_or_else(|| UniverseError::Validation("stored Atom energy overflow".into()))
        })
    }

    pub fn state(&self, atom: EntityKey) -> Option<&AtomState> {
        self.states.get(&atom)
    }

    pub fn fired(&self, atom: EntityKey) -> bool {
        self.states
            .get(&atom)
            .is_some_and(|state| state.fired_at.is_some())
    }

    pub fn tick(&self) -> Tick {
        self.tick
    }
}

// ---------------------------------------------------------------------------
// The physics-event -> Atom-deposit bridge.
//
// This is the single load-bearing seam CLAUDE.md names: "physics-event -> energy
// deposit onto a construct's trigger atom -- the *same* bridge that injects an
// L3 stimulus into an L1 field. Build it once; it powers both the house alarm
// and perception." It is pure trusted-computing-base plumbing and carries zero
// variable policy: the trigger identity, the target Atom, and the weight are all
// graph authority, resolved *before* the bridge runs.
// ---------------------------------------------------------------------------

/// A declared, data-driven binding from a significant physics event to an energy
/// deposit on a construct's trigger Atom — one edge of the `DepositBond` pattern
/// (`event -> +energy on a trigger atom`).
///
/// The native floor gives the event no ontology: it is identified only by a
/// stable [`EntityKey`] handle. What that handle *means* — a Rapier sensor
/// collider's intersection-enter, an Atom's threshold crossing — is graph data,
/// resolved elsewhere. The bridge can only turn "event `trigger` occurred" plus
/// "deposit `weight` onto `target`" into a runtime deposit request. It commits
/// nothing and consults no predicate name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhysicsEventDeposit {
    /// The event-source handle this binding listens for. Either a sensor
    /// collider entity (an intersection-enter) or an Atom (a threshold
    /// crossing) — the native floor does not distinguish them.
    pub trigger: EntityKey,
    /// The downstream trigger Atom that receives the deposit.
    pub target: EntityKey,
    /// Energy deposited onto the target's support when the event occurs. Must be
    /// positive; the weight is graph authority, never a native default.
    pub weight: u64,
}

/// The set of Atoms that crossed their firing threshold during a cluster run.
///
/// Each is a significant physics event — a threshold crossing — that a
/// [`PhysicsEventDeposit`] binding may route onward onto a downstream trigger
/// Atom. Reads measured evidence only; it invents nothing.
pub fn fired_atoms(receipt: &AtomExecutionReceipt) -> BTreeSet<EntityKey> {
    receipt
        .run
        .steps
        .iter()
        .flat_map(|step| step.fired.iter().copied())
        .collect()
}

/// Resolve declared bindings against the physics events observed this wave,
/// producing runtime deposits ([`AtomInjectionRequest`]) onto downstream trigger
/// Atoms.
///
/// Deterministic and bounded: output order follows `bindings`, one request per
/// binding whose `trigger` is present in `events`. It allocates only — it never
/// mutates a store, a snapshot, or the Atom field. Turning a request into an
/// actual `+energy` deposit is the caller's next step, via the existing
/// `AtomInjectionRequest`/[`AtomDynamics::inject`] primitive (see
/// [`deposit_onto_dynamics`] or [`LocalAtomCluster::injections`]).
pub fn resolve_physics_event_deposits(
    bindings: &[PhysicsEventDeposit],
    events: &BTreeSet<EntityKey>,
) -> Result<Vec<AtomInjectionRequest>, UniverseError> {
    let mut deposits = Vec::new();
    for binding in bindings {
        if binding.weight == 0 {
            return Err(UniverseError::Validation(
                "physics-event deposit weight must be positive".into(),
            ));
        }
        if events.contains(&binding.trigger) {
            deposits.push(AtomInjectionRequest {
                atom: binding.target,
                energy: binding.weight,
                provenance: format!(
                    "physics-event:{}->atom:{}",
                    binding.trigger, binding.target
                ),
            });
        }
    }
    Ok(deposits)
}

/// Land resolved deposits onto a live [`AtomDynamics`] field BEFORE its next
/// step wave, so a construct that was dormant now self-wakes when the deposit
/// crosses its threshold.
///
/// This is the runtime landing of the bridge: `+weight` lands on the in-memory
/// Atom support field ONLY. A [`PhysicsEvent`] never mutates the committed
/// store; the deposit is transient runtime state, released with the solver.
pub fn deposit_onto_dynamics(
    dynamics: &mut AtomDynamics,
    deposits: &[AtomInjectionRequest],
) -> Result<Vec<AtomInjection>, UniverseError> {
    let mut applied = Vec::with_capacity(deposits.len());
    for deposit in deposits {
        applied.push(dynamics.inject(deposit.atom, deposit.energy, deposit.provenance.clone())?);
    }
    Ok(applied)
}

/// A cuboid sensor/probe must have finite, strictly positive half-extents.
fn validate_half_extents(half_extents: [f32; 3]) -> Result<(), UniverseError> {
    if half_extents.iter().all(|value| value.is_finite() && *value > 0.0) {
        Ok(())
    } else {
        Err(UniverseError::Validation(
            "collider half-extents must be finite and positive".into(),
        ))
    }
}

fn readback_from_command(
    command: &RelationPhysicsCommand,
    status: RelationBindingStatus,
) -> RelationBindingReadback {
    RelationBindingReadback {
        relation: command.relation,
        source: command.source,
        target: command.target,
        semantic_predicate: command.semantic_predicate,
        status,
        binding: command
            .binding
            .clone()
            .expect("validated add/replace command has a binding"),
        provenance: command.provenance.clone(),
    }
}

fn binding_counts(readback: &RelationBindingReadback) -> (u32, u32) {
    if readback.status == RelationBindingStatus::Active {
        (1, u32::from(readback.binding.creates_joint()))
    } else {
        (0, 0)
    }
}

fn remove_relation_adjacency(
    adjacency: &mut BTreeMap<EntityKey, BTreeSet<RelationKey>>,
    entity: EntityKey,
    relation: RelationKey,
) {
    let remove_entry = adjacency.get_mut(&entity).is_some_and(|relations| {
        relations.remove(&relation);
        relations.is_empty()
    });
    if remove_entry {
        adjacency.remove(&entity);
    }
}

#[derive(Clone, Copy, Debug)]
struct BodyBinding {
    handle: RigidBodyHandle,
    generation: u32,
}

#[derive(Clone, Debug)]
struct RelationBinding {
    readback: RelationBindingReadback,
    joint: Option<ImpulseJointHandle>,
}

/// Rapier remains a bounded numerical projection; entity keys remain authoritative.
pub struct UniversePhysics {
    pipeline: PhysicsPipeline,
    gravity: Vector<Real>,
    integration: IntegrationParameters,
    islands: IslandManager,
    broad_phase: BroadPhaseMultiSap,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd: CCDSolver,
    bindings: BTreeMap<EntityKey, BodyBinding>,
    dormant: BTreeMap<EntityKey, PhysicalState>,
    /// Sensor collider handle (as its raw `(index, generation)` parts, since
    /// `ColliderHandle` is not `Ord`) -> the stable [`EntityKey`] it belongs to.
    /// When the solver reports an intersection-enter for one of these colliders,
    /// the entity is surfaced into the observed-event set. This map is the ONLY
    /// ontology-free channel from Rapier geometry back to a Universe handle.
    sensor_colliders: BTreeMap<(u32, u32), EntityKey>,
    relation_bindings: BTreeMap<RelationKey, RelationBinding>,
    active_relation_adjacency: BTreeMap<EntityKey, BTreeSet<RelationKey>>,
    applied_relation_commands: BTreeMap<String, RelationPhysicsCommand>,
    active_relation_binding_count: u32,
    active_relation_joint_count: u32,
    tick: Tick,
    budget: PhysicsBudget,
}

impl UniversePhysics {
    pub fn new(fixed_dt: f32, budget: PhysicsBudget) -> Result<Self, UniverseError> {
        if !fixed_dt.is_finite() || fixed_dt <= 0.0 || budget.max_active_bodies == 0 {
            return Err(UniverseError::Validation(
                "fixed_dt and active-body budget must be positive".into(),
            ));
        }
        let integration = IntegrationParameters {
            dt: fixed_dt,
            ..IntegrationParameters::default()
        };
        Ok(Self {
            pipeline: PhysicsPipeline::new(),
            gravity: vector![0.0, 0.0, 0.0],
            integration,
            islands: IslandManager::new(),
            broad_phase: BroadPhaseMultiSap::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd: CCDSolver::new(),
            bindings: BTreeMap::new(),
            dormant: BTreeMap::new(),
            sensor_colliders: BTreeMap::new(),
            relation_bindings: BTreeMap::new(),
            active_relation_adjacency: BTreeMap::new(),
            applied_relation_commands: BTreeMap::new(),
            active_relation_binding_count: 0,
            active_relation_joint_count: 0,
            tick: Tick(0),
            budget,
        })
    }

    pub fn apply(&mut self, mut commands: Vec<PhysicsCommand>) -> PhysicsDelta {
        commands.sort_by_key(PhysicsCommand::sort_key);
        let mut events = Vec::new();
        for command in commands {
            match command {
                PhysicsCommand::Materialize {
                    entity,
                    generation,
                    state,
                } => match self.materialize(entity, generation, state) {
                    Ok(true) => events.push(PhysicsEvent::Materialized { entity }),
                    Ok(false) => {}
                    Err(error) => events.push(PhysicsEvent::Rejected {
                        entity: Some(entity),
                        reason: error.to_string(),
                    }),
                },
                PhysicsCommand::Release { entity } => match self.release(entity) {
                    Ok(Some(state)) => events.push(PhysicsEvent::Released { entity, state }),
                    Ok(None) => {}
                    Err(error) => events.push(PhysicsEvent::Rejected {
                        entity: Some(entity),
                        reason: error.to_string(),
                    }),
                },
                PhysicsCommand::Step => {
                    self.pipeline.step(
                        &self.gravity,
                        &self.integration,
                        &mut self.islands,
                        &mut self.broad_phase,
                        &mut self.narrow_phase,
                        &mut self.bodies,
                        &mut self.colliders,
                        &mut self.impulse_joints,
                        &mut self.multibody_joints,
                        &mut self.ccd,
                        None,
                        &(),
                        &(),
                    );
                    self.tick.0 += 1;
                    events.push(PhysicsEvent::Stepped {
                        tick: self.tick,
                        active_bodies: self.bindings.len(),
                    });
                }
            }
        }
        PhysicsDelta {
            tick: self.tick,
            events,
        }
    }

    /// Applies graph-resolved relation commands atomically at the current
    /// physical tick boundary. Every command is validated and budgeted before
    /// Rapier or binding state is mutated.
    pub fn apply_relation_commands_at_tick(
        &mut self,
        at_tick: Tick,
        mut commands: Vec<RelationPhysicsCommand>,
        budget: RelationPhysicsBudget,
    ) -> RelationPhysicsReceipt {
        let before_bindings = self.active_relation_binding_count;
        let before_joints = self.active_relation_joint_count;
        let reject = |reason: String, scope: Vec<RelationKey>| RelationPhysicsReceipt {
            tick: at_tick,
            status: RelationBatchStatus::Rejected,
            commands: Vec::new(),
            active_bindings_before: before_bindings,
            active_bindings_after: before_bindings,
            active_joints_before: before_joints,
            active_joints_after: before_joints,
            wake_cost: 0,
            rollback: RelationRollbackEvidence {
                attempted: false,
                scope,
                restored: true,
            },
            error: Some(reason),
        };

        if at_tick != self.tick {
            return reject(
                format!(
                    "relation commands target tick {:?}, current physical tick is {:?}",
                    at_tick, self.tick
                ),
                Vec::new(),
            );
        }
        if budget.max_commands == 0 || budget.max_active_bindings == 0 {
            return reject(
                "relation command and active-binding budgets must be positive".into(),
                Vec::new(),
            );
        }
        let Ok(command_count) = u32::try_from(commands.len()) else {
            return reject("relation command count exceeds u32".into(), Vec::new());
        };
        if command_count > budget.max_commands {
            return reject(
                "relation command count exceeds batch budget".into(),
                Vec::new(),
            );
        }
        commands.sort_by(|left, right| {
            left.relation
                .cmp(&right.relation)
                .then_with(|| left.idempotency_key.cmp(&right.idempotency_key))
        });
        let scope = commands
            .iter()
            .map(|command| command.relation)
            .collect::<Vec<_>>();
        if scope.windows(2).any(|pair| pair[0] == pair[1]) {
            return reject(
                "one relation may appear only once per physical batch".into(),
                scope,
            );
        }
        let mut batch_idempotency = BTreeSet::new();
        if commands
            .iter()
            .any(|command| !batch_idempotency.insert(command.idempotency_key.as_str()))
        {
            return reject(
                "relation physics idempotency key is duplicated in the batch".into(),
                scope,
            );
        }

        let mut planned_bindings = before_bindings;
        let mut planned_joints = before_joints;
        let mut wake_cost = 0u32;
        let mut planned = Vec::with_capacity(commands.len());
        for command in commands {
            if let Err(error) = validate_relation_command(&command) {
                return reject(error.to_string(), scope);
            }
            if let Some(previous) = self.applied_relation_commands.get(&command.idempotency_key) {
                if previous == &command {
                    planned.push((command, RelationCommandOutcome::AlreadyApplied, true));
                    continue;
                }
                return reject(
                    "relation physics idempotency key collides with different command".into(),
                    scope,
                );
            }

            let current = self
                .relation_bindings
                .get(&command.relation)
                .map(|binding| &binding.readback);
            let (outcome, next) = match command.action {
                RelationPhysicalAction::Add => {
                    if current.is_some() {
                        return reject(
                            "add relation binding requires an unused RelationKey".into(),
                            scope,
                        );
                    }
                    (
                        RelationCommandOutcome::Added,
                        Some(readback_from_command(
                            &command,
                            RelationBindingStatus::Active,
                        )),
                    )
                }
                RelationPhysicalAction::Replace => {
                    let Some(existing) = current else {
                        return reject(
                            "replace relation binding requires an existing binding".into(),
                            scope,
                        );
                    };
                    if existing.status == RelationBindingStatus::Tombstoned {
                        return reject(
                            "tombstoned RelationKey cannot be reused by replace".into(),
                            scope,
                        );
                    }
                    if existing.source != command.source || existing.target != command.target {
                        return reject(
                            "replace relation binding cannot change stable endpoints".into(),
                            scope,
                        );
                    }
                    (
                        RelationCommandOutcome::Replaced,
                        Some(readback_from_command(
                            &command,
                            RelationBindingStatus::Active,
                        )),
                    )
                }
                RelationPhysicalAction::Tombstone => {
                    let Some(existing) = current else {
                        return reject(
                            "tombstone requires an existing relation binding".into(),
                            scope,
                        );
                    };
                    if existing.source != command.source || existing.target != command.target {
                        return reject(
                            "tombstone relation endpoints do not match active binding".into(),
                            scope,
                        );
                    }
                    if existing.status == RelationBindingStatus::Tombstoned {
                        (RelationCommandOutcome::Unchanged, Some(existing.clone()))
                    } else {
                        let mut next = existing.clone();
                        next.status = RelationBindingStatus::Tombstoned;
                        next.provenance = command.provenance.clone();
                        (RelationCommandOutcome::Tombstoned, Some(next))
                    }
                }
                RelationPhysicalAction::Release => {
                    let Some(existing) = current else {
                        return reject(
                            "release requires an existing relation binding".into(),
                            scope,
                        );
                    };
                    if existing.source != command.source || existing.target != command.target {
                        return reject(
                            "release relation endpoints do not match active binding".into(),
                            scope,
                        );
                    }
                    match existing.status {
                        RelationBindingStatus::Active => {
                            let mut next = existing.clone();
                            next.status = RelationBindingStatus::Released;
                            next.provenance = command.provenance.clone();
                            (RelationCommandOutcome::Released, Some(next))
                        }
                        RelationBindingStatus::Released => {
                            (RelationCommandOutcome::Unchanged, Some(existing.clone()))
                        }
                        RelationBindingStatus::Tombstoned => {
                            return reject(
                                "tombstoned relation binding cannot be released".into(),
                                scope,
                            );
                        }
                    }
                }
            };

            if matches!(
                command.action,
                RelationPhysicalAction::Add | RelationPhysicalAction::Replace
            ) && command
                .binding
                .as_ref()
                .is_some_and(ResolvedRelationPhysicalBinding::creates_joint)
                && (!self.bindings.contains_key(&command.source)
                    || !self.bindings.contains_key(&command.target))
            {
                return reject(
                    "relation solver binding endpoints are not physically resident".into(),
                    scope,
                );
            }

            let (current_binding, current_joint) = current.map(binding_counts).unwrap_or((0, 0));
            let (next_binding, next_joint) = next.as_ref().map(binding_counts).unwrap_or((0, 0));
            planned_bindings = planned_bindings - current_binding + next_binding;
            planned_joints = planned_joints - current_joint + next_joint;
            if matches!(
                command.action,
                RelationPhysicalAction::Add | RelationPhysicalAction::Replace
            ) {
                wake_cost = match command
                    .binding
                    .as_ref()
                    .map(ResolvedRelationPhysicalBinding::wake_cost)
                    .and_then(|cost| wake_cost.checked_add(cost))
                {
                    Some(cost) => cost,
                    None => {
                        return reject("relation wake-cost overflow".into(), scope);
                    }
                };
            }
            planned.push((command, outcome, false));
        }
        if planned_bindings > budget.max_active_bindings {
            return reject(
                "active relation binding budget would be exceeded".into(),
                scope,
            );
        }
        if planned_joints > budget.max_active_joints {
            return reject(
                "active relation joint budget would be exceeded".into(),
                scope,
            );
        }
        if wake_cost > budget.max_wake_cost {
            return reject("relation wake-cost budget would be exceeded".into(), scope);
        }

        let previous = scope
            .iter()
            .copied()
            .map(|relation| {
                (
                    relation,
                    self.relation_bindings
                        .get(&relation)
                        .map(|binding| binding.readback.clone()),
                )
            })
            .collect::<Vec<_>>();
        let mut evidence = Vec::with_capacity(planned.len());
        let mut newly_applied = Vec::new();
        for (command, outcome, already_applied) in &planned {
            if !*already_applied && *outcome != RelationCommandOutcome::Unchanged {
                if let Err(error) = self.apply_one_relation_command(command, *outcome) {
                    let restored = self.restore_relation_scope(&previous);
                    return RelationPhysicsReceipt {
                        tick: at_tick,
                        status: RelationBatchStatus::RolledBack,
                        commands: evidence,
                        active_bindings_before: before_bindings,
                        active_bindings_after: self.active_relation_binding_count,
                        active_joints_before: before_joints,
                        active_joints_after: self.active_relation_joint_count,
                        wake_cost: 0,
                        rollback: RelationRollbackEvidence {
                            attempted: true,
                            scope,
                            restored,
                        },
                        error: Some(error.to_string()),
                    };
                }
            }
            if !*already_applied {
                newly_applied.push(command.clone());
            }
            evidence.push(RelationCommandEvidence {
                relation: command.relation,
                idempotency_key: command.idempotency_key.clone(),
                outcome: *outcome,
                provenance: command.provenance.clone(),
            });
        }
        for command in newly_applied {
            self.applied_relation_commands
                .insert(command.idempotency_key.clone(), command);
        }
        RelationPhysicsReceipt {
            tick: at_tick,
            status: RelationBatchStatus::Applied,
            commands: evidence,
            active_bindings_before: before_bindings,
            active_bindings_after: self.active_relation_binding_count,
            active_joints_before: before_joints,
            active_joints_after: self.active_relation_joint_count,
            wake_cost,
            rollback: RelationRollbackEvidence {
                attempted: false,
                scope,
                restored: true,
            },
            error: None,
        }
    }

    fn apply_one_relation_command(
        &mut self,
        command: &RelationPhysicsCommand,
        outcome: RelationCommandOutcome,
    ) -> Result<(), UniverseError> {
        match outcome {
            RelationCommandOutcome::Added => self.install_relation_readback(readback_from_command(
                command,
                RelationBindingStatus::Active,
            )),
            RelationCommandOutcome::Replaced => {
                self.remove_relation_binding(command.relation, true)?;
                self.install_relation_readback(readback_from_command(
                    command,
                    RelationBindingStatus::Active,
                ))
            }
            RelationCommandOutcome::Tombstoned => {
                let mut previous = self.remove_relation_binding(command.relation, true)?;
                previous.status = RelationBindingStatus::Tombstoned;
                previous.provenance = command.provenance.clone();
                self.install_relation_readback(previous)
            }
            RelationCommandOutcome::Released => {
                let mut previous = self.remove_relation_binding(command.relation, true)?;
                previous.status = RelationBindingStatus::Released;
                previous.provenance = command.provenance.clone();
                self.install_relation_readback(previous)
            }
            RelationCommandOutcome::AlreadyApplied | RelationCommandOutcome::Unchanged => Ok(()),
        }
    }

    fn install_relation_readback(
        &mut self,
        readback: RelationBindingReadback,
    ) -> Result<(), UniverseError> {
        let joint = if readback.status == RelationBindingStatus::Active {
            self.create_relation_joint(&readback)?
        } else {
            None
        };
        if readback.status == RelationBindingStatus::Active {
            self.active_relation_binding_count = self
                .active_relation_binding_count
                .checked_add(1)
                .ok_or_else(|| UniverseError::Validation("active binding count overflow".into()))?;
            self.active_relation_adjacency
                .entry(readback.source)
                .or_default()
                .insert(readback.relation);
            self.active_relation_adjacency
                .entry(readback.target)
                .or_default()
                .insert(readback.relation);
        }
        if joint.is_some() {
            self.active_relation_joint_count = self
                .active_relation_joint_count
                .checked_add(1)
                .ok_or_else(|| UniverseError::Validation("active joint count overflow".into()))?;
        }
        self.relation_bindings
            .insert(readback.relation, RelationBinding { readback, joint });
        Ok(())
    }

    fn create_relation_joint(
        &mut self,
        readback: &RelationBindingReadback,
    ) -> Result<Option<ImpulseJointHandle>, UniverseError> {
        match &readback.binding {
            ResolvedRelationPhysicalBinding::NoSolverObject => Ok(None),
            ResolvedRelationPhysicalBinding::SpringJoint {
                rest_length,
                stiffness,
                damping,
                contacts_enabled,
                wake_up,
            } => {
                let source = self.bindings.get(&readback.source).ok_or_else(|| {
                    UniverseError::Validation("relation source body is not resident".into())
                })?;
                let target = self.bindings.get(&readback.target).ok_or_else(|| {
                    UniverseError::Validation("relation target body is not resident".into())
                })?;
                let joint = SpringJointBuilder::new(*rest_length, *stiffness, *damping)
                    .contacts_enabled(*contacts_enabled);
                Ok(Some(self.impulse_joints.insert(
                    source.handle,
                    target.handle,
                    joint,
                    *wake_up,
                )))
            }
        }
    }

    fn remove_relation_binding(
        &mut self,
        relation: RelationKey,
        strict_joint: bool,
    ) -> Result<RelationBindingReadback, UniverseError> {
        let binding = self
            .relation_bindings
            .remove(&relation)
            .ok_or_else(|| UniverseError::Validation("relation binding does not exist".into()))?;
        if binding.readback.status == RelationBindingStatus::Active {
            self.active_relation_binding_count =
                self.active_relation_binding_count.saturating_sub(1);
            remove_relation_adjacency(
                &mut self.active_relation_adjacency,
                binding.readback.source,
                relation,
            );
            remove_relation_adjacency(
                &mut self.active_relation_adjacency,
                binding.readback.target,
                relation,
            );
        }
        if let Some(handle) = binding.joint {
            let removed = self.impulse_joints.remove(handle, false);
            self.active_relation_joint_count = self.active_relation_joint_count.saturating_sub(1);
            if strict_joint && removed.is_none() {
                return Err(UniverseError::StaleHandle);
            }
        }
        Ok(binding.readback)
    }

    fn restore_relation_scope(
        &mut self,
        previous: &[(RelationKey, Option<RelationBindingReadback>)],
    ) -> bool {
        let mut restored = true;
        for (relation, _) in previous {
            if self.relation_bindings.contains_key(relation)
                && self.remove_relation_binding(*relation, false).is_err()
            {
                restored = false;
            }
        }
        for (_, readback) in previous {
            if let Some(readback) = readback {
                if self.install_relation_readback(readback.clone()).is_err() {
                    restored = false;
                }
            }
        }
        restored
    }

    fn materialize(
        &mut self,
        entity: EntityKey,
        generation: u32,
        state: PhysicalState,
    ) -> Result<bool, UniverseError> {
        state.validate()?;
        if self.bindings.contains_key(&entity) {
            return Ok(false);
        }
        if self.bindings.len() >= self.budget.max_active_bodies {
            return Err(UniverseError::BudgetExhausted("active body budget".into()));
        }
        let slot = u64::try_from(self.bodies.len())
            .map_err(|_| UniverseError::InvalidHandle("body slot overflow".into()))?;
        let user_data = PackedHandle {
            kind: HandleKind::Entity,
            generation,
            slot,
        }
        .pack()?;
        let body = RigidBodyBuilder::dynamic()
            .translation(vector![
                state.position[0],
                state.position[1],
                state.position[2]
            ])
            .linvel(vector![
                state.velocity[0],
                state.velocity[1],
                state.velocity[2]
            ])
            .user_data(user_data)
            .build();
        let handle = self.bodies.insert(body);
        self.bindings
            .insert(entity, BodyBinding { handle, generation });
        self.dormant.remove(&entity);
        Ok(true)
    }

    /// Materialize a resident SENSOR: a fixed body carrying a cuboid *sensor*
    /// collider that reports intersection-enter events. The collider's
    /// `user_data` packs the stable entity handle, and the handle is tracked so a
    /// reported intersection resolves back to this exact [`EntityKey`].
    ///
    /// This is trusted-computing-base geometry only. It carries no ontology: the
    /// sensor is identified solely by its handle, exactly as
    /// [`resolve_physics_event_deposits`] expects. What crossing it *means* is
    /// graph authority resolved elsewhere.
    pub fn arm_sensor(
        &mut self,
        entity: EntityKey,
        generation: u32,
        state: PhysicalState,
        half_extents: [f32; 3],
    ) -> Result<(), UniverseError> {
        let user_data = self.materialize_fixed_body(entity, generation, state, half_extents)?;
        let handle = self.bindings[&entity].handle;
        let collider = ColliderBuilder::cuboid(half_extents[0], half_extents[1], half_extents[2])
            .sensor(true)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .user_data(user_data)
            .build();
        let collider_handle = self
            .colliders
            .insert_with_parent(collider, handle, &mut self.bodies);
        self.sensor_colliders
            .insert(collider_handle.into_raw_parts(), entity);
        Ok(())
    }

    /// Materialize a resident PROBE: a dynamic body carrying a solid cuboid
    /// collider. A probe overlapping an armed sensor produces the sensor
    /// intersection-enter the bridge consumes. Like a sensor, it carries no
    /// ontology — only geometry and a packed handle.
    pub fn place_probe(
        &mut self,
        entity: EntityKey,
        generation: u32,
        state: PhysicalState,
        half_extents: [f32; 3],
    ) -> Result<(), UniverseError> {
        validate_half_extents(half_extents)?;
        let user_data = self.materialize_body(entity, generation, state, RigidBodyType::Dynamic)?;
        let handle = self.bindings[&entity].handle;
        let collider = ColliderBuilder::cuboid(half_extents[0], half_extents[1], half_extents[2])
            .user_data(user_data)
            .build();
        self.colliders
            .insert_with_parent(collider, handle, &mut self.bodies);
        Ok(())
    }

    /// Step the solver once and collect the stable [`EntityKey`] of every armed
    /// sensor that the narrow phase reports an intersection-*enter* for this
    /// step. The returned set is exactly the observed-event set
    /// [`resolve_physics_event_deposits`] consumes — a REAL Rapier collider
    /// crossing replacing the simulated handle used in unit tests.
    ///
    /// A [`PhysicsEvent`] never mutates the store: this reads solver geometry and
    /// advances the physical tick only. The event carries a handle and nothing
    /// else — zero ontology crosses this seam.
    pub fn step_collecting_sensor_intersections(
        &mut self,
    ) -> Result<BTreeSet<EntityKey>, UniverseError> {
        let (collision_send, collision_recv) = rapier3d::crossbeam::channel::unbounded();
        let (contact_force_send, _contact_force_recv) = rapier3d::crossbeam::channel::unbounded();
        let collector = ChannelEventCollector::new(collision_send, contact_force_send);
        self.pipeline.step(
            &self.gravity,
            &self.integration,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd,
            None,
            &(),
            &collector,
        );
        self.tick.0 = self
            .tick
            .0
            .checked_add(1)
            .ok_or_else(|| UniverseError::Validation("physics tick overflow".into()))?;
        let mut events = BTreeSet::new();
        while let Ok(event) = collision_recv.try_recv() {
            let CollisionEvent::Started(first, second, flags) = event else {
                continue;
            };
            if !flags.contains(CollisionEventFlags::SENSOR) {
                continue;
            }
            for handle in [first, second] {
                if let Some(&entity) = self.sensor_colliders.get(&handle.into_raw_parts()) {
                    events.insert(entity);
                }
            }
        }
        Ok(events)
    }

    /// Insert a rigid body of the given type at `state`, returning its packed
    /// `user_data`. Shared by [`Self::arm_sensor`] and [`Self::place_probe`].
    fn materialize_body(
        &mut self,
        entity: EntityKey,
        generation: u32,
        state: PhysicalState,
        body_type: RigidBodyType,
    ) -> Result<u128, UniverseError> {
        state.validate()?;
        if self.bindings.contains_key(&entity) {
            return Err(UniverseError::Validation(
                "entity is already physically resident".into(),
            ));
        }
        if self.bindings.len() >= self.budget.max_active_bodies {
            return Err(UniverseError::BudgetExhausted("active body budget".into()));
        }
        let slot = u64::try_from(self.bodies.len())
            .map_err(|_| UniverseError::InvalidHandle("body slot overflow".into()))?;
        let user_data = PackedHandle {
            kind: HandleKind::Entity,
            generation,
            slot,
        }
        .pack()?;
        let body = RigidBodyBuilder::new(body_type)
            .translation(vector![
                state.position[0],
                state.position[1],
                state.position[2]
            ])
            .linvel(vector![
                state.velocity[0],
                state.velocity[1],
                state.velocity[2]
            ])
            .user_data(user_data)
            .build();
        let handle = self.bodies.insert(body);
        self.bindings
            .insert(entity, BodyBinding { handle, generation });
        self.dormant.remove(&entity);
        Ok(user_data)
    }

    /// Insert a FIXED body with validated cuboid half-extents (the sensor host).
    fn materialize_fixed_body(
        &mut self,
        entity: EntityKey,
        generation: u32,
        state: PhysicalState,
        half_extents: [f32; 3],
    ) -> Result<u128, UniverseError> {
        validate_half_extents(half_extents)?;
        self.materialize_body(entity, generation, state, RigidBodyType::Fixed)
    }

    fn release(&mut self, entity: EntityKey) -> Result<Option<PhysicalState>, UniverseError> {
        if self
            .active_relation_adjacency
            .get(&entity)
            .is_some_and(|relations| !relations.is_empty())
        {
            return Err(UniverseError::Validation(
                "active relation bindings must be released before their endpoint body".into(),
            ));
        }
        let Some(binding) = self.bindings.remove(&entity) else {
            return Ok(None);
        };
        let body = self
            .bodies
            .get(binding.handle)
            .ok_or(UniverseError::StaleHandle)?;
        let position = body.translation();
        let velocity = body.linvel();
        let state = PhysicalState {
            position: [position.x, position.y, position.z],
            velocity: [velocity.x, velocity.y, velocity.z],
        };
        state.validate()?;
        self.bodies.remove(
            binding.handle,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
        // Removing the body removed its attached colliders; drop any sensor
        // handle bookkeeping for this entity so no stale handle can be surfaced.
        self.sensor_colliders.retain(|_, sensor_entity| *sensor_entity != entity);
        self.dormant.insert(entity, state);
        Ok(Some(state))
    }

    pub fn residency(&self, entity: EntityKey) -> Residency {
        if self.bindings.contains_key(&entity) {
            Residency::Hot
        } else {
            Residency::Dormant
        }
    }

    pub fn active_entities(&self) -> Vec<EntityKey> {
        self.bindings.keys().copied().collect()
    }

    pub fn active_count(&self) -> usize {
        self.bindings.len()
    }

    pub fn generation(&self, entity: EntityKey) -> Option<u32> {
        self.bindings.get(&entity).map(|binding| binding.generation)
    }

    pub fn relation_binding(&self, relation: RelationKey) -> Option<RelationBindingReadback> {
        self.relation_bindings
            .get(&relation)
            .map(|binding| binding.readback.clone())
    }

    pub fn active_relation_binding_count(&self) -> u32 {
        self.active_relation_binding_count
    }

    pub fn active_relation_joint_count(&self) -> u32 {
        self.active_relation_joint_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(x: f32) -> PhysicalState {
        PhysicalState {
            position: [x, 0.0, 0.0],
            velocity: [0.0; 3],
        }
    }

    fn relation_provenance(profile: u128, source_event: &str) -> RelationBindingProvenance {
        RelationBindingProvenance {
            universe_revision: Revision(7),
            mapping_revision: Revision(3),
            profile: EntityKey(profile),
            profile_hash: format!("profile-{profile}-hash"),
            source_event: source_event.into(),
        }
    }

    fn spring(rest_length: f32) -> ResolvedRelationPhysicalBinding {
        ResolvedRelationPhysicalBinding::SpringJoint {
            rest_length,
            stiffness: 10.0,
            damping: 0.5,
            contacts_enabled: false,
            wake_up: true,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn relation_command(
        idempotency_key: &str,
        relation: u128,
        source: u128,
        target: u128,
        semantic_predicate: u128,
        action: RelationPhysicalAction,
        binding: Option<ResolvedRelationPhysicalBinding>,
        profile: u128,
    ) -> RelationPhysicsCommand {
        map_relation_physical_delta(RelationPhysicalDelta {
            idempotency_key: idempotency_key.into(),
            relation: RelationKey(relation),
            source: EntityKey(source),
            target: EntityKey(target),
            semantic_predicate: EntityKey(semantic_predicate),
            action,
            binding,
            provenance: relation_provenance(profile, idempotency_key),
        })
        .unwrap()
    }

    fn relation_budget(
        max_commands: u32,
        max_active_bindings: u32,
        max_active_joints: u32,
        max_wake_cost: u32,
    ) -> RelationPhysicsBudget {
        RelationPhysicsBudget {
            max_commands,
            max_active_bindings,
            max_active_joints,
            max_wake_cost,
        }
    }

    fn resident_physics() -> UniversePhysics {
        let mut physics = UniversePhysics::new(
            1.0 / 60.0,
            PhysicsBudget {
                max_active_bodies: 3,
            },
        )
        .unwrap();
        let delta = physics.apply(vec![
            PhysicsCommand::Materialize {
                entity: EntityKey(1),
                generation: 0,
                state: state(0.0),
            },
            PhysicsCommand::Materialize {
                entity: EntityKey(2),
                generation: 0,
                state: state(2.0),
            },
            PhysicsCommand::Materialize {
                entity: EntityKey(3),
                generation: 0,
                state: state(4.0),
            },
        ]);
        assert_eq!(delta.events.len(), 3);
        assert_eq!(physics.active_count(), 3);
        physics
    }

    #[test]
    fn graph_resolved_binding_is_deterministic_and_predicate_agnostic() {
        let mut left = resident_physics();
        let mut right = resident_physics();
        let left_command = relation_command(
            "add-10",
            10,
            1,
            2,
            91,
            RelationPhysicalAction::Add,
            Some(spring(2.0)),
            501,
        );
        let right_command = relation_command(
            "add-10",
            10,
            1,
            2,
            9_999,
            RelationPhysicalAction::Add,
            Some(spring(2.0)),
            501,
        );

        let left_receipt = left.apply_relation_commands_at_tick(
            Tick(0),
            vec![left_command],
            relation_budget(1, 1, 1, 2),
        );
        let right_receipt = right.apply_relation_commands_at_tick(
            Tick(0),
            vec![right_command],
            relation_budget(1, 1, 1, 2),
        );

        assert_eq!(left_receipt, right_receipt);
        assert_eq!(left_receipt.status, RelationBatchStatus::Applied);
        assert_eq!(
            left_receipt.commands[0].outcome,
            RelationCommandOutcome::Added
        );
        assert_eq!(left.active_relation_binding_count(), 1);
        assert_eq!(left.active_relation_joint_count(), 1);
        assert_eq!(
            left.relation_binding(RelationKey(10))
                .unwrap()
                .semantic_predicate,
            EntityKey(91)
        );
        assert_eq!(
            right
                .relation_binding(RelationKey(10))
                .unwrap()
                .semantic_predicate,
            EntityKey(9_999)
        );
    }

    #[test]
    fn relation_command_idempotency_is_exact_and_collision_safe() {
        let mut physics = resident_physics();
        let command = relation_command(
            "add-20",
            20,
            1,
            2,
            91,
            RelationPhysicalAction::Add,
            Some(spring(2.0)),
            502,
        );
        let first = physics.apply_relation_commands_at_tick(
            Tick(0),
            vec![command.clone()],
            relation_budget(1, 1, 1, 2),
        );
        let second = physics.apply_relation_commands_at_tick(
            Tick(0),
            vec![command],
            relation_budget(1, 1, 1, 2),
        );

        assert_eq!(first.status, RelationBatchStatus::Applied);
        assert_eq!(second.status, RelationBatchStatus::Applied);
        assert_eq!(
            second.commands[0].outcome,
            RelationCommandOutcome::AlreadyApplied
        );
        assert_eq!(second.active_bindings_before, 1);
        assert_eq!(second.active_bindings_after, 1);
        assert_eq!(physics.active_relation_joint_count(), 1);

        let collision = relation_command(
            "add-20",
            20,
            1,
            2,
            92,
            RelationPhysicalAction::Add,
            Some(spring(2.0)),
            502,
        );
        let rejected = physics.apply_relation_commands_at_tick(
            Tick(0),
            vec![collision],
            relation_budget(1, 1, 1, 2),
        );
        assert_eq!(rejected.status, RelationBatchStatus::Rejected);
        assert!(rejected.error.unwrap().contains("collides"));
        assert_eq!(
            physics
                .relation_binding(RelationKey(20))
                .unwrap()
                .semantic_predicate,
            EntityKey(91)
        );
    }

    #[test]
    fn replace_release_and_tombstone_update_solver_residency() {
        let mut physics = resident_physics();
        let add = relation_command(
            "add-30",
            30,
            1,
            2,
            91,
            RelationPhysicalAction::Add,
            Some(spring(2.0)),
            503,
        );
        physics.apply_relation_commands_at_tick(Tick(0), vec![add], relation_budget(1, 2, 2, 2));

        let replace = relation_command(
            "replace-30",
            30,
            1,
            2,
            91,
            RelationPhysicalAction::Replace,
            Some(ResolvedRelationPhysicalBinding::NoSolverObject),
            504,
        );
        let replaced = physics.apply_relation_commands_at_tick(
            Tick(0),
            vec![replace],
            relation_budget(1, 2, 2, 0),
        );
        assert_eq!(replaced.status, RelationBatchStatus::Applied);
        assert_eq!(
            replaced.commands[0].outcome,
            RelationCommandOutcome::Replaced
        );
        assert_eq!(physics.active_relation_binding_count(), 1);
        assert_eq!(physics.active_relation_joint_count(), 0);

        let release = relation_command(
            "release-30",
            30,
            1,
            2,
            91,
            RelationPhysicalAction::Release,
            None,
            505,
        );
        let released = physics.apply_relation_commands_at_tick(
            Tick(0),
            vec![release],
            relation_budget(1, 2, 2, 0),
        );
        assert_eq!(released.status, RelationBatchStatus::Applied);
        assert_eq!(
            physics.relation_binding(RelationKey(30)).unwrap().status,
            RelationBindingStatus::Released
        );
        assert_eq!(physics.active_relation_binding_count(), 0);

        let add_for_tombstone = relation_command(
            "add-31",
            31,
            2,
            3,
            92,
            RelationPhysicalAction::Add,
            Some(ResolvedRelationPhysicalBinding::NoSolverObject),
            506,
        );
        physics.apply_relation_commands_at_tick(
            Tick(0),
            vec![add_for_tombstone],
            relation_budget(1, 2, 2, 0),
        );
        let tombstone = relation_command(
            "tombstone-31",
            31,
            2,
            3,
            92,
            RelationPhysicalAction::Tombstone,
            None,
            507,
        );
        let tombstoned = physics.apply_relation_commands_at_tick(
            Tick(0),
            vec![tombstone],
            relation_budget(1, 2, 2, 0),
        );
        assert_eq!(tombstoned.status, RelationBatchStatus::Applied);
        assert_eq!(
            physics.relation_binding(RelationKey(31)).unwrap().status,
            RelationBindingStatus::Tombstoned
        );
        assert_eq!(physics.active_relation_binding_count(), 0);

        let body_release = physics.apply(vec![PhysicsCommand::Release {
            entity: EntityKey(2),
        }]);
        assert!(matches!(
            body_release.events[0],
            PhysicsEvent::Released {
                entity: EntityKey(2),
                ..
            }
        ));
    }

    #[test]
    fn relation_budget_rejection_has_no_partial_application() {
        let mut physics = resident_physics();
        let commands = vec![
            relation_command(
                "add-40",
                40,
                1,
                2,
                91,
                RelationPhysicalAction::Add,
                Some(spring(2.0)),
                508,
            ),
            relation_command(
                "add-41",
                41,
                2,
                3,
                92,
                RelationPhysicalAction::Add,
                Some(spring(2.0)),
                509,
            ),
        ];

        let rejected =
            physics.apply_relation_commands_at_tick(Tick(0), commands, relation_budget(2, 1, 2, 4));
        assert_eq!(rejected.status, RelationBatchStatus::Rejected);
        assert_eq!(rejected.active_bindings_before, 0);
        assert_eq!(rejected.active_bindings_after, 0);
        assert!(physics.relation_binding(RelationKey(40)).is_none());
        assert!(physics.relation_binding(RelationKey(41)).is_none());
        assert_eq!(physics.active_relation_joint_count(), 0);
    }

    #[test]
    fn non_finite_profile_is_rejected_before_any_batch_mutation() {
        let invalid_delta = RelationPhysicalDelta {
            idempotency_key: "nan-map".into(),
            relation: RelationKey(50),
            source: EntityKey(1),
            target: EntityKey(2),
            semantic_predicate: EntityKey(91),
            action: RelationPhysicalAction::Add,
            binding: Some(spring(f32::NAN)),
            provenance: relation_provenance(510, "nan-map"),
        };
        assert!(matches!(
            map_relation_physical_delta(invalid_delta),
            Err(UniverseError::Validation(_))
        ));
        let mut infinite_delta = RelationPhysicalDelta {
            idempotency_key: "inf-map".into(),
            relation: RelationKey(51),
            source: EntityKey(1),
            target: EntityKey(2),
            semantic_predicate: EntityKey(91),
            action: RelationPhysicalAction::Add,
            binding: Some(spring(2.0)),
            provenance: relation_provenance(511, "inf-map"),
        };
        infinite_delta.binding = Some(ResolvedRelationPhysicalBinding::SpringJoint {
            rest_length: 2.0,
            stiffness: f32::INFINITY,
            damping: 0.5,
            contacts_enabled: false,
            wake_up: false,
        });
        assert!(matches!(
            map_relation_physical_delta(infinite_delta),
            Err(UniverseError::Validation(_))
        ));

        let mut physics = resident_physics();
        let valid = relation_command(
            "add-52",
            52,
            1,
            2,
            91,
            RelationPhysicalAction::Add,
            Some(spring(2.0)),
            512,
        );
        let mut invalid = relation_command(
            "add-53",
            53,
            2,
            3,
            92,
            RelationPhysicalAction::Add,
            Some(spring(2.0)),
            513,
        );
        invalid.binding = Some(spring(f32::NAN));
        let rejected = physics.apply_relation_commands_at_tick(
            Tick(0),
            vec![valid, invalid],
            relation_budget(2, 2, 2, 4),
        );
        assert_eq!(rejected.status, RelationBatchStatus::Rejected);
        assert!(physics.relation_binding(RelationKey(52)).is_none());
        assert!(physics.relation_binding(RelationKey(53)).is_none());
    }

    #[test]
    fn local_application_failure_rolls_back_the_bounded_relation_scope() {
        let mut physics = resident_physics();
        let initial_commands = vec![
            relation_command(
                "add-60",
                60,
                1,
                2,
                91,
                RelationPhysicalAction::Add,
                Some(spring(2.0)),
                514,
            ),
            relation_command(
                "add-61",
                61,
                2,
                3,
                92,
                RelationPhysicalAction::Add,
                Some(spring(2.0)),
                515,
            ),
        ];
        let initial = physics.apply_relation_commands_at_tick(
            Tick(0),
            initial_commands,
            relation_budget(2, 2, 2, 4),
        );
        assert_eq!(initial.status, RelationBatchStatus::Applied);
        let before_60 = physics.relation_binding(RelationKey(60)).unwrap();
        let before_61 = physics.relation_binding(RelationKey(61)).unwrap();

        let stale_joint = physics
            .relation_bindings
            .get(&RelationKey(61))
            .unwrap()
            .joint
            .unwrap();
        assert!(physics.impulse_joints.remove(stale_joint, false).is_some());

        let mutations = vec![
            relation_command(
                "replace-60",
                60,
                1,
                2,
                91,
                RelationPhysicalAction::Replace,
                Some(spring(1.5)),
                516,
            ),
            relation_command(
                "release-61",
                61,
                2,
                3,
                92,
                RelationPhysicalAction::Release,
                None,
                517,
            ),
        ];
        let receipt = physics.apply_relation_commands_at_tick(
            Tick(0),
            mutations,
            relation_budget(2, 2, 2, 2),
        );

        assert_eq!(receipt.status, RelationBatchStatus::RolledBack);
        assert!(receipt.rollback.attempted);
        assert!(receipt.rollback.restored);
        assert_eq!(
            receipt.rollback.scope,
            vec![RelationKey(60), RelationKey(61)]
        );
        assert_eq!(
            physics.relation_binding(RelationKey(60)).unwrap(),
            before_60
        );
        assert_eq!(
            physics.relation_binding(RelationKey(61)).unwrap(),
            before_61
        );
        assert_eq!(physics.active_relation_binding_count(), 2);
        assert_eq!(physics.active_relation_joint_count(), 2);
    }

    #[test]
    fn commands_are_deterministic_bounded_and_release_to_dormant() {
        let mut physics = UniversePhysics::new(
            1.0 / 60.0,
            PhysicsBudget {
                max_active_bodies: 2,
            },
        )
        .unwrap();
        let delta = physics.apply(vec![
            PhysicsCommand::Materialize {
                entity: EntityKey(2),
                generation: 0,
                state: state(2.0),
            },
            PhysicsCommand::Materialize {
                entity: EntityKey(1),
                generation: 0,
                state: state(1.0),
            },
            PhysicsCommand::Materialize {
                entity: EntityKey(3),
                generation: 0,
                state: state(3.0),
            },
            PhysicsCommand::Step,
        ]);
        assert_eq!(physics.active_entities(), vec![EntityKey(1), EntityKey(2)]);
        assert!(matches!(
            delta.events[2],
            PhysicsEvent::Rejected {
                entity: Some(EntityKey(3)),
                ..
            }
        ));
        physics.apply(vec![PhysicsCommand::Release {
            entity: EntityKey(1),
        }]);
        assert_eq!(physics.residency(EntityKey(1)), Residency::Dormant);
        assert_eq!(physics.active_count(), 1);
    }

    #[test]
    fn invalid_state_is_local_rejection() {
        let mut physics = UniversePhysics::new(
            1.0 / 60.0,
            PhysicsBudget {
                max_active_bodies: 1,
            },
        )
        .unwrap();
        let delta = physics.apply(vec![PhysicsCommand::Materialize {
            entity: EntityKey(1),
            generation: 0,
            state: state(f32::NAN),
        }]);
        assert!(matches!(delta.events[0], PhysicsEvent::Rejected { .. }));
        assert_eq!(physics.active_count(), 0);
    }

    fn atom(key: u128, seed_energy: u64, required_supports: &[u128]) -> AtomSpec {
        AtomSpec {
            key: EntityKey(key),
            threshold: if required_supports.is_empty() {
                100
            } else {
                required_supports.len() as u64 * 100
            },
            seed_energy,
            required_supports: required_supports.iter().copied().map(RelationKey).collect(),
            inhibition_threshold: None,
        }
    }

    fn bond(key: u128, source: u128, target: u128, polarity: BondPolarity) -> AtomBond {
        AtomBond {
            key: RelationKey(key),
            source: EntityKey(source),
            target: EntityKey(target),
            polarity,
            energy: 100,
        }
    }

    #[test]
    fn atom_gate_requires_every_declared_support_and_conserves_energy() {
        let mut dynamics = AtomDynamics::new(
            vec![atom(1, 100, &[]), atom(2, 100, &[]), atom(3, 0, &[1, 2])],
            vec![
                bond(1, 1, 3, BondPolarity::Support),
                bond(2, 2, 3, BondPolarity::Support),
            ],
        )
        .unwrap();
        let run = dynamics.run_until_quiescent(4).unwrap();
        assert!(run.quiescent);
        assert!(run.energy_conserved);
        assert_eq!(run.steps.len(), 2);
        assert!(dynamics.fired(EntityKey(3)));
    }

    #[test]
    fn contradiction_inhibits_an_otherwise_complete_closure() {
        let mut closure = atom(3, 0, &[1, 2]);
        closure.inhibition_threshold = Some(1);
        let mut inhibitor = atom(4, 0, &[]);
        inhibitor.threshold = 1;
        let mut inhibit = bond(3, 4, 3, BondPolarity::Inhibit);
        inhibit.energy = 1;
        let mut dynamics = AtomDynamics::new(
            vec![atom(1, 100, &[]), atom(2, 100, &[]), closure, inhibitor],
            vec![
                bond(1, 1, 3, BondPolarity::Support),
                bond(2, 2, 3, BondPolarity::Support),
                inhibit,
            ],
        )
        .unwrap();
        dynamics.inject(EntityKey(4), 1, "counterexample").unwrap();
        let run = dynamics.run_until_quiescent(4).unwrap();
        assert!(run.energy_conserved);
        assert!(!dynamics.fired(EntityKey(3)));
        assert_eq!(dynamics.state(EntityKey(3)).unwrap().inhibition_energy, 1);
    }

    #[test]
    fn atom_runs_are_exactly_deterministic() {
        let make = || {
            AtomDynamics::new(
                vec![atom(1, 100, &[]), atom(2, 100, &[]), atom(3, 0, &[1, 2])],
                vec![
                    bond(1, 1, 3, BondPolarity::Support),
                    bond(2, 2, 3, BondPolarity::Support),
                ],
            )
            .unwrap()
        };
        let mut left = make();
        let mut right = make();
        assert_eq!(
            left.run_until_quiescent(4).unwrap(),
            right.run_until_quiescent(4).unwrap()
        );
    }

    #[test]
    fn local_cluster_receipt_measures_convergence_containment_energy_and_release() {
        let receipt = execute_local_atom_cluster(
            LocalAtomCluster {
                atoms: vec![atom(1, 100, &[]), atom(2, 0, &[1])],
                bonds: vec![bond(1, 1, 2, BondPolarity::Support)],
                injections: Vec::new(),
            },
            AtomExecutionBudget {
                max_atoms: 2,
                max_bonds: 1,
                max_steps: 3,
                max_total_energy: 100,
            },
        )
        .unwrap();

        assert_eq!(receipt.atoms, vec![EntityKey(1), EntityKey(2)]);
        assert_eq!(receipt.bonds, vec![RelationKey(1)]);
        assert_eq!(receipt.convergence, AtomConvergence::Quiescent);
        assert!(receipt.energy.conserved);
        assert_eq!(receipt.energy.initial, 100);
        assert_eq!(receipt.energy.stored, 100);
        assert!(receipt.containment.within_budget);
        assert_eq!(receipt.containment.executed_steps, 2);
        assert!(receipt.lifetime.within_limit);
        assert_eq!(receipt.lifetime.ticks_used, 2);
        assert!(receipt.release.ephemeral_state_released);
        assert_eq!(receipt.release.retained_runtime_atoms, 0);
        assert!(receipt.run.quiescent);
        assert_eq!(
            assess_atom_physical_health(AtomPhysicalHealthObservation::Executed(&receipt)).envelope,
            Epistemic::Measured(AtomPhysicalEnvelopeState::WithinEnvelope)
        );
    }

    #[test]
    fn local_cluster_refuses_working_set_and_energy_over_budget() {
        let cluster = || LocalAtomCluster {
            atoms: vec![atom(1, 100, &[]), atom(2, 0, &[1])],
            bonds: vec![bond(1, 1, 2, BondPolarity::Support)],
            injections: Vec::new(),
        };
        let too_few_atoms = execute_local_atom_cluster(
            cluster(),
            AtomExecutionBudget {
                max_atoms: 1,
                max_bonds: 1,
                max_steps: 3,
                max_total_energy: 100,
            },
        );
        assert!(matches!(
            &too_few_atoms,
            Err(UniverseError::BudgetExhausted(_))
        ));
        let budget_health = assess_atom_physical_health(AtomPhysicalHealthObservation::Failed(
            too_few_atoms.as_ref().unwrap_err(),
        ));
        assert_eq!(
            budget_health.containment,
            Epistemic::Measured(AtomPhysicalHealthCheck::Fail)
        );
        assert!(matches!(
            budget_health.envelope,
            Epistemic::MeasurementFailed { .. }
        ));

        let too_little_energy = execute_local_atom_cluster(
            cluster(),
            AtomExecutionBudget {
                max_atoms: 2,
                max_bonds: 1,
                max_steps: 3,
                max_total_energy: 99,
            },
        );
        assert!(matches!(
            too_little_energy,
            Err(UniverseError::BudgetExhausted(_))
        ));
    }

    #[test]
    fn terminal_starvation_is_observed_without_breaking_conservation() {
        let mut source = atom(1, 50, &[]);
        source.threshold = 50;
        let mut expensive = bond(1, 1, 2, BondPolarity::Support);
        expensive.energy = 100;
        let receipt = execute_local_atom_cluster(
            LocalAtomCluster {
                atoms: vec![source, atom(2, 0, &[1])],
                bonds: vec![expensive],
                injections: Vec::new(),
            },
            AtomExecutionBudget {
                max_atoms: 2,
                max_bonds: 1,
                max_steps: 2,
                max_total_energy: 50,
            },
        )
        .unwrap();

        assert_eq!(receipt.convergence, AtomConvergence::Quiescent);
        assert_eq!(receipt.terminal_starved, vec![EntityKey(1)]);
        assert!(receipt.energy.conserved);
        assert_eq!(receipt.energy.stored, 50);
    }

    #[test]
    fn step_budget_exhaustion_is_a_measured_bounded_result() {
        let receipt = execute_local_atom_cluster(
            LocalAtomCluster {
                atoms: vec![atom(1, 100, &[]), atom(2, 0, &[1]), atom(3, 0, &[2])],
                bonds: vec![
                    bond(1, 1, 2, BondPolarity::Support),
                    bond(2, 2, 3, BondPolarity::Support),
                ],
                injections: Vec::new(),
            },
            AtomExecutionBudget {
                max_atoms: 3,
                max_bonds: 2,
                max_steps: 1,
                max_total_energy: 100,
            },
        )
        .unwrap();

        assert_eq!(receipt.convergence, AtomConvergence::StepBudgetExhausted);
        assert!(receipt.run.budget_exhausted);
        assert!(!receipt.run.quiescent);
        assert_eq!(receipt.containment.executed_steps, 1);
        assert!(receipt.containment.within_budget);
        assert!(receipt.energy.conserved);
        assert!(receipt.release.ephemeral_state_released);
        let health = assess_atom_physical_health(AtomPhysicalHealthObservation::Executed(&receipt));
        assert_eq!(
            health.convergence,
            Epistemic::Measured(AtomPhysicalHealthCheck::Fail)
        );
        assert_eq!(
            health.envelope,
            Epistemic::Measured(AtomPhysicalEnvelopeState::OutsideEnvelope)
        );
    }

    #[test]
    fn invalid_physics_and_unknown_state_remain_epistemically_distinct() {
        let error = execute_local_atom_cluster(
            LocalAtomCluster {
                atoms: vec![atom(1, 100, &[]), atom(2, 0, &[])],
                bonds: vec![AtomBond {
                    key: RelationKey(1),
                    source: EntityKey(1),
                    target: EntityKey(2),
                    polarity: BondPolarity::Neutral,
                    energy: 1,
                }],
                injections: Vec::new(),
            },
            AtomExecutionBudget {
                max_atoms: 2,
                max_bonds: 1,
                max_steps: 1,
                max_total_energy: 100,
            },
        )
        .unwrap_err();
        let failed = assess_atom_physical_health(AtomPhysicalHealthObservation::Failed(&error));
        assert!(matches!(
            failed.energy_conservation,
            Epistemic::MeasurementFailed { .. }
        ));
        assert!(matches!(
            failed.envelope,
            Epistemic::MeasurementFailed { .. }
        ));

        let unknown = assess_atom_physical_health(AtomPhysicalHealthObservation::Unknown);
        assert_eq!(unknown.convergence, Epistemic::Unknown);
        assert_eq!(unknown.energy_conservation, Epistemic::Unknown);
        assert_eq!(unknown.envelope, Epistemic::Unknown);
    }

    // -- the physics-event -> Atom-deposit bridge -------------------------

    #[test]
    fn a_simulated_event_deposits_energy_and_the_construct_trigger_self_wakes() {
        // A construct's trigger Atom, seeded empty, dormant: threshold 100, no
        // support, cannot fire on its own.
        let trigger = EntityKey(42);
        let mut dynamics = AtomDynamics::new(
            vec![AtomSpec {
                key: trigger,
                threshold: 100,
                seed_energy: 0,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            }],
            Vec::new(),
        )
        .unwrap();

        // Nothing fires while dormant.
        let quiet = dynamics.clone().run_until_quiescent(4).unwrap();
        assert!(quiet.quiescent);
        assert!(quiet.steps.is_empty());
        assert!(!dynamics.fired(trigger));

        // A significant physics event: a sensor collider (entity 7) reports an
        // intersection-enter. Rapier collider geometry is out of scope here, so
        // the event is SIMULATED as its stable handle appearing in the observed
        // event set — exactly the shape the bridge consumes.
        let sensor = EntityKey(7);
        let events: BTreeSet<EntityKey> = [sensor].into_iter().collect();
        let bindings = vec![PhysicsEventDeposit {
            trigger: sensor,
            target: trigger,
            weight: 100,
        }];

        // Resolve the event against the declared binding -> a runtime deposit.
        let deposits = resolve_physics_event_deposits(&bindings, &events).unwrap();
        assert_eq!(deposits.len(), 1);
        assert_eq!(deposits[0].atom, trigger);
        assert_eq!(deposits[0].energy, 100);

        // Land the deposit onto the live field BEFORE the next step wave.
        deposit_onto_dynamics(&mut dynamics, &deposits).unwrap();
        assert_eq!(dynamics.state(trigger).unwrap().support_energy, 100);

        // The construct self-wakes: it now crosses its threshold and fires.
        let run = dynamics.run_until_quiescent(4).unwrap();
        assert!(run.quiescent);
        assert_eq!(run.steps.len(), 1);
        assert_eq!(run.steps[0].fired, vec![trigger]);
        assert!(dynamics.fired(trigger));
    }

    #[test]
    fn an_unbound_event_deposits_nothing_and_a_zero_weight_binding_is_rejected() {
        let target = EntityKey(1);
        let bindings = vec![PhysicsEventDeposit {
            trigger: EntityKey(7),
            target,
            weight: 100,
        }];

        // An event for an unbound handle produces no deposit.
        let unrelated: BTreeSet<EntityKey> = [EntityKey(99)].into_iter().collect();
        assert!(resolve_physics_event_deposits(&bindings, &unrelated)
            .unwrap()
            .is_empty());

        // A zero-weight binding is invalid — weight is graph authority and must
        // be positive; there is no native default.
        let events: BTreeSet<EntityKey> = [EntityKey(7)].into_iter().collect();
        let zero = vec![PhysicsEventDeposit {
            trigger: EntityKey(7),
            target,
            weight: 0,
        }];
        assert!(resolve_physics_event_deposits(&zero, &events).is_err());
    }

    #[test]
    fn fired_atoms_are_collected_as_events_and_route_through_the_bridge() {
        // A sensor Atom that fires on its own (seeded to threshold), then a
        // downstream construct-trigger Atom that only fires once the sensor's
        // firing (a threshold crossing) is routed as a deposit.
        let sensor = EntityKey(1);
        let receipt = execute_local_atom_cluster(
            LocalAtomCluster {
                atoms: vec![AtomSpec {
                    key: sensor,
                    threshold: 100,
                    seed_energy: 100,
                    required_supports: Vec::new(),
                    inhibition_threshold: None,
                }],
                bonds: Vec::new(),
                injections: Vec::new(),
            },
            AtomExecutionBudget {
                max_atoms: 1,
                max_bonds: 1,
                max_steps: 2,
                max_total_energy: 100,
            },
        )
        .unwrap();

        // The sensor's threshold crossing is a measured event.
        let events = fired_atoms(&receipt);
        assert!(events.contains(&sensor));

        // Route it onto a downstream construct trigger.
        let downstream = EntityKey(2);
        let deposits = resolve_physics_event_deposits(
            &[PhysicsEventDeposit {
                trigger: sensor,
                target: downstream,
                weight: 100,
            }],
            &events,
        )
        .unwrap();
        assert_eq!(deposits.len(), 1);
        assert_eq!(deposits[0].atom, downstream);
    }

    #[test]
    fn a_real_rapier_sensor_crossing_surfaces_its_entity_and_drives_the_bridge() {
        // A real solver, an armed sensor at the origin, and a probe body placed
        // OVERLAPPING it. No hand-seeded event: the crossing is computed by the
        // Rapier narrow phase.
        let mut physics = UniversePhysics::new(
            1.0 / 60.0,
            PhysicsBudget {
                max_active_bodies: 4,
            },
        )
        .unwrap();
        let sensor = EntityKey(7);
        physics
            .arm_sensor(
                sensor,
                0,
                PhysicalState {
                    position: [0.0, 0.0, 0.0],
                    velocity: [0.0; 3],
                },
                [1.0, 1.0, 1.0],
            )
            .unwrap();

        // Before any crossing, a step reports no sensor intersection.
        let quiet = physics.step_collecting_sensor_intersections().unwrap();
        assert!(quiet.is_empty(), "no probe present yet: no crossing");

        // A citizen body crosses the membrane: a probe overlapping the sensor.
        physics
            .place_probe(
                EntityKey(8),
                0,
                PhysicalState {
                    position: [0.5, 0.0, 0.0],
                    velocity: [0.0; 3],
                },
                [1.0, 1.0, 1.0],
            )
            .unwrap();

        // The REAL event: the narrow phase reports the sensor intersection-enter,
        // and the bridge surfaces the sensor's stable EntityKey — exactly the
        // observed-event set `resolve_physics_event_deposits` consumes.
        let events = physics.step_collecting_sensor_intersections().unwrap();
        assert!(
            events.contains(&sensor),
            "a real collider crossing must surface the sensor's EntityKey handle"
        );

        // A dormant construct trigger: threshold 100, empty, cannot fire alone.
        let trigger = EntityKey(42);
        let mut dynamics = AtomDynamics::new(
            vec![AtomSpec {
                key: trigger,
                threshold: 100,
                seed_energy: 0,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            }],
            Vec::new(),
        )
        .unwrap();
        assert!(!dynamics.fired(trigger));

        // Route the REAL event through a declared DepositBond onto the trigger.
        let deposits = resolve_physics_event_deposits(
            &[PhysicsEventDeposit {
                trigger: sensor,
                target: trigger,
                weight: 100,
            }],
            &events,
        )
        .unwrap();
        assert_eq!(deposits.len(), 1);
        assert_eq!(deposits[0].atom, trigger);

        // Land the deposit and the construct self-wakes on the real crossing.
        deposit_onto_dynamics(&mut dynamics, &deposits).unwrap();
        let run = dynamics.run_until_quiescent(4).unwrap();
        assert!(run.quiescent);
        assert_eq!(run.steps.len(), 1);
        assert!(dynamics.fired(trigger));
    }

    #[test]
    fn released_sensor_no_longer_surfaces_events() {
        let mut physics = UniversePhysics::new(
            1.0 / 60.0,
            PhysicsBudget {
                max_active_bodies: 4,
            },
        )
        .unwrap();
        let sensor = EntityKey(7);
        physics
            .arm_sensor(
                sensor,
                0,
                PhysicalState {
                    position: [0.0, 0.0, 0.0],
                    velocity: [0.0; 3],
                },
                [1.0, 1.0, 1.0],
            )
            .unwrap();
        physics
            .place_probe(
                EntityKey(8),
                0,
                PhysicalState {
                    position: [0.5, 0.0, 0.0],
                    velocity: [0.0; 3],
                },
                [1.0, 1.0, 1.0],
            )
            .unwrap();
        assert!(physics
            .step_collecting_sensor_intersections()
            .unwrap()
            .contains(&sensor));

        // Releasing the sensor body drops its collider AND its handle bookkeeping;
        // a later crossing can no longer resolve to a stale handle.
        physics.apply(vec![PhysicsCommand::Release { entity: sensor }]);
        assert!(physics.sensor_colliders.is_empty());
        let events = physics.step_collecting_sensor_intersections().unwrap();
        assert!(events.is_empty());
    }
}
