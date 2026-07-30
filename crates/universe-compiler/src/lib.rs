//! Deterministic validation and compilation of graph-materialized IR.

pub mod atom;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use universe_core::{EntityKey, Epistemic, RelationKey, Revision, Tick};
use universe_ir::{
    BehaviorAuthority, BehaviorBond, BehaviorBudgets, BehaviorGate, BehaviorGateContent,
    BehaviorGraphProjection, BehaviorLogicBinding, BehaviorLogicKind, BehaviorLoopClosure,
    BehaviorPhysicalEvidence, BehaviorProfileParameters, BehaviorReadbackEvidence,
    BehaviorResolvedContent, CodeDefinition, EpistemicState, ExecutionRequest,
    ExecutionRequestReceipt, ExecutionRequestState, OntologyBindingStatus, Operator, Register,
    ResolvedBehaviorNode, TriggerEvent, TriggerEvidenceRequirement, TriggerIssue,
    TriggerSubscription, TriggerValidationReport, Value, IR_VERSION, TRIGGER_CONTRACT_VERSION,
};

pub const RUNTIME_BOND_PLAN_VERSION: u16 = 0;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CompileError {
    #[error("unsupported IR version {0}")]
    UnsupportedVersion(u16),
    #[error("program is empty")]
    Empty,
    #[error("register {0} is read before assignment")]
    ReadBeforeAssignment(Register),
    #[error("register {0} is assigned more than once")]
    DuplicateAssignment(Register),
    #[error("operator {0} has a zero bound")]
    ZeroBound(usize),
    #[error("branch at operator {operator} targets invalid operator {target}")]
    InvalidBranchTarget { operator: usize, target: u32 },
    #[error("branch at operator {operator} creates an unbounded cycle through {target}")]
    UnboundedCycle { operator: usize, target: u32 },
    #[error("operator {0} is unreachable")]
    UnreachableOperator(usize),
    #[error("operator {operator} calls undeclared capability {capability}")]
    UndeclaredCapability { operator: usize, capability: String },
    #[error("every reachable execution path must end in return")]
    InvalidReturn,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    pub ir_version: u16,
    pub code_revision: Revision,
    pub canonical_hash: String,
    pub required_capabilities: Vec<String>,
    pub instructions: Vec<Operator>,
    pub source_nodes: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorBondValidationReport {
    pub bond: EntityKey,
    pub behavior_hash: String,
    pub authority: Option<BehaviorAuthority>,
    pub valid: bool,
    pub issues: Vec<BehaviorBondIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeAtomPlan {
    pub atom: EntityKey,
    pub threshold: u64,
    pub seed_energy: u64,
    pub inhibition_threshold: Option<u64>,
}

/// Derived, non-authoritative execution artifact consumed by the generic
/// physics adapter. Every semantic choice and coefficient is already resolved
/// from graph authority; consumers never dispatch on `predicate`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeBondPlan {
    pub plan_version: u16,
    pub behavior_bond: EntityKey,
    pub behavior_hash: String,
    pub source: RuntimeAtomPlan,
    pub target: RuntimeAtomPlan,
    pub predicate: EntityKey,
    pub profile: EntityKey,
    pub profile_hash: String,
    pub logic_role: EntityKey,
    pub logic_role_hash: String,
    pub logic_kind: BehaviorLogicKind,
    pub transfer_energy: u64,
    pub gates: Vec<EntityKey>,
    pub objective: EntityKey,
    pub justifications: Vec<EntityKey>,
    pub budgets: BehaviorBudgets,
    pub authority: BehaviorAuthority,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeBondArtifact {
    pub artifact_hash: String,
    pub plan: RuntimeBondPlan,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RuntimeArtifactError {
    #[error("runtime plan artifact hash mismatch: expected {expected}, observed {observed}")]
    HashMismatch { expected: String, observed: String },
}

impl RuntimeBondArtifact {
    /// Verifies the content address before a downstream adapter executes the
    /// derived plan. Consumers must not reimplement canonical hashing.
    pub fn verify(&self) -> Result<(), RuntimeArtifactError> {
        let observed = runtime_plan_hash(&self.plan);
        if observed == self.artifact_hash {
            Ok(())
        } else {
            Err(RuntimeArtifactError::HashMismatch {
                expected: self.artifact_hash.clone(),
                observed,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorCompilationStatus {
    Compiled,
    Rejected,
}

/// A compiler receipt is evidence of validation and deterministic derivation.
/// It is deliberately not an execution or health receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorCompilationReceipt {
    pub plan_version: u16,
    pub bond: EntityKey,
    pub behavior_hash: String,
    pub projection_hash: Option<String>,
    pub artifact_hash: Option<String>,
    pub authority: Option<BehaviorAuthority>,
    pub status: BehaviorCompilationStatus,
    pub validation: BehaviorBondValidationReport,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorCompilation {
    pub artifact: Option<RuntimeBondArtifact>,
    pub receipt: BehaviorCompilationReceipt,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorRelationRole {
    Source,
    Target,
    Predicate,
    Profile,
    LogicRole,
    Gate,
    Objective,
    Justification,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceInputState {
    Observed,
    KnownAbsent,
    Unknown,
    NotMeasured,
    MeasurementFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "issue", rename_all = "snake_case")]
pub enum BehaviorProjectionIssue {
    LocalReadUnavailable {
        state: EvidenceInputState,
    },
    LocalReadIncomplete,
    ZeroReadBudget {
        field: String,
    },
    InvalidContentHash {
        field: String,
    },
    LocalReadRevisionMismatch {
        read: Revision,
        authority: Revision,
    },
    DuplicateVocabularyId {
        predicate: EntityKey,
    },
    DuplicateRelation {
        relation: RelationKey,
    },
    DuplicateResolvedContent {
        entity: EntityKey,
    },
    MissingRequiredRelation {
        role: BehaviorRelationRole,
    },
    InvalidCardinality {
        role: BehaviorRelationRole,
        expected: String,
        observed: u32,
    },
    MissingResolvedContent {
        entity: EntityKey,
    },
    ResolvedContentUnavailable {
        entity: EntityKey,
        state: EvidenceInputState,
    },
    ResolvedContentKindMismatch {
        entity: EntityKey,
        expected: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorProjectionValidationReport {
    pub behavior_bond: EntityKey,
    pub projection_hash: String,
    pub valid: bool,
    pub issues: Vec<BehaviorProjectionIssue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorMaterializationStatus {
    Materialized,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorMaterializationReceipt {
    pub behavior_bond: EntityKey,
    pub projection_hash: String,
    pub behavior_hash: Option<String>,
    pub status: BehaviorMaterializationStatus,
    pub validation: BehaviorProjectionValidationReport,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorMaterialization {
    pub bond: Option<BehaviorBond>,
    pub receipt: BehaviorMaterializationReceipt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorHealthEvidenceKind {
    Compilation,
    Physical,
    Readback,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "blocker", rename_all = "snake_case")]
pub enum BehaviorLoopHealthBlocker {
    EvidenceUnavailable {
        evidence: BehaviorHealthEvidenceKind,
        state: EvidenceInputState,
    },
    CompilationRejected,
    CompilationValidationFailed,
    MissingProjectionProvenance,
    MissingArtifactHash,
    BehaviorBondMismatch {
        evidence: BehaviorHealthEvidenceKind,
    },
    ArtifactHashMismatch {
        evidence: BehaviorHealthEvidenceKind,
    },
    CompilationReceiptHashMismatch,
    ProjectionHashMismatch,
    ExecutionReceiptHashMismatch,
    PhysicalNotConverged,
    EnergyNotConserved,
    PhysicalContainmentFailed,
    PhysicalReleaseFailed,
    PhysicalLifetimeExceeded,
    ContentHashReadbackFailed,
    CausalChainReadbackFailed,
    Contradiction {
        evidence: BehaviorHealthEvidenceKind,
    },
    InvalidEvidenceHash {
        field: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorLoopHealthInput {
    pub compilation: Epistemic<BehaviorCompilationReceipt>,
    pub physical: Epistemic<BehaviorPhysicalEvidence>,
    pub readback: Epistemic<BehaviorReadbackEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorLoopHealthRecord {
    pub behavior_bond: Option<EntityKey>,
    pub health: Epistemic<BehaviorLoopClosure>,
    pub blockers: Vec<BehaviorLoopHealthBlocker>,
    pub compilation_receipt_hash: Option<String>,
    pub projection_hash: Option<String>,
    pub artifact_hash: Option<String>,
    pub execution_receipt_hash: Option<String>,
    pub independent_readback_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "issue", rename_all = "snake_case")]
pub enum BehaviorBondIssue {
    MissingSource,
    MissingTarget,
    MissingPredicate,
    MissingProfile,
    MissingLogicRole,
    MissingGate,
    MissingObjective,
    MissingJustification,
    MissingBudgets,
    MissingAuthority,
    LogicBindingUnavailable {
        state: ResolvedBindingEvidenceState,
    },
    ProfileParametersUnavailable {
        state: ResolvedBindingEvidenceState,
    },
    UnresolvedOntologyGap,
    OntologyBindingUnavailable {
        state: OntologyBindingEvidenceState,
    },
    ZeroBudget {
        field: String,
    },
    InsufficientBudget {
        field: String,
        required: u64,
        actual: u64,
    },
    ZeroProfileParameter {
        field: String,
    },
    InvalidContentHash {
        field: String,
    },
    LogicEnergyMismatch {
        kind: BehaviorLogicKind,
        transfer_energy: u64,
    },
    ProfileEnergyExceedsBudget {
        required: u64,
        maximum: u64,
    },
    DuplicateGate {
        gate: EntityKey,
    },
    DuplicateJustification {
        justification: EntityKey,
    },
    GateStale {
        gate: EntityKey,
    },
    GateContradictory {
        gate: EntityKey,
    },
    GateNotClosed {
        gate: EntityKey,
        state: GateClosureState,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateClosureState {
    False,
    KnownAbsent,
    Unknown,
    NotMeasured,
    MeasurementFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyBindingEvidenceState {
    KnownAbsent,
    Unknown,
    NotMeasured,
    MeasurementFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedBindingEvidenceState {
    KnownAbsent,
    Unknown,
    NotMeasured,
    MeasurementFailed,
}

/// Validates only generic executable shape and evidence closure.
///
/// Predicate meaning and physical coefficients remain graph data. Successful
/// validation is not an execution or health receipt.
pub fn validate_behavior_bond(bond: &BehaviorBond) -> BehaviorBondValidationReport {
    let mut issues = Vec::new();
    if bond.source.is_none() {
        issues.push(BehaviorBondIssue::MissingSource);
    }
    if bond.target.is_none() {
        issues.push(BehaviorBondIssue::MissingTarget);
    }
    if bond.predicate.is_none() {
        issues.push(BehaviorBondIssue::MissingPredicate);
    }
    if bond.profile.is_none() {
        issues.push(BehaviorBondIssue::MissingProfile);
    }
    if bond.logic_role.is_none() {
        issues.push(BehaviorBondIssue::MissingLogicRole);
    }
    match &bond.logic_binding {
        Epistemic::Observed(binding) | Epistemic::Measured(binding) => {
            validate_logic_binding(binding, &mut issues);
        }
        state => issues.push(BehaviorBondIssue::LogicBindingUnavailable {
            state: resolved_binding_evidence_state(state),
        }),
    }
    match &bond.profile_parameters {
        Epistemic::Observed(parameters) | Epistemic::Measured(parameters) => {
            validate_profile_parameters(parameters, bond.budgets.as_ref(), &mut issues);
        }
        state => issues.push(BehaviorBondIssue::ProfileParametersUnavailable {
            state: resolved_binding_evidence_state(state),
        }),
    }
    if let (
        Epistemic::Observed(logic) | Epistemic::Measured(logic),
        Epistemic::Observed(parameters) | Epistemic::Measured(parameters),
    ) = (&bond.logic_binding, &bond.profile_parameters)
    {
        let transfer_is_valid = match logic.kind {
            BehaviorLogicKind::Neutral => parameters.transfer_energy == 0,
            BehaviorLogicKind::Support | BehaviorLogicKind::Inhibit => {
                parameters.transfer_energy > 0
            }
        };
        if !transfer_is_valid {
            issues.push(BehaviorBondIssue::LogicEnergyMismatch {
                kind: logic.kind,
                transfer_energy: parameters.transfer_energy,
            });
        }
    }
    if bond.gates.is_empty() {
        issues.push(BehaviorBondIssue::MissingGate);
    }
    if bond.objective.is_none() {
        issues.push(BehaviorBondIssue::MissingObjective);
    }
    if bond.justifications.is_empty() {
        issues.push(BehaviorBondIssue::MissingJustification);
    }
    match &bond.budgets {
        Some(budgets) => validate_behavior_budgets(budgets, &mut issues),
        None => issues.push(BehaviorBondIssue::MissingBudgets),
    }
    match &bond.authority {
        Some(authority) => validate_behavior_authority(authority, &mut issues),
        None => issues.push(BehaviorBondIssue::MissingAuthority),
    }
    match &bond.ontology_status {
        Epistemic::Observed(OntologyBindingStatus::Active)
        | Epistemic::Measured(OntologyBindingStatus::Active) => {}
        Epistemic::Observed(OntologyBindingStatus::UnresolvedGap)
        | Epistemic::Measured(OntologyBindingStatus::UnresolvedGap) => {
            issues.push(BehaviorBondIssue::UnresolvedOntologyGap);
        }
        Epistemic::KnownAbsent => issues.push(BehaviorBondIssue::OntologyBindingUnavailable {
            state: OntologyBindingEvidenceState::KnownAbsent,
        }),
        Epistemic::Unknown => issues.push(BehaviorBondIssue::OntologyBindingUnavailable {
            state: OntologyBindingEvidenceState::Unknown,
        }),
        Epistemic::NotMeasured => issues.push(BehaviorBondIssue::OntologyBindingUnavailable {
            state: OntologyBindingEvidenceState::NotMeasured,
        }),
        Epistemic::MeasurementFailed { .. } => {
            issues.push(BehaviorBondIssue::OntologyBindingUnavailable {
                state: OntologyBindingEvidenceState::MeasurementFailed,
            });
        }
    }
    for gate in &bond.gates {
        if gate.stale {
            issues.push(BehaviorBondIssue::GateStale { gate: gate.gate });
        }
        if gate.contradictory {
            issues.push(BehaviorBondIssue::GateContradictory { gate: gate.gate });
        }
        let state = match &gate.closure {
            Epistemic::Observed(true) | Epistemic::Measured(true) => None,
            Epistemic::Observed(false) | Epistemic::Measured(false) => {
                Some(GateClosureState::False)
            }
            Epistemic::KnownAbsent => Some(GateClosureState::KnownAbsent),
            Epistemic::Unknown => Some(GateClosureState::Unknown),
            Epistemic::NotMeasured => Some(GateClosureState::NotMeasured),
            Epistemic::MeasurementFailed { .. } => Some(GateClosureState::MeasurementFailed),
        };
        if let Some(state) = state {
            issues.push(BehaviorBondIssue::GateNotClosed {
                gate: gate.gate,
                state,
            });
        }
    }
    push_duplicates(
        bond.gates.iter().map(|gate| gate.gate),
        |gate| BehaviorBondIssue::DuplicateGate { gate },
        &mut issues,
    );
    push_duplicates(
        bond.justifications.iter().copied(),
        |justification| BehaviorBondIssue::DuplicateJustification { justification },
        &mut issues,
    );
    BehaviorBondValidationReport {
        bond: bond.bond,
        behavior_hash: canonical_behavior_hash(bond),
        authority: bond.authority.clone(),
        valid: issues.is_empty(),
        issues,
    }
}

fn validate_behavior_budgets(budgets: &BehaviorBudgets, issues: &mut Vec<BehaviorBondIssue>) {
    for (field, value) in [
        ("max_atoms", u64::from(budgets.max_atoms)),
        ("max_bonds", u64::from(budgets.max_bonds)),
        ("max_steps", u64::from(budgets.max_steps)),
        ("lifetime_ticks", u64::from(budgets.lifetime_ticks)),
        ("max_total_energy", budgets.max_total_energy),
        ("max_wake_cost", u64::from(budgets.max_wake_cost)),
    ] {
        if value == 0 {
            issues.push(BehaviorBondIssue::ZeroBudget {
                field: field.into(),
            });
        }
    }
    for (field, required, actual) in [
        ("max_atoms", 2, u64::from(budgets.max_atoms)),
        ("max_bonds", 1, u64::from(budgets.max_bonds)),
    ] {
        if actual > 0 && actual < required {
            issues.push(BehaviorBondIssue::InsufficientBudget {
                field: field.into(),
                required,
                actual,
            });
        }
    }
}

fn validate_logic_binding(binding: &BehaviorLogicBinding, issues: &mut Vec<BehaviorBondIssue>) {
    validate_content_hash(
        "logic_binding.definition_hash",
        &binding.definition_hash,
        issues,
    );
}

fn validate_profile_parameters(
    parameters: &BehaviorProfileParameters,
    budgets: Option<&BehaviorBudgets>,
    issues: &mut Vec<BehaviorBondIssue>,
) {
    validate_content_hash(
        "profile_parameters.profile_hash",
        &parameters.profile_hash,
        issues,
    );
    for (field, value) in [
        ("source_threshold", parameters.source_threshold),
        ("target_threshold", parameters.target_threshold),
    ] {
        if value == 0 {
            issues.push(BehaviorBondIssue::ZeroProfileParameter {
                field: field.into(),
            });
        }
    }
    for (field, value) in [
        (
            "source_inhibition_threshold",
            parameters.source_inhibition_threshold,
        ),
        (
            "target_inhibition_threshold",
            parameters.target_inhibition_threshold,
        ),
    ] {
        if value == Some(0) {
            issues.push(BehaviorBondIssue::ZeroProfileParameter {
                field: field.into(),
            });
        }
    }
    if let Some(budgets) = budgets {
        let required = parameters
            .source_seed_energy
            .checked_add(parameters.target_seed_energy);
        match required {
            Some(required) if required > budgets.max_total_energy => {
                issues.push(BehaviorBondIssue::ProfileEnergyExceedsBudget {
                    required,
                    maximum: budgets.max_total_energy,
                });
            }
            None => issues.push(BehaviorBondIssue::ProfileEnergyExceedsBudget {
                required: u64::MAX,
                maximum: budgets.max_total_energy,
            }),
            _ => {}
        }
        if parameters.transfer_energy > budgets.max_total_energy {
            issues.push(BehaviorBondIssue::ProfileEnergyExceedsBudget {
                required: parameters.transfer_energy,
                maximum: budgets.max_total_energy,
            });
        }
    }
}

fn validate_behavior_authority(authority: &BehaviorAuthority, issues: &mut Vec<BehaviorBondIssue>) {
    for (field, hash) in [
        ("authority.change_set_hash", &authority.change_set_hash),
        ("authority.ontology_hash", &authority.ontology_hash),
        ("authority.mapping_hash", &authority.mapping_hash),
    ] {
        validate_content_hash(field, hash, issues);
    }
}

fn validate_content_hash(field: &str, hash: &str, issues: &mut Vec<BehaviorBondIssue>) {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        issues.push(BehaviorBondIssue::InvalidContentHash {
            field: field.into(),
        });
    }
}

fn resolved_binding_evidence_state<T>(state: &Epistemic<T>) -> ResolvedBindingEvidenceState {
    match state {
        Epistemic::KnownAbsent => ResolvedBindingEvidenceState::KnownAbsent,
        Epistemic::Unknown => ResolvedBindingEvidenceState::Unknown,
        Epistemic::NotMeasured => ResolvedBindingEvidenceState::NotMeasured,
        Epistemic::MeasurementFailed { .. } => ResolvedBindingEvidenceState::MeasurementFailed,
        Epistemic::Observed(_) | Epistemic::Measured(_) => {
            unreachable!("resolved evidence is handled before this conversion")
        }
    }
}

fn push_duplicates(
    values: impl Iterator<Item = EntityKey>,
    issue: impl Fn(EntityKey) -> BehaviorBondIssue,
    issues: &mut Vec<BehaviorBondIssue>,
) {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            issues.push(issue(value));
        }
    }
}

fn canonical_behavior_hash(bond: &BehaviorBond) -> String {
    let mut canonical = bond.clone();
    canonical.gates.sort_by_key(|gate| gate.gate);
    canonical.justifications.sort();
    hex::encode(Sha256::digest(
        serde_json::to_vec(&canonical).expect("BehaviorBond serialization is infallible"),
    ))
}

fn runtime_plan_hash(plan: &RuntimeBondPlan) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(plan).expect("RuntimeBondPlan serialization is infallible"),
    ))
}

/// Compiles one graph-materialized BehaviorBond into a content-addressed
/// physical plan and always emits a receipt, including on validation failure.
pub fn compile_behavior_bond(bond: &BehaviorBond) -> BehaviorCompilation {
    let validation = validate_behavior_bond(bond);
    if !validation.valid {
        return rejected_behavior_compilation(validation);
    }

    let (
        Some(source),
        Some(target),
        Some(predicate),
        Some(profile),
        Some(logic_role),
        Some(objective),
        Some(budgets),
        Some(authority),
    ) = (
        bond.source,
        bond.target,
        bond.predicate,
        bond.profile,
        bond.logic_role,
        bond.objective,
        bond.budgets.clone(),
        bond.authority.clone(),
    )
    else {
        return rejected_behavior_compilation(validation);
    };
    let logic = match &bond.logic_binding {
        Epistemic::Observed(binding) | Epistemic::Measured(binding) => binding,
        _ => return rejected_behavior_compilation(validation),
    };
    let parameters = match &bond.profile_parameters {
        Epistemic::Observed(parameters) | Epistemic::Measured(parameters) => parameters,
        _ => return rejected_behavior_compilation(validation),
    };

    let mut gates = bond.gates.iter().map(|gate| gate.gate).collect::<Vec<_>>();
    gates.sort();
    let mut justifications = bond.justifications.clone();
    justifications.sort();
    let plan = RuntimeBondPlan {
        plan_version: RUNTIME_BOND_PLAN_VERSION,
        behavior_bond: bond.bond,
        behavior_hash: validation.behavior_hash.clone(),
        source: RuntimeAtomPlan {
            atom: source,
            threshold: parameters.source_threshold,
            seed_energy: parameters.source_seed_energy,
            inhibition_threshold: parameters.source_inhibition_threshold,
        },
        target: RuntimeAtomPlan {
            atom: target,
            threshold: parameters.target_threshold,
            seed_energy: parameters.target_seed_energy,
            inhibition_threshold: parameters.target_inhibition_threshold,
        },
        predicate,
        profile,
        profile_hash: parameters.profile_hash.clone(),
        logic_role,
        logic_role_hash: logic.definition_hash.clone(),
        logic_kind: logic.kind,
        transfer_energy: parameters.transfer_energy,
        gates,
        objective,
        justifications,
        budgets,
        authority: authority.clone(),
    };
    let artifact_hash = runtime_plan_hash(&plan);
    BehaviorCompilation {
        receipt: BehaviorCompilationReceipt {
            plan_version: RUNTIME_BOND_PLAN_VERSION,
            bond: bond.bond,
            behavior_hash: validation.behavior_hash.clone(),
            projection_hash: None,
            artifact_hash: Some(artifact_hash.clone()),
            authority: Some(authority),
            status: BehaviorCompilationStatus::Compiled,
            validation,
        },
        artifact: Some(RuntimeBondArtifact {
            artifact_hash,
            plan,
        }),
    }
}

fn rejected_behavior_compilation(validation: BehaviorBondValidationReport) -> BehaviorCompilation {
    BehaviorCompilation {
        receipt: BehaviorCompilationReceipt {
            plan_version: RUNTIME_BOND_PLAN_VERSION,
            bond: validation.bond,
            behavior_hash: validation.behavior_hash.clone(),
            projection_hash: None,
            artifact_hash: None,
            authority: validation.authority.clone(),
            status: BehaviorCompilationStatus::Rejected,
            validation,
        },
        artifact: None,
    }
}

pub fn behavior_compilation_receipt_hash(receipt: &BehaviorCompilationReceipt) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(receipt)
            .expect("BehaviorCompilationReceipt serialization is infallible"),
    ))
}

pub const BEHAVIOR_LOOP_HEALTH_INPUT_NAMES: &[&str] = &[
    "compilation_status",
    "compilation_validation",
    "compilation_projection_hash",
    "compilation_projection_hash_valid",
    "compilation_artifact_hash",
    "compilation_artifact_hash_valid",
    "compilation_bond",
    "compilation_receipt_hash",
    "physical_bond",
    "physical_artifact_hash",
    "physical_execution_hash",
    "physical_execution_hash_valid",
    "physical_converged",
    "physical_energy_conserved",
    "physical_contained",
    "physical_released",
    "physical_lifetime_within_limit",
    "readback_bond",
    "readback_projection_hash",
    "readback_compilation_hash",
    "readback_artifact_hash",
    "readback_execution_hash",
    "readback_independent_hash_valid",
    "readback_content_hashes_verified",
    "readback_causal_chain_verified",
    "readback_contradictory",
];

fn project_evidence<T>(
    combined_failure_reason: Option<&str>,
    evidence: &Epistemic<T>,
    project: impl Fn(&T) -> Value,
) -> Value {
    Value::Epistemic(match evidence {
        Epistemic::Observed(value) => Epistemic::Observed(Box::new(project(value))),
        Epistemic::Measured(value) => Epistemic::Measured(Box::new(project(value))),
        Epistemic::KnownAbsent => Epistemic::KnownAbsent,
        Epistemic::Unknown => Epistemic::Unknown,
        Epistemic::NotMeasured => Epistemic::NotMeasured,
        Epistemic::MeasurementFailed { reason } => Epistemic::MeasurementFailed {
            reason: combined_failure_reason.unwrap_or(reason).into(),
        },
    })
}

fn combined_health_measurement_failure_reason(input: &BehaviorLoopHealthInput) -> Option<String> {
    let mut reasons = Vec::new();
    for (reason, name) in [
        (
            match &input.compilation {
                Epistemic::MeasurementFailed { reason } => Some(reason),
                _ => None,
            },
            "compilation",
        ),
        (
            match &input.physical {
                Epistemic::MeasurementFailed { reason } => Some(reason),
                _ => None,
            },
            "physical",
        ),
        (
            match &input.readback {
                Epistemic::MeasurementFailed { reason } => Some(reason),
                _ => None,
            },
            "readback",
        ),
    ] {
        if let Some(reason) = reason {
            reasons.push(format!("{name}: {reason}"));
        }
    }
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

fn optional_text(value: Option<&String>) -> Value {
    value
        .map(|value| Value::Text(value.clone()))
        .unwrap_or(Value::Unit)
}

type NamedProjection<T, U> = (&'static str, fn(&T) -> U);

/// Projects receipts into atomic epistemic facts consumed by the graph-owned
/// Behavior Loop health CodeDefinition. This adapter performs no conjunction
/// and makes no closure decision.
pub fn behavior_loop_health_graph_inputs(
    input: &BehaviorLoopHealthInput,
) -> BTreeMap<String, Value> {
    let mut values = BTreeMap::new();
    let measurement_failure_reason = combined_health_measurement_failure_reason(input);
    let mut insert = |name: &str, value: Value| {
        values.insert(name.into(), value);
    };

    insert(
        "compilation_status",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.compilation,
            |receipt| {
                Value::Text(
                    match receipt.status {
                        BehaviorCompilationStatus::Compiled => "compiled",
                        BehaviorCompilationStatus::Rejected => "rejected",
                    }
                    .into(),
                )
            },
        ),
    );
    insert(
        "compilation_validation",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.compilation,
            |receipt| Value::Bool(receipt.validation.valid),
        ),
    );
    insert(
        "compilation_projection_hash",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.compilation,
            |receipt| optional_text(receipt.projection_hash.as_ref()),
        ),
    );
    insert(
        "compilation_projection_hash_valid",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.compilation,
            |receipt| {
                Value::Bool(
                    receipt
                        .projection_hash
                        .as_deref()
                        .is_some_and(is_content_hash),
                )
            },
        ),
    );
    insert(
        "compilation_artifact_hash",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.compilation,
            |receipt| optional_text(receipt.artifact_hash.as_ref()),
        ),
    );
    insert(
        "compilation_artifact_hash_valid",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.compilation,
            |receipt| {
                Value::Bool(
                    receipt
                        .artifact_hash
                        .as_deref()
                        .is_some_and(is_content_hash),
                )
            },
        ),
    );
    insert(
        "compilation_bond",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.compilation,
            |receipt| Value::Entity(receipt.bond),
        ),
    );
    insert(
        "compilation_receipt_hash",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.compilation,
            |receipt| Value::Text(behavior_compilation_receipt_hash(receipt)),
        ),
    );

    insert(
        "physical_bond",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.physical,
            |evidence| Value::Entity(evidence.behavior_bond),
        ),
    );
    insert(
        "physical_artifact_hash",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.physical,
            |evidence| Value::Text(evidence.artifact_hash.clone()),
        ),
    );
    insert(
        "physical_execution_hash",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.physical,
            |evidence| Value::Text(evidence.execution_receipt_hash.clone()),
        ),
    );
    insert(
        "physical_execution_hash_valid",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.physical,
            |evidence| Value::Bool(is_content_hash(&evidence.execution_receipt_hash)),
        ),
    );
    let physical_flags: [NamedProjection<BehaviorPhysicalEvidence, bool>; 5] = [
        (
            "physical_converged",
            |evidence: &BehaviorPhysicalEvidence| evidence.converged,
        ),
        ("physical_energy_conserved", |evidence| {
            evidence.energy_conserved
        }),
        ("physical_contained", |evidence| evidence.contained),
        ("physical_released", |evidence| evidence.released),
        ("physical_lifetime_within_limit", |evidence| {
            evidence.lifetime_within_limit
        }),
    ];
    for (name, project) in physical_flags {
        insert(
            name,
            project_evidence(
                measurement_failure_reason.as_deref(),
                &input.physical,
                |evidence| Value::Bool(project(evidence)),
            ),
        );
    }

    insert(
        "readback_bond",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.readback,
            |evidence| Value::Entity(evidence.behavior_bond),
        ),
    );
    let readback_hashes: [NamedProjection<BehaviorReadbackEvidence, String>; 4] = [
        (
            "readback_projection_hash",
            |evidence: &BehaviorReadbackEvidence| evidence.projection_hash.clone(),
        ),
        ("readback_compilation_hash", |evidence| {
            evidence.compilation_receipt_hash.clone()
        }),
        ("readback_artifact_hash", |evidence| {
            evidence.artifact_hash.clone()
        }),
        ("readback_execution_hash", |evidence| {
            evidence.execution_receipt_hash.clone()
        }),
    ];
    for (name, project) in readback_hashes {
        insert(
            name,
            project_evidence(
                measurement_failure_reason.as_deref(),
                &input.readback,
                |evidence| Value::Text(project(evidence)),
            ),
        );
    }
    insert(
        "readback_independent_hash_valid",
        project_evidence(
            measurement_failure_reason.as_deref(),
            &input.readback,
            |evidence| Value::Bool(is_content_hash(&evidence.independent_readback_hash)),
        ),
    );
    let readback_flags: [NamedProjection<BehaviorReadbackEvidence, bool>; 3] = [
        (
            "readback_content_hashes_verified",
            |evidence: &BehaviorReadbackEvidence| evidence.content_hashes_verified,
        ),
        ("readback_causal_chain_verified", |evidence| {
            evidence.causal_chain_verified
        }),
        ("readback_contradictory", |evidence| evidence.contradictory),
    ];
    for (name, project) in readback_flags {
        insert(
            name,
            project_evidence(
                measurement_failure_reason.as_deref(),
                &input.readback,
                |evidence| Value::Bool(project(evidence)),
            ),
        );
    }
    values
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BehaviorHealthDecodeError {
    #[error("Behavior Loop health program must return an epistemic boolean")]
    ExpectedEpistemicBool,
}

pub fn decode_behavior_loop_health(
    value: &Value,
) -> Result<Epistemic<BehaviorLoopClosure>, BehaviorHealthDecodeError> {
    let Value::Epistemic(evidence) = value else {
        return Err(BehaviorHealthDecodeError::ExpectedEpistemicBool);
    };
    let decode = |value: &Value| match value {
        Value::Bool(true) => Ok(BehaviorLoopClosure::Closed),
        Value::Bool(false) => Ok(BehaviorLoopClosure::Open),
        _ => Err(BehaviorHealthDecodeError::ExpectedEpistemicBool),
    };
    Ok(match evidence {
        Epistemic::Observed(value) => Epistemic::Observed(decode(value)?),
        Epistemic::Measured(value) => Epistemic::Measured(decode(value)?),
        Epistemic::KnownAbsent => Epistemic::KnownAbsent,
        Epistemic::Unknown => Epistemic::Unknown,
        Epistemic::NotMeasured => Epistemic::NotMeasured,
        Epistemic::MeasurementFailed { reason } => Epistemic::MeasurementFailed {
            reason: reason.clone(),
        },
    })
}

fn canonical_projection_hash(projection: &BehaviorGraphProjection) -> String {
    let mut canonical = projection.clone();
    canonical
        .relations
        .sort_by_key(|relation| relation.relation);
    canonical.contents.sort_by_key(|content| content.entity);
    hex::encode(Sha256::digest(
        serde_json::to_vec(&canonical)
            .expect("BehaviorGraphProjection serialization is infallible"),
    ))
}

fn is_content_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn input_state<T>(evidence: &Epistemic<T>) -> Option<EvidenceInputState> {
    match evidence {
        Epistemic::Measured(_) => None,
        Epistemic::Observed(_) => Some(EvidenceInputState::Observed),
        Epistemic::KnownAbsent => Some(EvidenceInputState::KnownAbsent),
        Epistemic::Unknown => Some(EvidenceInputState::Unknown),
        Epistemic::NotMeasured => Some(EvidenceInputState::NotMeasured),
        Epistemic::MeasurementFailed { .. } => Some(EvidenceInputState::MeasurementFailed),
    }
}

fn rejected_materialization(
    behavior_bond: EntityKey,
    projection_hash: String,
    issues: Vec<BehaviorProjectionIssue>,
) -> BehaviorMaterialization {
    let validation = BehaviorProjectionValidationReport {
        behavior_bond,
        projection_hash: projection_hash.clone(),
        valid: false,
        issues,
    };
    BehaviorMaterialization {
        bond: None,
        receipt: BehaviorMaterializationReceipt {
            behavior_bond,
            projection_hash,
            behavior_hash: None,
            status: BehaviorMaterializationStatus::Rejected,
            validation,
        },
    }
}

fn role_targets(projection: &BehaviorGraphProjection, predicate: EntityKey) -> Vec<EntityKey> {
    let mut targets = projection
        .relations
        .iter()
        .filter(|relation| {
            relation.source == projection.behavior_bond && relation.predicate == predicate
        })
        .map(|relation| relation.target)
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

fn require_cardinality(
    role: BehaviorRelationRole,
    targets: &[EntityKey],
    exactly_one: bool,
    issues: &mut Vec<BehaviorProjectionIssue>,
) {
    let observed = u32::try_from(targets.len()).unwrap_or(u32::MAX);
    if observed == 0 {
        issues.push(BehaviorProjectionIssue::MissingRequiredRelation { role });
    } else if exactly_one && observed != 1 {
        issues.push(BehaviorProjectionIssue::InvalidCardinality {
            role,
            expected: "exactly_one".into(),
            observed,
        });
    }
}

fn measured_content<'a>(
    contents: &'a BTreeMap<EntityKey, &'a ResolvedBehaviorNode>,
    entity: EntityKey,
    expected: &'static str,
    matches_kind: impl Fn(&BehaviorResolvedContent) -> bool,
    issues: &mut Vec<BehaviorProjectionIssue>,
) -> Option<&'a BehaviorResolvedContent> {
    let Some(content) = contents.get(&entity) else {
        issues.push(BehaviorProjectionIssue::MissingResolvedContent { entity });
        return None;
    };
    match &content.value {
        Epistemic::Measured(value) if matches_kind(value) => Some(value),
        Epistemic::Measured(_) => {
            issues.push(BehaviorProjectionIssue::ResolvedContentKindMismatch {
                entity,
                expected: expected.into(),
            });
            None
        }
        value => {
            issues.push(BehaviorProjectionIssue::ResolvedContentUnavailable {
                entity,
                state: input_state(value)
                    .expect("non-measured resolved content has an evidence state"),
            });
            None
        }
    }
}

/// Materializes a BehaviorBond exclusively from a complete, measured local
/// graph projection. Required relation absence is never inferred from a
/// partial or unmeasured read.
pub fn materialize_behavior_bond(projection: &BehaviorGraphProjection) -> BehaviorMaterialization {
    let projection_hash = canonical_projection_hash(projection);
    let read = match &projection.local_read {
        Epistemic::Measured(read) => read,
        value => {
            return rejected_materialization(
                projection.behavior_bond,
                projection_hash,
                vec![BehaviorProjectionIssue::LocalReadUnavailable {
                    state: input_state(value)
                        .expect("non-measured local read has an evidence state"),
                }],
            );
        }
    };
    if !read.complete_for_behavior {
        return rejected_materialization(
            projection.behavior_bond,
            projection_hash,
            vec![BehaviorProjectionIssue::LocalReadIncomplete],
        );
    }

    let mut issues = Vec::new();
    for (field, value) in [
        ("max_entities", read.max_entities),
        ("max_relations", read.max_relations),
        ("max_content_bytes", read.max_content_bytes),
        ("timeout_ticks", read.timeout_ticks),
    ] {
        if value == 0 {
            issues.push(BehaviorProjectionIssue::ZeroReadBudget {
                field: field.into(),
            });
        }
    }
    for (field, hash) in [
        ("local_read.query_receipt_hash", &read.query_receipt_hash),
        (
            "local_read.independent_readback_hash",
            &read.independent_readback_hash,
        ),
        (
            "local_read.active_registry_hash",
            &read.active_registry_hash,
        ),
    ] {
        if !is_content_hash(hash) {
            issues.push(BehaviorProjectionIssue::InvalidContentHash {
                field: field.into(),
            });
        }
    }

    let vocabulary_ids = [
        projection.vocabulary.source_atom,
        projection.vocabulary.target_atom,
        projection.vocabulary.uses_predicate,
        projection.vocabulary.uses_profile,
        projection.vocabulary.has_logic_role,
        projection.vocabulary.gated_by,
        projection.vocabulary.serves_objective,
        projection.vocabulary.justified_by,
    ];
    let mut vocabulary_seen = BTreeSet::new();
    for predicate in vocabulary_ids {
        if !vocabulary_seen.insert(predicate) {
            issues.push(BehaviorProjectionIssue::DuplicateVocabularyId { predicate });
        }
    }

    let mut relation_seen = BTreeSet::new();
    for relation in &projection.relations {
        if !relation_seen.insert(relation.relation) {
            issues.push(BehaviorProjectionIssue::DuplicateRelation {
                relation: relation.relation,
            });
        }
    }
    let mut contents = BTreeMap::new();
    for content in &projection.contents {
        if contents.insert(content.entity, content).is_some() {
            issues.push(BehaviorProjectionIssue::DuplicateResolvedContent {
                entity: content.entity,
            });
        }
        if !is_content_hash(&content.content_hash) {
            issues.push(BehaviorProjectionIssue::InvalidContentHash {
                field: format!("contents.{}.content_hash", content.entity),
            });
        }
    }

    let source = role_targets(projection, projection.vocabulary.source_atom);
    let target = role_targets(projection, projection.vocabulary.target_atom);
    let predicate = role_targets(projection, projection.vocabulary.uses_predicate);
    let profile = role_targets(projection, projection.vocabulary.uses_profile);
    let logic_role = role_targets(projection, projection.vocabulary.has_logic_role);
    let gates = role_targets(projection, projection.vocabulary.gated_by);
    let objective = role_targets(projection, projection.vocabulary.serves_objective);
    let justifications = role_targets(projection, projection.vocabulary.justified_by);
    for (role, targets, exactly_one) in [
        (BehaviorRelationRole::Source, source.as_slice(), true),
        (BehaviorRelationRole::Target, target.as_slice(), true),
        (BehaviorRelationRole::Predicate, predicate.as_slice(), true),
        (BehaviorRelationRole::Profile, profile.as_slice(), true),
        (BehaviorRelationRole::LogicRole, logic_role.as_slice(), true),
        (BehaviorRelationRole::Gate, gates.as_slice(), false),
        (BehaviorRelationRole::Objective, objective.as_slice(), true),
        (
            BehaviorRelationRole::Justification,
            justifications.as_slice(),
            false,
        ),
    ] {
        require_cardinality(role, targets, exactly_one, &mut issues);
    }

    if !issues.is_empty() {
        return rejected_materialization(projection.behavior_bond, projection_hash, issues);
    }

    let bond_content = measured_content(
        &contents,
        projection.behavior_bond,
        "bond",
        |value| matches!(value, BehaviorResolvedContent::Bond(_)),
        &mut issues,
    );
    let predicate_content = measured_content(
        &contents,
        predicate[0],
        "predicate",
        |value| matches!(value, BehaviorResolvedContent::Predicate(_)),
        &mut issues,
    );
    let profile_content = measured_content(
        &contents,
        profile[0],
        "physical_profile",
        |value| matches!(value, BehaviorResolvedContent::PhysicalProfile(_)),
        &mut issues,
    );
    let logic_content = measured_content(
        &contents,
        logic_role[0],
        "logic_role",
        |value| matches!(value, BehaviorResolvedContent::LogicRole(_)),
        &mut issues,
    );
    let gate_contents = gates
        .iter()
        .filter_map(|gate| {
            measured_content(
                &contents,
                *gate,
                "gate",
                |value| matches!(value, BehaviorResolvedContent::Gate(_)),
                &mut issues,
            )
            .map(|content| (*gate, content))
        })
        .collect::<Vec<_>>();
    let _objective_content = measured_content(
        &contents,
        objective[0],
        "objective",
        |value| matches!(value, BehaviorResolvedContent::Objective),
        &mut issues,
    );
    let _justification_contents = justifications
        .iter()
        .filter_map(|justification| {
            measured_content(
                &contents,
                *justification,
                "justification",
                |value| matches!(value, BehaviorResolvedContent::Justification),
                &mut issues,
            )
        })
        .collect::<Vec<_>>();

    let properties = match bond_content {
        Some(BehaviorResolvedContent::Bond(properties)) => Some(properties),
        _ => None,
    };
    if let Some(properties) = properties {
        if properties.authority.universe_revision != read.universe_revision {
            issues.push(BehaviorProjectionIssue::LocalReadRevisionMismatch {
                read: read.universe_revision,
                authority: properties.authority.universe_revision,
            });
        }
    }
    if !issues.is_empty() {
        return rejected_materialization(projection.behavior_bond, projection_hash, issues);
    }

    let BehaviorResolvedContent::Predicate(ontology_status) =
        predicate_content.expect("validated predicate content")
    else {
        unreachable!("validated predicate content kind")
    };
    let BehaviorResolvedContent::PhysicalProfile(profile_content) =
        profile_content.expect("validated profile content")
    else {
        unreachable!("validated profile content kind")
    };
    let BehaviorResolvedContent::LogicRole(logic_content) =
        logic_content.expect("validated logic content")
    else {
        unreachable!("validated logic content kind")
    };
    let properties = properties.expect("validated bond properties");
    let profile_parameters = BehaviorProfileParameters {
        profile_hash: contents[&profile[0]].content_hash.clone(),
        source_threshold: profile_content.source_threshold,
        source_seed_energy: profile_content.source_seed_energy,
        source_inhibition_threshold: profile_content.source_inhibition_threshold,
        target_threshold: profile_content.target_threshold,
        target_seed_energy: profile_content.target_seed_energy,
        target_inhibition_threshold: profile_content.target_inhibition_threshold,
        transfer_energy: profile_content.transfer_energy,
    };
    let logic_binding = BehaviorLogicBinding {
        kind: logic_content.kind,
        definition_hash: contents[&logic_role[0]].content_hash.clone(),
    };
    let gates = gate_contents
        .into_iter()
        .map(|(gate, content)| {
            let BehaviorResolvedContent::Gate(BehaviorGateContent {
                closure,
                stale,
                contradictory,
            }) = content
            else {
                unreachable!("validated gate content kind")
            };
            BehaviorGate {
                gate,
                closure: closure.clone(),
                stale: *stale,
                contradictory: *contradictory,
            }
        })
        .collect();
    let bond = BehaviorBond {
        bond: projection.behavior_bond,
        source: Some(source[0]),
        target: Some(target[0]),
        predicate: Some(predicate[0]),
        profile: Some(profile[0]),
        logic_role: Some(logic_role[0]),
        logic_binding: Epistemic::Measured(logic_binding),
        profile_parameters: Epistemic::Measured(profile_parameters),
        gates,
        objective: Some(objective[0]),
        justifications,
        budgets: Some(properties.budgets.clone()),
        authority: Some(properties.authority.clone()),
        ontology_status: Epistemic::Measured(*ontology_status),
    };
    let behavior_hash = canonical_behavior_hash(&bond);
    let validation = BehaviorProjectionValidationReport {
        behavior_bond: projection.behavior_bond,
        projection_hash: projection_hash.clone(),
        valid: true,
        issues: Vec::new(),
    };
    BehaviorMaterialization {
        receipt: BehaviorMaterializationReceipt {
            behavior_bond: projection.behavior_bond,
            projection_hash,
            behavior_hash: Some(behavior_hash),
            status: BehaviorMaterializationStatus::Materialized,
            validation,
        },
        bond: Some(bond),
    }
}

/// Compiles a successfully materialized bond while pinning the source local
/// projection hash into the compilation receipt.
pub fn compile_materialized_behavior(
    materialization: &BehaviorMaterialization,
) -> Option<BehaviorCompilation> {
    let bond = materialization.bond.as_ref()?;
    if materialization.receipt.status != BehaviorMaterializationStatus::Materialized
        || !materialization.receipt.validation.valid
        || materialization.receipt.projection_hash
            != materialization.receipt.validation.projection_hash
        || materialization.receipt.behavior_hash.as_deref()
            != Some(canonical_behavior_hash(bond).as_str())
    {
        return None;
    }
    let mut compilation = compile_behavior_bond(bond);
    compilation.receipt.projection_hash = Some(materialization.receipt.projection_hash.clone());
    Some(compilation)
}

pub fn canonical_hash(code: &CodeDefinition) -> Result<String, CompileError> {
    let bytes = serde_json::to_vec(code).expect("CodeDefinition serialization is infallible");
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub fn canonical_trigger_subscription_hash(subscription: &TriggerSubscription) -> String {
    canonical_serialized_hash(subscription)
}

pub fn canonical_trigger_event_hash(event: &TriggerEvent) -> String {
    canonical_serialized_hash(event)
}

pub fn canonical_execution_request_hash(request: &ExecutionRequest) -> String {
    canonical_serialized_hash(request)
}

fn canonical_serialized_hash(value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).expect("trigger contract serialization is infallible");
    hex::encode(Sha256::digest(bytes))
}

/// Validates only the generic, bounded trigger contract.
///
/// Event filters and behavior policy remain graph-owned CodeDefinitions.
pub fn validate_trigger_subscription(
    subscription: &TriggerSubscription,
) -> TriggerValidationReport {
    let mut issues = Vec::new();
    if subscription.contract_version != TRIGGER_CONTRACT_VERSION {
        issues.push(TriggerIssue::UnsupportedContractVersion {
            observed: subscription.contract_version,
        });
    }
    if !subscription.enabled {
        issues.push(TriggerIssue::DisabledSubscription);
    }
    if subscription.event_kinds.is_empty() {
        issues.push(TriggerIssue::MissingEventKind);
    }
    let mut event_kinds = BTreeSet::new();
    for kind in &subscription.event_kinds {
        if !event_kinds.insert(*kind) {
            issues.push(TriggerIssue::DuplicateEventKind { kind: *kind });
        }
    }
    if !is_sha256(&subscription.code_hash) {
        issues.push(TriggerIssue::InvalidCodeHash);
    }
    if subscription.idempotency_namespace.trim().is_empty() {
        issues.push(TriggerIssue::EmptyIdempotencyNamespace);
    }
    for (field, value) in [
        (
            "max_event_age_ticks",
            subscription.max_event_age_ticks as u64,
        ),
        ("fuel", subscription.budgets.fuel),
        ("max_mutations", subscription.budgets.max_mutations as u64),
        ("max_ticks", subscription.budgets.max_ticks as u64),
        (
            "max_causal_depth",
            subscription.controls.max_causal_depth as u64,
        ),
        (
            "max_firings_per_tick",
            subscription.controls.max_firings_per_tick as u64,
        ),
    ] {
        if value == 0 {
            issues.push(TriggerIssue::ZeroBudget {
                field: field.into(),
            });
        }
    }
    TriggerValidationReport {
        subscription: subscription.subscription,
        valid: issues.is_empty(),
        issues,
    }
}

/// Deterministically converts one immutable event into one pinned execution
/// request without consulting process state or the Store.
pub fn build_execution_request(
    subscription: &TriggerSubscription,
    event: &TriggerEvent,
    starting_universe_revision: Revision,
    issued_at_tick: Tick,
) -> ExecutionRequestReceipt {
    let mut issues = validate_trigger_subscription(subscription).issues;
    if event.event_id.trim().is_empty() {
        issues.push(TriggerIssue::EmptyEventId);
    }
    if !subscription.event_kinds.contains(&event.kind) {
        issues.push(TriggerIssue::UnsupportedEvent { kind: event.kind });
    }
    if event.observed_at.0 < event.occurred_at.0 {
        issues.push(TriggerIssue::EventObservedBeforeOccurrence);
    }
    if event.observed_at.0 > issued_at_tick.0 {
        issues.push(TriggerIssue::EventFromFuture {
            observed_at: event.observed_at,
            issued_at: issued_at_tick,
        });
    } else {
        let age_ticks = issued_at_tick.0 - event.observed_at.0;
        if age_ticks > subscription.max_event_age_ticks as u64 {
            issues.push(TriggerIssue::StaleEvent {
                age_ticks,
                maximum_ticks: subscription.max_event_age_ticks,
            });
        }
    }
    let evidence_state = trigger_evidence_state(&event.evidence);
    let evidence_allowed = match subscription.evidence_requirement {
        TriggerEvidenceRequirement::Measured => evidence_state == EpistemicState::Measured,
        TriggerEvidenceRequirement::ObservedOrMeasured => {
            matches!(
                evidence_state,
                EpistemicState::Observed | EpistemicState::Measured
            )
        }
    };
    if !evidence_allowed {
        issues.push(TriggerIssue::EventEvidenceUnavailable {
            state: evidence_state,
        });
    }

    let mut request_ids = BTreeSet::new();
    for hop in &event.causal_ancestry {
        if !request_ids.insert(hop.request_id.clone()) {
            issues.push(TriggerIssue::DuplicateCausalRequest {
                request_id: hop.request_id.clone(),
            });
        }
    }
    if event.causal_ancestry.iter().any(|hop| {
        hop.subscription == subscription.subscription
            && hop.subscription_revision == subscription.revision
    }) {
        issues.push(TriggerIssue::CausalCycle {
            subscription: subscription.subscription,
        });
    }
    let causal_depth = u16::try_from(event.causal_ancestry.len())
        .ok()
        .and_then(|depth| depth.checked_add(1));
    match causal_depth {
        Some(depth) if depth > subscription.controls.max_causal_depth => {
            issues.push(TriggerIssue::CausalDepthExceeded {
                depth,
                maximum: subscription.controls.max_causal_depth,
            });
        }
        None => issues.push(TriggerIssue::CausalDepthExceeded {
            depth: u16::MAX,
            maximum: subscription.controls.max_causal_depth,
        }),
        _ => {}
    }
    let deadline_tick = issued_at_tick
        .0
        .checked_add(subscription.budgets.max_ticks as u64)
        .map(Tick);
    if deadline_tick.is_none() {
        issues.push(TriggerIssue::DeadlineOverflow);
    }

    if !issues.is_empty() {
        return ExecutionRequestReceipt {
            subscription: subscription.subscription,
            subscription_revision: subscription.revision,
            event_id: event.event_id.clone(),
            idempotency_key: None,
            request_hash: None,
            state: Epistemic::Measured(ExecutionRequestState::Rejected),
            issues,
            request: None,
        };
    }

    let causal_depth = causal_depth.expect("validated causal depth exists");
    let deadline_tick = deadline_tick.expect("validated deadline exists");
    let idempotency_hash = canonical_serialized_hash(&(
        canonical_trigger_subscription_hash(subscription),
        canonical_trigger_event_hash(event),
    ));
    let idempotency_key = format!(
        "{}:{idempotency_hash}",
        subscription.idempotency_namespace.trim()
    );
    let identity_hash =
        canonical_serialized_hash(&(&idempotency_key, starting_universe_revision, issued_at_tick));
    let request = ExecutionRequest {
        contract_version: TRIGGER_CONTRACT_VERSION,
        request_id: identity_hash,
        idempotency_key: idempotency_key.clone(),
        subscription: subscription.subscription,
        subscription_revision: subscription.revision,
        code_definition: subscription.code_definition,
        code_revision: subscription.code_revision,
        code_hash: subscription.code_hash.clone(),
        starting_universe_revision,
        issued_at_tick,
        deadline_tick,
        trigger: event.clone(),
        causal_depth,
        budgets: subscription.budgets.clone(),
    };
    let request_hash = canonical_execution_request_hash(&request);
    ExecutionRequestReceipt {
        subscription: subscription.subscription,
        subscription_revision: subscription.revision,
        event_id: event.event_id.clone(),
        idempotency_key: Some(idempotency_key),
        request_hash: Some(request_hash),
        state: Epistemic::Measured(ExecutionRequestState::Accepted),
        issues,
        request: Some(request),
    }
}

fn trigger_evidence_state(
    evidence: &Epistemic<universe_ir::TriggerEventPayload>,
) -> EpistemicState {
    match evidence {
        Epistemic::Observed(_) => EpistemicState::Observed,
        Epistemic::Measured(_) => EpistemicState::Measured,
        Epistemic::KnownAbsent => EpistemicState::KnownAbsent,
        Epistemic::Unknown => EpistemicState::Unknown,
        Epistemic::NotMeasured => EpistemicState::NotMeasured,
        Epistemic::MeasurementFailed { .. } => EpistemicState::MeasurementFailed,
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn validate(code: &CodeDefinition) -> Result<(), CompileError> {
    if code.ir_version != IR_VERSION {
        return Err(CompileError::UnsupportedVersion(code.ir_version));
    }
    if code.operators.is_empty() {
        return Err(CompileError::Empty);
    }
    let operator_count = code.operators.len();
    let mut successors = vec![Vec::new(); operator_count];
    let mut globally_assigned = BTreeSet::new();
    for (index, op) in code.operators.iter().enumerate() {
        match op {
            Operator::QueryOpen { spec, .. }
                if spec.budget.max_entities == 0
                    || spec.budget.max_relations == 0
                    || spec.budget.max_depth == 0
                    || spec.timeout_ticks == 0 =>
            {
                return Err(CompileError::ZeroBound(index));
            }
            Operator::FilterTruthy { max_items: 0, .. }
            | Operator::SelectMembers { max_items: 0, .. }
            | Operator::OrderByPreference { max_items: 0, .. }
            | Operator::TopK { limit: 0, .. }
            | Operator::Hydrate { max_items: 0, .. }
            | Operator::Hydrate { max_bytes: 0, .. } => {
                return Err(CompileError::ZeroBound(index));
            }
            Operator::EvidenceAll { inputs, .. } if inputs.is_empty() => {
                return Err(CompileError::ZeroBound(index));
            }
            Operator::CapabilityCall { capability, .. }
                if !code.required_capabilities.contains(capability) =>
            {
                return Err(CompileError::UndeclaredCapability {
                    operator: index,
                    capability: capability.clone(),
                });
            }
            Operator::Branch {
                true_next,
                false_next,
                ..
            } => {
                for target in [*true_next, *false_next] {
                    let target_index = target as usize;
                    if target_index >= operator_count {
                        return Err(CompileError::InvalidBranchTarget {
                            operator: index,
                            target,
                        });
                    }
                    if target_index <= index {
                        return Err(CompileError::UnboundedCycle {
                            operator: index,
                            target,
                        });
                    }
                    successors[index].push(target_index);
                }
            }
            Operator::BranchOnEvidence {
                observed_next,
                measured_next,
                known_absent_next,
                unknown_next,
                not_measured_next,
                measurement_failed_next,
                ..
            } => {
                for target in [
                    *observed_next,
                    *measured_next,
                    *known_absent_next,
                    *unknown_next,
                    *not_measured_next,
                    *measurement_failed_next,
                ] {
                    let target_index = target as usize;
                    if target_index >= operator_count {
                        return Err(CompileError::InvalidBranchTarget {
                            operator: index,
                            target,
                        });
                    }
                    if target_index <= index {
                        return Err(CompileError::UnboundedCycle {
                            operator: index,
                            target,
                        });
                    }
                    successors[index].push(target_index);
                }
            }
            _ => {}
        }
        if let Some(output) = op.output() {
            if !globally_assigned.insert(output) {
                return Err(CompileError::DuplicateAssignment(output));
            }
        }
        if !matches!(
            op,
            Operator::Branch { .. } | Operator::BranchOnEvidence { .. } | Operator::Return { .. }
        ) && index + 1 < operator_count
        {
            successors[index].push(index + 1);
        }
    }

    let mut reachable = vec![false; operator_count];
    let mut frontier = vec![0usize];
    while let Some(index) = frontier.pop() {
        if reachable[index] {
            continue;
        }
        reachable[index] = true;
        frontier.extend(successors[index].iter().copied());
    }
    if let Some(index) = reachable.iter().position(|reachable| !reachable) {
        return Err(CompileError::UnreachableOperator(index));
    }

    let mut predecessors = vec![Vec::new(); operator_count];
    for (source, targets) in successors.iter().enumerate() {
        for target in targets {
            predecessors[*target].push(source);
        }
    }
    let mut assigned_after = vec![BTreeSet::new(); operator_count];
    for (index, op) in code.operators.iter().enumerate() {
        let mut assigned_before = if index == 0 {
            BTreeSet::new()
        } else {
            let mut incoming = predecessors[index].iter();
            let first = incoming
                .next()
                .expect("reachable non-entry operator has a predecessor");
            let mut intersection = assigned_after[*first].clone();
            for predecessor in incoming {
                intersection = intersection
                    .intersection(&assigned_after[*predecessor])
                    .copied()
                    .collect();
            }
            intersection
        };
        for input in op.inputs() {
            if !assigned_before.contains(&input) {
                return Err(CompileError::ReadBeforeAssignment(input));
            }
        }
        if let Some(output) = op.output() {
            assigned_before.insert(output);
        }
        assigned_after[index] = assigned_before;
    }

    let mut returns_on_all_paths = vec![false; operator_count];
    for index in (0..operator_count).rev() {
        returns_on_all_paths[index] = matches!(code.operators[index], Operator::Return { .. })
            || (!successors[index].is_empty()
                && successors[index]
                    .iter()
                    .all(|successor| returns_on_all_paths[*successor]));
    }
    if !returns_on_all_paths[0] {
        return Err(CompileError::InvalidReturn);
    }
    Ok(())
}

pub fn compile(code: &CodeDefinition) -> Result<Bytecode, CompileError> {
    validate(code)?;
    let mut capabilities = code.required_capabilities.clone();
    capabilities.sort();
    capabilities.dedup();
    Ok(Bytecode {
        ir_version: code.ir_version,
        code_revision: code.revision,
        canonical_hash: canonical_hash(code)?,
        required_capabilities: capabilities,
        instructions: code.operators.clone(),
        source_nodes: (0..code.operators.len() as u32).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_ir::{
        BehaviorBond, BehaviorBondProperties, BehaviorGraphProjection, BehaviorGraphRelation,
        BehaviorPhysicalProfileContent, BehaviorProjectionReadEvidence, BehaviorVocabulary,
        CausalHop, TriggerEventKind, TriggerEventPayload, Value, IR_VERSION,
    };

    fn trigger_subscription_fixture() -> TriggerSubscription {
        serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/trigger-subscription.json"
        ))
        .unwrap()
    }

    fn trigger_event(evidence: Epistemic<TriggerEventPayload>) -> TriggerEvent {
        TriggerEvent {
            event_id: "event-7".into(),
            kind: TriggerEventKind::LocalObservation,
            source_revision: Revision(12),
            occurred_at: Tick(20),
            observed_at: Tick(21),
            evidence,
            causal_ancestry: vec![],
        }
    }

    fn measured_trigger_payload() -> Epistemic<TriggerEventPayload> {
        Epistemic::Measured(TriggerEventPayload {
            subject: Some(EntityKey(0x6003)),
            fields: BTreeMap::from([("value".into(), Value::Integer(7))]),
            receipt_hash: None,
        })
    }

    #[test]
    fn trigger_contract_builds_one_content_addressed_pinned_request() {
        let subscription = trigger_subscription_fixture();
        let event = trigger_event(measured_trigger_payload());
        let receipt = build_execution_request(&subscription, &event, Revision(12), Tick(22));
        assert_eq!(
            receipt.state,
            Epistemic::Measured(ExecutionRequestState::Accepted)
        );
        assert!(receipt.issues.is_empty());
        let request = receipt.request.as_ref().unwrap();
        assert_eq!(request.code_revision, subscription.code_revision);
        assert_eq!(request.code_hash, subscription.code_hash);
        assert_eq!(request.causal_depth, 1);
        assert_eq!(request.deadline_tick, Tick(25));
        assert_eq!(
            receipt.request_hash.as_deref(),
            Some(canonical_execution_request_hash(request).as_str())
        );
        assert_eq!(
            build_execution_request(&subscription, &event, Revision(12), Tick(22)),
            receipt
        );
        let retry = build_execution_request(&subscription, &event, Revision(12), Tick(23));
        assert_eq!(retry.idempotency_key, receipt.idempotency_key);
        assert_ne!(
            retry.request.as_ref().unwrap().request_id,
            request.request_id
        );
    }

    #[test]
    fn trigger_contract_rejects_zero_or_missing_budgets() {
        let mut subscription = trigger_subscription_fixture();
        subscription.budgets.fuel = 0;
        subscription.controls.max_firings_per_tick = 0;
        let report = validate_trigger_subscription(&subscription);
        assert_eq!(
            report.issues,
            vec![
                TriggerIssue::ZeroBudget {
                    field: "fuel".into()
                },
                TriggerIssue::ZeroBudget {
                    field: "max_firings_per_tick".into()
                }
            ]
        );

        let fixture = include_str!("../../../fixtures/graph-ir/trigger-subscription.json");
        let mut value: serde_json::Value = serde_json::from_str(fixture).unwrap();
        value.as_object_mut().unwrap().remove("budgets");
        assert!(serde_json::from_value::<TriggerSubscription>(value).is_err());
    }

    #[test]
    fn trigger_contract_rejects_cycle_and_causal_depth() {
        let mut subscription = trigger_subscription_fixture();
        subscription.controls.max_causal_depth = 1;
        let mut event = trigger_event(measured_trigger_payload());
        event.causal_ancestry.push(CausalHop {
            subscription: subscription.subscription,
            subscription_revision: subscription.revision,
            event_id: "ancestor-event".into(),
            request_id: "ancestor-request".into(),
        });
        let receipt = build_execution_request(&subscription, &event, Revision(12), Tick(22));
        assert_eq!(
            receipt.state,
            Epistemic::Measured(ExecutionRequestState::Rejected)
        );
        assert_eq!(
            receipt.issues,
            vec![
                TriggerIssue::CausalCycle {
                    subscription: subscription.subscription
                },
                TriggerIssue::CausalDepthExceeded {
                    depth: 2,
                    maximum: 1
                }
            ]
        );
        assert!(receipt.request.is_none());
    }

    #[test]
    fn descendant_ancestry_closes_trigger_to_receipt_causal_chain() {
        // The chain: a root event materializes an accepted request; a downstream
        // event caused by that execution must carry the request's descendant
        // ancestry so the same subscription firing again is caught as a cycle.
        let subscription = trigger_subscription_fixture();
        let root_event = trigger_event(measured_trigger_payload());
        let first = build_execution_request(&subscription, &root_event, Revision(12), Tick(22));
        assert_eq!(
            first.state,
            Epistemic::Measured(ExecutionRequestState::Accepted)
        );
        let first_request = first.request.as_ref().unwrap();

        // A downstream event inherits the closed structured ancestry.
        let mut downstream = trigger_event(measured_trigger_payload());
        downstream.event_id = "event-8".into();
        downstream.causal_ancestry = first_request.descendant_causal_ancestry();
        assert_eq!(downstream.causal_ancestry.len(), 1);
        assert_eq!(downstream.causal_ancestry[0], first_request.execution_hop());

        // Re-firing the same subscription off that downstream event is a cycle,
        // proving the chain is preserved end to end rather than dropped.
        let second = build_execution_request(&subscription, &downstream, Revision(12), Tick(23));
        assert_eq!(
            second.state,
            Epistemic::Measured(ExecutionRequestState::Rejected)
        );
        assert!(second.issues.contains(&TriggerIssue::CausalCycle {
            subscription: subscription.subscription
        }));

        // The same ancestry projects into deterministic write-set tokens so the
        // opaque Vec<String> commit ancestry can carry the trigger identity.
        let tokens = first_request.descendant_causal_tokens();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], first_request.execution_hop().canonical_token());
    }

    #[test]
    fn trigger_contract_preserves_unavailable_and_failed_event_evidence() {
        let subscription = trigger_subscription_fixture();
        for (evidence, state) in [
            (
                Epistemic::Observed(TriggerEventPayload {
                    subject: None,
                    fields: BTreeMap::new(),
                    receipt_hash: None,
                }),
                EpistemicState::Observed,
            ),
            (Epistemic::KnownAbsent, EpistemicState::KnownAbsent),
            (Epistemic::Unknown, EpistemicState::Unknown),
            (Epistemic::NotMeasured, EpistemicState::NotMeasured),
            (
                Epistemic::MeasurementFailed {
                    reason: "sensor failure".into(),
                },
                EpistemicState::MeasurementFailed,
            ),
        ] {
            let receipt = build_execution_request(
                &subscription,
                &trigger_event(evidence),
                Revision(12),
                Tick(22),
            );
            assert_eq!(
                receipt.issues,
                vec![TriggerIssue::EventEvidenceUnavailable { state }]
            );
            assert!(receipt.request.is_none());
        }
    }

    #[test]
    fn trigger_contract_accepts_observed_only_when_graph_contract_allows_it() {
        let mut subscription = trigger_subscription_fixture();
        subscription.evidence_requirement = TriggerEvidenceRequirement::ObservedOrMeasured;
        let event = trigger_event(Epistemic::Observed(TriggerEventPayload {
            subject: None,
            fields: BTreeMap::new(),
            receipt_hash: None,
        }));
        let receipt = build_execution_request(&subscription, &event, Revision(12), Tick(22));
        assert_eq!(
            receipt.state,
            Epistemic::Measured(ExecutionRequestState::Accepted)
        );
    }

    #[test]
    fn trigger_contract_rejects_stale_or_unsupported_event() {
        let mut subscription = trigger_subscription_fixture();
        subscription.event_kinds = vec![TriggerEventKind::EffectReceipt];
        let event = trigger_event(measured_trigger_payload());
        let receipt = build_execution_request(&subscription, &event, Revision(12), Tick(40));
        assert_eq!(
            receipt.issues,
            vec![
                TriggerIssue::UnsupportedEvent {
                    kind: TriggerEventKind::LocalObservation
                },
                TriggerIssue::StaleEvent {
                    age_ticks: 19,
                    maximum_ticks: 8
                }
            ]
        );
        assert_eq!(
            receipt.state,
            Epistemic::Measured(ExecutionRequestState::Rejected)
        );
    }

    fn behavior_bond_fixture() -> BehaviorBond {
        serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/behavior-bond.json"
        ))
        .unwrap()
    }

    fn projection_fixture() -> BehaviorGraphProjection {
        let bond = behavior_bond_fixture();
        let behavior_bond = bond.bond;
        let vocabulary = BehaviorVocabulary {
            source_atom: EntityKey(0x4001),
            target_atom: EntityKey(0x4002),
            uses_predicate: EntityKey(0x4003),
            uses_profile: EntityKey(0x4004),
            has_logic_role: EntityKey(0x4005),
            gated_by: EntityKey(0x4006),
            serves_objective: EntityKey(0x4007),
            justified_by: EntityKey(0x4008),
        };
        let mut next_relation = 1u128;
        let mut relation = |predicate: EntityKey, target: EntityKey| {
            let value = BehaviorGraphRelation {
                relation: RelationKey(next_relation),
                source: behavior_bond,
                predicate,
                target,
            };
            next_relation += 1;
            value
        };
        let source = bond.source.unwrap();
        let target = bond.target.unwrap();
        let predicate = bond.predicate.unwrap();
        let profile = bond.profile.unwrap();
        let logic_role = bond.logic_role.unwrap();
        let objective = bond.objective.unwrap();
        let relations = [
            vec![
                relation(vocabulary.source_atom, source),
                relation(vocabulary.target_atom, target),
                relation(vocabulary.uses_predicate, predicate),
                relation(vocabulary.uses_profile, profile),
                relation(vocabulary.has_logic_role, logic_role),
                relation(vocabulary.serves_objective, objective),
            ],
            bond.gates
                .iter()
                .map(|gate| relation(vocabulary.gated_by, gate.gate))
                .collect(),
            bond.justifications
                .iter()
                .map(|justification| relation(vocabulary.justified_by, *justification))
                .collect(),
        ]
        .concat();
        let logic_binding = match bond.logic_binding.clone() {
            Epistemic::Measured(binding) => binding,
            _ => panic!("fixture logic binding must be measured"),
        };
        let profile_parameters = match bond.profile_parameters.clone() {
            Epistemic::Measured(parameters) => parameters,
            _ => panic!("fixture profile parameters must be measured"),
        };
        let mut contents = vec![
            ResolvedBehaviorNode {
                entity: behavior_bond,
                content_hash: "a".repeat(64),
                value: Epistemic::Measured(BehaviorResolvedContent::Bond(BehaviorBondProperties {
                    budgets: bond.budgets.unwrap(),
                    authority: bond.authority.clone().unwrap(),
                })),
            },
            ResolvedBehaviorNode {
                entity: predicate,
                content_hash: "b".repeat(64),
                value: Epistemic::Measured(BehaviorResolvedContent::Predicate(
                    OntologyBindingStatus::Active,
                )),
            },
            ResolvedBehaviorNode {
                entity: profile,
                content_hash: profile_parameters.profile_hash.clone(),
                value: Epistemic::Measured(BehaviorResolvedContent::PhysicalProfile(
                    BehaviorPhysicalProfileContent {
                        source_threshold: profile_parameters.source_threshold,
                        source_seed_energy: profile_parameters.source_seed_energy,
                        source_inhibition_threshold: profile_parameters.source_inhibition_threshold,
                        target_threshold: profile_parameters.target_threshold,
                        target_seed_energy: profile_parameters.target_seed_energy,
                        target_inhibition_threshold: profile_parameters.target_inhibition_threshold,
                        transfer_energy: profile_parameters.transfer_energy,
                    },
                )),
            },
            ResolvedBehaviorNode {
                entity: logic_role,
                content_hash: logic_binding.definition_hash.clone(),
                value: Epistemic::Measured(BehaviorResolvedContent::LogicRole(
                    universe_ir::BehaviorLogicContent {
                        kind: logic_binding.kind,
                    },
                )),
            },
        ];
        contents.extend(bond.gates.into_iter().enumerate().map(|(index, gate)| {
            ResolvedBehaviorNode {
                entity: gate.gate,
                content_hash: if index == 0 {
                    "c".repeat(64)
                } else {
                    "d".repeat(64)
                },
                value: Epistemic::Measured(BehaviorResolvedContent::Gate(BehaviorGateContent {
                    closure: gate.closure,
                    stale: gate.stale,
                    contradictory: gate.contradictory,
                })),
            }
        }));
        contents.push(ResolvedBehaviorNode {
            entity: objective,
            content_hash: "4".repeat(64),
            value: Epistemic::Measured(BehaviorResolvedContent::Objective),
        });
        contents.extend(
            bond.justifications
                .iter()
                .enumerate()
                .map(|(index, justification)| ResolvedBehaviorNode {
                    entity: *justification,
                    content_hash: hex::encode(Sha256::digest(format!("justification-{index}"))),
                    value: Epistemic::Measured(BehaviorResolvedContent::Justification),
                }),
        );
        let authority = bond.authority.unwrap();
        BehaviorGraphProjection {
            behavior_bond,
            vocabulary,
            relations,
            contents,
            local_read: Epistemic::Measured(BehaviorProjectionReadEvidence {
                origin: EntityKey(1),
                universe_revision: authority.universe_revision,
                max_entities: 32,
                max_relations: 64,
                max_content_bytes: 65_536,
                timeout_ticks: 8,
                complete_for_behavior: true,
                query_receipt_hash: "e".repeat(64),
                independent_readback_hash: "f".repeat(64),
                active_registry_hash: "1".repeat(64),
            }),
        }
    }

    fn branch_fixture() -> CodeDefinition {
        serde_json::from_str(include_str!("../../../fixtures/graph-ir/branch.json")).unwrap()
    }

    #[test]
    fn compilation_is_deterministic() {
        let code = CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(3),
            required_capabilities: vec![],
            operators: vec![
                Operator::Constant {
                    value: Value::Unit,
                    output: 0,
                },
                Operator::Return { value: 0 },
            ],
        };
        assert_eq!(compile(&code).unwrap(), compile(&code).unwrap());
    }

    fn evidence_branch(observed_next: u32) -> CodeDefinition {
        CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(5),
            required_capabilities: vec![],
            operators: vec![
                Operator::Constant {
                    value: Value::Epistemic(Epistemic::Unknown),
                    output: 0,
                },
                Operator::BranchOnEvidence {
                    input: 0,
                    observed_next,
                    measured_next: 2,
                    known_absent_next: 2,
                    unknown_next: 2,
                    not_measured_next: 2,
                    measurement_failed_next: 2,
                },
                Operator::Return { value: 0 },
            ],
        }
    }

    #[test]
    fn evidence_branch_validates_forward_targets_and_rejects_cycles() {
        assert!(compile(&evidence_branch(2)).is_ok());
        assert_eq!(
            validate(&evidence_branch(1)),
            Err(CompileError::UnboundedCycle {
                operator: 1,
                target: 1
            })
        );
        assert_eq!(
            validate(&evidence_branch(9)),
            Err(CompileError::InvalidBranchTarget {
                operator: 1,
                target: 9
            })
        );
    }

    #[test]
    fn rejects_read_before_assignment() {
        let code = CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(0),
            required_capabilities: vec![],
            operators: vec![Operator::Return { value: 9 }],
        };
        assert_eq!(validate(&code), Err(CompileError::ReadBeforeAssignment(9)));
    }

    #[test]
    fn graph_fixture_loads_validates_and_compiles() {
        let fixture = include_str!("../../../fixtures/graph-ir/minimal-read.json");
        let code: CodeDefinition = serde_json::from_str(fixture).unwrap();
        let artifact = compile(&code).unwrap();
        assert_eq!(artifact.instructions.len(), 17);
        assert_eq!(artifact.source_nodes, (0..17).collect::<Vec<_>>());
    }

    #[test]
    fn behavior_loop_health_fixture_owns_the_required_proof_set() {
        let code: CodeDefinition = serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/behavior-loop-health.json"
        ))
        .unwrap();
        let artifact = compile(&code).unwrap();
        let declared_inputs = code
            .operators
            .iter()
            .filter_map(|operator| match operator {
                Operator::Input { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            declared_inputs,
            BEHAVIOR_LOOP_HEALTH_INPUT_NAMES
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        );
        assert!(code
            .operators
            .iter()
            .any(|operator| matches!(operator, Operator::EvidenceAll { .. })));
        assert_eq!(artifact.instructions.len(), 39);
    }

    #[test]
    fn epistemic_all_requires_at_least_one_graph_selected_proof() {
        let code = CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(1),
            required_capabilities: Vec::new(),
            operators: vec![
                Operator::EvidenceAll {
                    inputs: Vec::new(),
                    output: 0,
                },
                Operator::Return { value: 0 },
            ],
        };
        assert_eq!(validate(&code), Err(CompileError::ZeroBound(0)));
    }

    #[test]
    fn forward_branch_fixture_validates_and_compiles() {
        let artifact = compile(&branch_fixture()).unwrap();
        assert_eq!(artifact.instructions.len(), 5);
    }

    #[test]
    fn branch_rejects_unbounded_cycles() {
        let mut code = branch_fixture();
        code.operators[2] = Operator::Branch {
            condition: 0,
            true_next: 2,
            false_next: 4,
        };
        assert_eq!(
            validate(&code),
            Err(CompileError::UnboundedCycle {
                operator: 2,
                target: 2,
            })
        );
    }

    #[test]
    fn branch_requires_registers_assigned_on_every_incoming_path() {
        let mut code = branch_fixture();
        code.operators.push(Operator::Return { value: 2 });
        code.operators[4] = Operator::Branch {
            condition: 0,
            true_next: 5,
            false_next: 5,
        };
        assert_eq!(validate(&code), Err(CompileError::ReadBeforeAssignment(2)));
    }

    #[test]
    fn behavior_bond_fixture_is_executable_shape() {
        let bond = behavior_bond_fixture();
        let report = validate_behavior_bond(&bond);
        assert!(report.valid);
        assert!(report.issues.is_empty());
        assert_eq!(report.behavior_hash.len(), 64);
        assert_eq!(report.authority, bond.authority);
        let encoded = serde_json::to_value(report).unwrap();
        assert_eq!(encoded["valid"], true);
        assert_eq!(
            serde_json::from_value::<BehaviorBondValidationReport>(encoded).unwrap(),
            validate_behavior_bond(&bond)
        );
    }

    #[test]
    fn behavior_bond_compiles_to_content_addressed_runtime_plan_and_receipt() {
        let bond = behavior_bond_fixture();
        let first = compile_behavior_bond(&bond);
        let second = compile_behavior_bond(&bond);
        assert_eq!(first, second);
        assert_eq!(first.receipt.status, BehaviorCompilationStatus::Compiled);
        assert_eq!(first.receipt.artifact_hash.as_deref().unwrap().len(), 64);

        let artifact = first.artifact.unwrap();
        assert_eq!(artifact.verify(), Ok(()));
        assert_eq!(
            first.receipt.artifact_hash.as_deref(),
            Some(artifact.artifact_hash.as_str())
        );
        assert_eq!(artifact.plan.behavior_bond, bond.bond);
        assert_eq!(artifact.plan.logic_kind, BehaviorLogicKind::Support);
        assert_eq!(artifact.plan.transfer_energy, 100);
        assert_eq!(artifact.plan.source.threshold, 100);
        assert_eq!(artifact.plan.target.inhibition_threshold, Some(100));
        assert_eq!(artifact.plan.budgets.max_atoms, 16);
        assert_eq!(
            artifact.plan.authority.mapping_hash,
            bond.authority.unwrap().mapping_hash
        );
        let receipt_bytes = serde_json::to_vec(&first.receipt).unwrap();
        assert_eq!(
            serde_json::from_slice::<BehaviorCompilationReceipt>(&receipt_bytes).unwrap(),
            first.receipt
        );
    }

    #[test]
    fn runtime_artifact_verification_rejects_tampering() {
        let mut artifact = compile_behavior_bond(&behavior_bond_fixture())
            .artifact
            .unwrap();
        artifact.plan.transfer_energy += 1;
        assert!(matches!(
            artifact.verify(),
            Err(RuntimeArtifactError::HashMismatch { .. })
        ));
    }

    #[test]
    fn semantic_collection_order_does_not_change_behavior_or_artifact_hash() {
        let mut reordered = behavior_bond_fixture();
        reordered.gates.reverse();
        let original = compile_behavior_bond(&behavior_bond_fixture());
        let reordered = compile_behavior_bond(&reordered);
        assert_eq!(
            original.receipt.behavior_hash,
            reordered.receipt.behavior_hash
        );
        assert_eq!(
            original.receipt.artifact_hash,
            reordered.receipt.artifact_hash
        );
    }

    #[test]
    fn behavior_bond_rejects_missing_role_without_predicate_dispatch() {
        let mut bond = behavior_bond_fixture();
        bond.logic_role = None;
        let compilation = compile_behavior_bond(&bond);
        assert!(compilation.artifact.is_none());
        assert_eq!(
            compilation.receipt.status,
            BehaviorCompilationStatus::Rejected
        );
        assert_eq!(
            compilation.receipt.validation.issues,
            vec![BehaviorBondIssue::MissingLogicRole]
        );
    }

    #[test]
    fn behavior_bond_rejects_unresolved_ontology_gap() {
        let mut bond = behavior_bond_fixture();
        bond.ontology_status = Epistemic::Measured(OntologyBindingStatus::UnresolvedGap);
        let report = validate_behavior_bond(&bond);
        assert!(!report.valid);
        assert_eq!(
            report.issues,
            vec![BehaviorBondIssue::UnresolvedOntologyGap]
        );
    }

    #[test]
    fn behavior_bond_gates_are_non_compensatory() {
        let mut bond = behavior_bond_fixture();
        bond.gates[1].closure = Epistemic::NotMeasured;
        let report = validate_behavior_bond(&bond);
        assert!(!report.valid);
        assert_eq!(
            report.issues,
            vec![BehaviorBondIssue::GateNotClosed {
                gate: bond.gates[1].gate,
                state: GateClosureState::NotMeasured,
            }]
        );
    }

    #[test]
    fn behavior_bond_does_not_treat_unmeasured_ontology_as_active() {
        let mut bond = behavior_bond_fixture();
        bond.ontology_status = Epistemic::NotMeasured;
        let report = validate_behavior_bond(&bond);
        assert!(!report.valid);
        assert_eq!(
            report.issues,
            vec![BehaviorBondIssue::OntologyBindingUnavailable {
                state: OntologyBindingEvidenceState::NotMeasured,
            }]
        );
    }

    #[test]
    fn behavior_bond_does_not_coerce_unknown_or_failed_bindings() {
        let mut unknown = behavior_bond_fixture();
        unknown.logic_binding = Epistemic::Unknown;
        assert_eq!(
            validate_behavior_bond(&unknown).issues,
            vec![BehaviorBondIssue::LogicBindingUnavailable {
                state: ResolvedBindingEvidenceState::Unknown,
            }]
        );

        let mut failed = behavior_bond_fixture();
        failed.profile_parameters = Epistemic::MeasurementFailed {
            reason: "profile hydration failed".into(),
        };
        assert_eq!(
            validate_behavior_bond(&failed).issues,
            vec![BehaviorBondIssue::ProfileParametersUnavailable {
                state: ResolvedBindingEvidenceState::MeasurementFailed,
            }]
        );
    }

    #[test]
    fn behavior_bond_rejects_role_energy_mismatch() {
        let mut bond = behavior_bond_fixture();
        let Epistemic::Measured(logic) = &mut bond.logic_binding else {
            panic!("fixture logic binding must be measured");
        };
        logic.kind = BehaviorLogicKind::Neutral;
        let report = validate_behavior_bond(&bond);
        assert_eq!(
            report.issues,
            vec![BehaviorBondIssue::LogicEnergyMismatch {
                kind: BehaviorLogicKind::Neutral,
                transfer_energy: 100,
            }]
        );
    }

    #[test]
    fn projection_materializes_relations_then_compiles_with_provenance() {
        let projection = projection_fixture();
        let materialization = materialize_behavior_bond(&projection);
        assert_eq!(
            materialization.receipt.status,
            BehaviorMaterializationStatus::Materialized
        );
        assert!(materialization.receipt.validation.valid);
        let compilation = compile_materialized_behavior(&materialization).unwrap();
        assert_eq!(
            compilation.receipt.projection_hash.as_deref(),
            Some(materialization.receipt.projection_hash.as_str())
        );
        assert_eq!(
            compilation.receipt.status,
            BehaviorCompilationStatus::Compiled
        );
    }

    #[test]
    fn missing_relation_is_only_claimed_after_complete_measured_read() {
        let mut complete = projection_fixture();
        complete
            .relations
            .retain(|relation| relation.predicate != complete.vocabulary.source_atom);
        let receipt = materialize_behavior_bond(&complete).receipt;
        assert_eq!(
            receipt.validation.issues,
            vec![BehaviorProjectionIssue::MissingRequiredRelation {
                role: BehaviorRelationRole::Source,
            }]
        );

        let mut incomplete = complete;
        let Epistemic::Measured(read) = &mut incomplete.local_read else {
            panic!("fixture read must be measured");
        };
        read.complete_for_behavior = false;
        let receipt = materialize_behavior_bond(&incomplete).receipt;
        assert_eq!(
            receipt.validation.issues,
            vec![BehaviorProjectionIssue::LocalReadIncomplete]
        );
    }

    #[test]
    fn projection_rejects_exact_relation_cardinality_violation() {
        let mut projection = projection_fixture();
        projection.relations.push(BehaviorGraphRelation {
            relation: RelationKey(99),
            source: projection.behavior_bond,
            predicate: projection.vocabulary.source_atom,
            target: EntityKey(0x9999),
        });
        let receipt = materialize_behavior_bond(&projection).receipt;
        assert_eq!(
            receipt.validation.issues,
            vec![BehaviorProjectionIssue::InvalidCardinality {
                role: BehaviorRelationRole::Source,
                expected: "exactly_one".into(),
                observed: 2,
            }]
        );
    }

    #[test]
    fn objective_and_justification_content_must_be_measured() {
        let mut missing = projection_fixture();
        let justification = missing
            .relations
            .iter()
            .find(|relation| relation.predicate == missing.vocabulary.justified_by)
            .unwrap()
            .target;
        missing
            .contents
            .retain(|content| content.entity != justification);
        assert_eq!(
            materialize_behavior_bond(&missing)
                .receipt
                .validation
                .issues,
            vec![BehaviorProjectionIssue::MissingResolvedContent {
                entity: justification,
            }]
        );

        let mut unavailable = projection_fixture();
        let objective = unavailable
            .relations
            .iter()
            .find(|relation| relation.predicate == unavailable.vocabulary.serves_objective)
            .unwrap()
            .target;
        unavailable
            .contents
            .iter_mut()
            .find(|content| content.entity == objective)
            .unwrap()
            .value = Epistemic::NotMeasured;
        assert_eq!(
            materialize_behavior_bond(&unavailable)
                .receipt
                .validation
                .issues,
            vec![BehaviorProjectionIssue::ResolvedContentUnavailable {
                entity: objective,
                state: EvidenceInputState::NotMeasured,
            }]
        );
    }

    #[test]
    fn resolved_ontology_gap_materializes_but_cannot_compile() {
        let mut projection = projection_fixture();
        let predicate = projection
            .relations
            .iter()
            .find(|relation| relation.predicate == projection.vocabulary.uses_predicate)
            .unwrap()
            .target;
        let content = projection
            .contents
            .iter_mut()
            .find(|content| content.entity == predicate)
            .unwrap();
        content.value = Epistemic::Measured(BehaviorResolvedContent::Predicate(
            OntologyBindingStatus::UnresolvedGap,
        ));
        let materialization = materialize_behavior_bond(&projection);
        assert_eq!(
            materialization.receipt.status,
            BehaviorMaterializationStatus::Materialized
        );
        let compilation = compile_materialized_behavior(&materialization).unwrap();
        assert_eq!(
            compilation.receipt.status,
            BehaviorCompilationStatus::Rejected
        );
        assert_eq!(
            compilation.receipt.validation.issues,
            vec![BehaviorBondIssue::UnresolvedOntologyGap]
        );
    }
}
