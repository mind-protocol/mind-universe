//! Canonical, graph-materialized instruction representation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use universe_core::{ContentPtr, EntityKey, Epistemic, RelationKey, Revision, Tick};
use universe_query::QueryBudget;

pub const IR_VERSION: u16 = 0;
pub const TRIGGER_CONTRACT_VERSION: u16 = 0;
pub type Register = u16;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Unit,
    Bool(bool),
    Integer(i64),
    Text(String),
    Entity(EntityKey),
    Content(ContentPtr),
    Epistemic(Epistemic<Box<Value>>),
    EpistemicState(EpistemicState),
    List(Vec<Value>),
    Record(BTreeMap<String, Value>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicState {
    Observed,
    Measured,
    KnownAbsent,
    Unknown,
    NotMeasured,
    MeasurementFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QuerySpec {
    pub origin: Register,
    pub selector: Register,
    pub budget: QueryBudget,
    pub timeout_ticks: u32,
    pub allow_approximate: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonKind {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanBinaryKind {
    And,
    Or,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operator {
    Input {
        name: String,
        output: Register,
    },
    Constant {
        value: Value,
        output: Register,
    },
    Compare {
        left: Register,
        right: Register,
        kind: ComparisonKind,
        output: Register,
    },
    BooleanBinary {
        left: Register,
        right: Register,
        kind: BooleanBinaryKind,
        output: Register,
    },
    BooleanNot {
        input: Register,
        output: Register,
    },
    EvidenceState {
        input: Register,
        output: Register,
    },
    EvidenceValue {
        input: Register,
        output: Register,
    },
    EvidenceCompare {
        left: Register,
        right: Register,
        kind: ComparisonKind,
        output: Register,
    },
    /// Non-empty, non-compensatory conjunction of epistemic booleans.
    ///
    /// The program selects the required inputs. Native semantics only preserve
    /// evidence state and compute boolean conjunction when every input is
    /// measured.
    EvidenceAll {
        inputs: Vec<Register>,
        output: Register,
    },
    Branch {
        condition: Register,
        true_next: u32,
        false_next: u32,
    },
    /// Explicit six-way control path over an epistemic value's state.
    ///
    /// Each destination is graph data. Unavailable evidence
    /// (`KnownAbsent`/`Unknown`/`NotMeasured`/`MeasurementFailed`) selects its
    /// own named successor instead of being coerced into a boolean or trapped,
    /// preserving the epistemic distinction in control flow.
    BranchOnEvidence {
        input: Register,
        observed_next: u32,
        measured_next: u32,
        known_absent_next: u32,
        unknown_next: u32,
        not_measured_next: u32,
        measurement_failed_next: u32,
    },
    QueryOpen {
        spec: QuerySpec,
        output: Register,
    },
    QueryAwait {
        handle: Register,
        output: Register,
    },
    FollowOne {
        source: Register,
        predicate: Register,
        output: Register,
    },
    EntitySymbol {
        entity: Register,
        output: Register,
    },
    SelectMembers {
        input: Register,
        allowed: Register,
        max_items: u32,
        output: Register,
    },
    OrderByPreference {
        input: Register,
        preference: Register,
        max_items: u32,
        output: Register,
    },
    FilterTruthy {
        input: Register,
        field: String,
        max_items: u32,
        output: Register,
    },
    TopK {
        input: Register,
        score_field: String,
        limit: u32,
        output: Register,
    },
    Hydrate {
        input: Register,
        max_items: u32,
        max_bytes: u32,
        output: Register,
    },
    /// Calls one explicitly declared, host-provided primitive.
    ///
    /// The capability name and all policy remain graph data. The VM only
    /// enforces declaration and routes the immutable input value to the host.
    CapabilityCall {
        capability: String,
        input: Register,
        output: Register,
    },
    MakeRecord {
        fields: Vec<(String, Register)>,
        output: Register,
    },
    Propose {
        command: Register,
        output: Register,
    },
    /// Bounded subroutine invocation with a REQUIRED runtime call-depth budget.
    ///
    /// Control transfers forward to `target`, the entry of a callee region that
    /// ends in `Return`; the callee's returned value is bound into `output` and
    /// control resumes at the following operator. Registers are shared with the
    /// caller: the callee may read any register the caller has already assigned,
    /// but `output` is defined only once the call returns.
    ///
    /// `max_depth` is graph data and caps the number of simultaneously live call
    /// frames. Exceeding it is a deterministic trap, never a silent truncation.
    /// Because `target` must point forward the static call graph is a DAG, so
    /// termination is structurally guaranteed independently of the budget; the
    /// budget bounds live stack depth for nested calls.
    Call {
        target: u32,
        output: Register,
        max_depth: u32,
    },
    Return {
        value: Register,
    },
}

impl Operator {
    pub fn output(&self) -> Option<Register> {
        match self {
            Self::Input { output, .. }
            | Self::Constant { output, .. }
            | Self::Compare { output, .. }
            | Self::BooleanBinary { output, .. }
            | Self::BooleanNot { output, .. }
            | Self::EvidenceState { output, .. }
            | Self::EvidenceValue { output, .. }
            | Self::EvidenceCompare { output, .. }
            | Self::EvidenceAll { output, .. }
            | Self::QueryOpen { output, .. }
            | Self::QueryAwait { output, .. }
            | Self::FollowOne { output, .. }
            | Self::EntitySymbol { output, .. }
            | Self::SelectMembers { output, .. }
            | Self::OrderByPreference { output, .. }
            | Self::FilterTruthy { output, .. }
            | Self::TopK { output, .. }
            | Self::Hydrate { output, .. }
            | Self::CapabilityCall { output, .. }
            | Self::MakeRecord { output, .. }
            | Self::Propose { output, .. }
            | Self::Call { output, .. } => Some(*output),
            Self::Branch { .. } | Self::BranchOnEvidence { .. } | Self::Return { .. } => None,
        }
    }

    pub fn inputs(&self) -> Vec<Register> {
        match self {
            Self::Input { .. } | Self::Constant { .. } | Self::Call { .. } => vec![],
            Self::Compare { left, right, .. } | Self::BooleanBinary { left, right, .. } => {
                vec![*left, *right]
            }
            Self::BooleanNot { input, .. }
            | Self::EvidenceState { input, .. }
            | Self::EvidenceValue { input, .. } => vec![*input],
            Self::EvidenceCompare { left, right, .. } => vec![*left, *right],
            Self::EvidenceAll { inputs, .. } => inputs.clone(),
            Self::Branch { condition, .. } => vec![*condition],
            Self::BranchOnEvidence { input, .. } => vec![*input],
            Self::QueryOpen { spec, .. } => vec![spec.origin, spec.selector],
            Self::QueryAwait { handle, .. } => vec![*handle],
            Self::FollowOne {
                source, predicate, ..
            } => vec![*source, *predicate],
            Self::EntitySymbol { entity, .. } => vec![*entity],
            Self::SelectMembers { input, allowed, .. } => vec![*input, *allowed],
            Self::OrderByPreference {
                input, preference, ..
            } => vec![*input, *preference],
            Self::FilterTruthy { input, .. }
            | Self::TopK { input, .. }
            | Self::Hydrate { input, .. }
            | Self::CapabilityCall { input, .. } => vec![*input],
            Self::MakeRecord { fields, .. } => {
                fields.iter().map(|(_, register)| *register).collect()
            }
            Self::Propose { command, .. } => vec![*command],
            Self::Return { value } => vec![*value],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeDefinition {
    pub ir_version: u16,
    pub revision: Revision,
    pub required_capabilities: Vec<String>,
    pub operators: Vec<Operator>,
}

/// Generic Universe event families that may activate graph-owned subscriptions.
///
/// These variants describe bootstrap event shapes only. Predicate meaning,
/// filtering, objectives, and workflow policy remain graph data.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerEventKind {
    ApprovedChangeSet,
    LocalObservation,
    EffectReceipt,
    HealthFailure,
    ScheduledTick,
    OperatorRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerEvidenceRequirement {
    Measured,
    ObservedOrMeasured,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerBudgets {
    pub fuel: u64,
    pub max_mutations: u32,
    pub max_ticks: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerControls {
    pub cooldown_ticks: u32,
    pub debounce_ticks: u32,
    pub max_causal_depth: u16,
    pub max_firings_per_tick: u32,
}

/// Versioned graph authority selecting a pinned CodeDefinition for later
/// executions. A later subscription or code revision cannot mutate an already
/// constructed `ExecutionRequest`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerSubscription {
    pub contract_version: u16,
    pub subscription: EntityKey,
    pub revision: Revision,
    pub enabled: bool,
    pub event_kinds: Vec<TriggerEventKind>,
    pub code_definition: EntityKey,
    pub code_revision: Revision,
    pub code_hash: String,
    pub evidence_requirement: TriggerEvidenceRequirement,
    pub max_event_age_ticks: u32,
    pub budgets: TriggerBudgets,
    pub controls: TriggerControls,
    pub idempotency_namespace: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CausalHop {
    pub subscription: EntityKey,
    pub subscription_revision: Revision,
    pub event_id: String,
    pub request_id: String,
}

impl CausalHop {
    /// Deterministic, collision-resistant token that projects one structured
    /// causal hop into the opaque `Vec<String>` causal ancestry carried by
    /// `UniverseWriteSet` and `CommitReceipt`, so the trigger identity chain can
    /// close through the write set without changing that contract type.
    ///
    /// The token is unambiguous: `subscription` renders as fixed 32-hex, the
    /// numeric revision has no delimiter collisions, and the variable-length
    /// `event_id` is length-prefixed. The final `request_id` needs no length
    /// because it terminates the token.
    pub fn canonical_token(&self) -> String {
        format!(
            "trigger-hop:v{}:{}:{}:{}:{}:{}",
            TRIGGER_CONTRACT_VERSION,
            self.subscription,
            self.subscription_revision.0,
            self.event_id.len(),
            self.event_id,
            self.request_id,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerEventPayload {
    pub subject: Option<EntityKey>,
    pub fields: BTreeMap<String, Value>,
    pub receipt_hash: Option<String>,
}

/// Immutable evidence supplied to trigger matching.
///
/// `occurred_at` names source time while `observed_at` names the authoritative
/// observation time. Freshness is evaluated against `observed_at`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerEvent {
    pub event_id: String,
    pub kind: TriggerEventKind,
    pub source_revision: Revision,
    pub occurred_at: Tick,
    pub observed_at: Tick,
    pub evidence: Epistemic<TriggerEventPayload>,
    pub causal_ancestry: Vec<CausalHop>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRequest {
    pub contract_version: u16,
    pub request_id: String,
    pub idempotency_key: String,
    pub subscription: EntityKey,
    pub subscription_revision: Revision,
    pub code_definition: EntityKey,
    pub code_revision: Revision,
    pub code_hash: String,
    pub starting_universe_revision: Revision,
    pub issued_at_tick: Tick,
    pub deadline_tick: Tick,
    pub trigger: TriggerEvent,
    pub causal_depth: u16,
    pub budgets: TriggerBudgets,
}

impl ExecutionRequest {
    /// The causal hop this request contributes to any Universe artifact it later
    /// causes (a downstream trigger event, an effect receipt, or a committed
    /// observation). Identity is taken from the pinned request, never invented.
    pub fn execution_hop(&self) -> CausalHop {
        CausalHop {
            subscription: self.subscription,
            subscription_revision: self.subscription_revision,
            event_id: self.trigger.event_id.clone(),
            request_id: self.request_id.clone(),
        }
    }

    /// Structured causal ancestry a downstream event or write set must carry so
    /// that cycle and causal-depth detection continues across the
    /// trigger → receipt → downstream-trigger chain. It is the triggering
    /// event's own ancestry followed by this request's hop, preserving order
    /// and multiplicity without deduplication or reordering.
    pub fn descendant_causal_ancestry(&self) -> Vec<CausalHop> {
        let mut ancestry = self.trigger.causal_ancestry.clone();
        ancestry.push(self.execution_hop());
        ancestry
    }

    /// The descendant causal ancestry projected into opaque write-set tokens so
    /// `UniverseWriteSet::causal_ancestry` can close the chain from this trigger
    /// execution to its committed receipt without a contract type change.
    pub fn descendant_causal_tokens(&self) -> Vec<String> {
        self.descendant_causal_ancestry()
            .iter()
            .map(CausalHop::canonical_token)
            .collect()
    }
}

/// Scheduler-level disposition. Every variant must be wrapped in explicit
/// epistemic evidence by `ExecutionRequestReceipt`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRequestState {
    Accepted,
    Rejected,
    Quarantined,
    Duplicate,
    CoolingDown,
    Debounced,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "issue", rename_all = "snake_case")]
pub enum TriggerIssue {
    UnsupportedContractVersion { observed: u16 },
    DisabledSubscription,
    MissingEventKind,
    DuplicateEventKind { kind: TriggerEventKind },
    InvalidCodeHash,
    EmptyIdempotencyNamespace,
    EmptyEventId,
    ZeroBudget { field: String },
    UnsupportedEvent { kind: TriggerEventKind },
    EventObservedBeforeOccurrence,
    EventFromFuture { observed_at: Tick, issued_at: Tick },
    StaleEvent { age_ticks: u64, maximum_ticks: u32 },
    EventEvidenceUnavailable { state: EpistemicState },
    DuplicateCausalRequest { request_id: String },
    CausalCycle { subscription: EntityKey },
    CausalDepthExceeded { depth: u16, maximum: u16 },
    DeadlineOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerValidationReport {
    pub subscription: EntityKey,
    pub valid: bool,
    pub issues: Vec<TriggerIssue>,
}

/// Content-addressed evidence for event-to-request materialization.
///
/// Rejected, delayed, duplicate, or quarantined decisions intentionally carry
/// no executable request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRequestReceipt {
    pub subscription: EntityKey,
    pub subscription_revision: Revision,
    pub event_id: String,
    pub idempotency_key: Option<String>,
    pub request_hash: Option<String>,
    pub state: Epistemic<ExecutionRequestState>,
    pub issues: Vec<TriggerIssue>,
    pub request: Option<ExecutionRequest>,
}

/// A bounded, reified physical behavior read from graph authority.
///
/// Optional bindings preserve the difference between a missing graph edge and
/// a valid entity reference so validation can emit graph-readable evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorBond {
    pub bond: EntityKey,
    pub source: Option<EntityKey>,
    pub target: Option<EntityKey>,
    pub predicate: Option<EntityKey>,
    pub profile: Option<EntityKey>,
    pub logic_role: Option<EntityKey>,
    pub logic_binding: Epistemic<BehaviorLogicBinding>,
    pub profile_parameters: Epistemic<BehaviorProfileParameters>,
    pub gates: Vec<BehaviorGate>,
    pub objective: Option<EntityKey>,
    pub justifications: Vec<EntityKey>,
    pub budgets: Option<BehaviorBudgets>,
    pub authority: Option<BehaviorAuthority>,
    pub ontology_status: Epistemic<OntologyBindingStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorGate {
    pub gate: EntityKey,
    pub closure: Epistemic<bool>,
    pub stale: bool,
    pub contradictory: bool,
}

/// Native logic is deliberately limited to three generic energy-routing
/// primitives. The graph selects the primitive explicitly; no predicate name
/// or identifier is interpreted by the compiler or physics host.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorLogicKind {
    Support,
    Inhibit,
    Neutral,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorLogicBinding {
    pub kind: BehaviorLogicKind,
    pub definition_hash: String,
}

/// Fully resolved numerical parameters read from the graph-owned physical
/// profile. Values use integer microunits so plan compilation is deterministic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorProfileParameters {
    pub profile_hash: String,
    pub source_threshold: u64,
    pub source_seed_energy: u64,
    pub source_inhibition_threshold: Option<u64>,
    pub target_threshold: u64,
    pub target_seed_energy: u64,
    pub target_inhibition_threshold: Option<u64>,
    pub transfer_energy: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorBudgets {
    pub max_atoms: u32,
    pub max_bonds: u32,
    pub max_steps: u32,
    pub lifetime_ticks: u32,
    pub max_total_energy: u64,
    pub max_wake_cost: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorAuthority {
    pub change_set: EntityKey,
    pub context: EntityKey,
    pub ontology_revision: Revision,
    pub mapping_revision: Revision,
    pub behavior_revision: Revision,
    pub universe_revision: Revision,
    pub change_set_hash: String,
    pub ontology_hash: String,
    pub mapping_hash: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyBindingStatus {
    Active,
    UnresolvedGap,
}

/// Active-registry vocabulary for interpreting the shape of a BehaviorBond.
///
/// Only stable IDs cross this boundary. Names remain graph content and never
/// select compiler or physics behavior.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorVocabulary {
    pub source_atom: EntityKey,
    pub target_atom: EntityKey,
    pub uses_predicate: EntityKey,
    pub uses_profile: EntityKey,
    pub has_logic_role: EntityKey,
    pub gated_by: EntityKey,
    pub serves_objective: EntityKey,
    pub justified_by: EntityKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorGraphRelation {
    pub relation: RelationKey,
    pub source: EntityKey,
    pub predicate: EntityKey,
    pub target: EntityKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorProjectionReadEvidence {
    pub origin: EntityKey,
    pub universe_revision: Revision,
    pub max_entities: u32,
    pub max_relations: u32,
    pub max_content_bytes: u32,
    pub timeout_ticks: u32,
    pub complete_for_behavior: bool,
    pub query_receipt_hash: String,
    pub independent_readback_hash: String,
    pub active_registry_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorBondProperties {
    pub budgets: BehaviorBudgets,
    pub authority: BehaviorAuthority,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorGateContent {
    pub closure: Epistemic<bool>,
    pub stale: bool,
    pub contradictory: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorLogicContent {
    pub kind: BehaviorLogicKind,
}

/// Hash-free graph payload. The materializer injects the independently
/// observed content hash into the compiled `BehaviorProfileParameters`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorPhysicalProfileContent {
    pub source_threshold: u64,
    pub source_seed_energy: u64,
    pub source_inhibition_threshold: Option<u64>,
    pub target_threshold: u64,
    pub target_seed_energy: u64,
    pub target_inhibition_threshold: Option<u64>,
    pub transfer_energy: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum BehaviorResolvedContent {
    Bond(BehaviorBondProperties),
    Predicate(OntologyBindingStatus),
    LogicRole(BehaviorLogicContent),
    PhysicalProfile(BehaviorPhysicalProfileContent),
    Gate(BehaviorGateContent),
    Objective,
    Justification,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedBehaviorNode {
    pub entity: EntityKey,
    pub content_hash: String,
    pub value: Epistemic<BehaviorResolvedContent>,
}

/// Bounded local-query projection used to materialize one BehaviorBond.
///
/// An incomplete or unmeasured read cannot prove that a relation is absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorGraphProjection {
    pub behavior_bond: EntityKey,
    pub vocabulary: BehaviorVocabulary,
    pub relations: Vec<BehaviorGraphRelation>,
    pub contents: Vec<ResolvedBehaviorNode>,
    pub local_read: Epistemic<BehaviorProjectionReadEvidence>,
}

/// Physics-to-health bridge DTO. It contains only measured claims and hashes;
/// no solver type or ontology-specific meaning enters Graph IR.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorPhysicalEvidence {
    pub behavior_bond: EntityKey,
    pub artifact_hash: String,
    pub execution_receipt_hash: String,
    pub converged: bool,
    pub energy_conserved: bool,
    pub contained: bool,
    pub released: bool,
    pub lifetime_within_limit: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorReadbackEvidence {
    pub behavior_bond: EntityKey,
    pub projection_hash: String,
    pub compilation_receipt_hash: String,
    pub artifact_hash: String,
    pub execution_receipt_hash: String,
    pub independent_readback_hash: String,
    pub content_hashes_verified: bool,
    pub causal_chain_verified: bool,
    pub contradictory: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorLoopClosure {
    Closed,
    Open,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representation_round_trips() {
        let code = CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(1),
            required_capabilities: vec!["local_query".into()],
            operators: vec![Operator::Input {
                name: "actor".into(),
                output: 0,
            }],
        };
        let encoded = serde_json::to_vec(&code).unwrap();
        assert_eq!(
            serde_json::from_slice::<CodeDefinition>(&encoded).unwrap(),
            code
        );
    }

    #[test]
    fn trigger_subscription_fixture_round_trips() {
        let fixture = include_str!("../../../fixtures/graph-ir/trigger-subscription.json");
        let subscription: TriggerSubscription = serde_json::from_str(fixture).unwrap();
        assert_eq!(subscription.contract_version, TRIGGER_CONTRACT_VERSION);
        assert_eq!(subscription.event_kinds.len(), 6);
        assert_eq!(
            serde_json::from_slice::<TriggerSubscription>(
                &serde_json::to_vec(&subscription).unwrap()
            )
            .unwrap(),
            subscription
        );
    }

    #[test]
    fn trigger_subscription_without_budgets_is_not_deserializable() {
        let fixture = include_str!("../../../fixtures/graph-ir/trigger-subscription.json");
        let mut value: serde_json::Value = serde_json::from_str(fixture).unwrap();
        value.as_object_mut().unwrap().remove("budgets");
        assert!(serde_json::from_value::<TriggerSubscription>(value).is_err());
    }

    #[test]
    fn behavior_bond_fixture_round_trips() {
        let fixture = include_str!("../../../fixtures/graph-ir/behavior-bond.json");
        let bond: BehaviorBond = serde_json::from_str(fixture).unwrap();
        assert_eq!(bond.gates.len(), 2);
        assert_eq!(
            bond.ontology_status,
            Epistemic::Measured(OntologyBindingStatus::Active)
        );
        assert_eq!(
            serde_json::from_slice::<BehaviorBond>(&serde_json::to_vec(&bond).unwrap()).unwrap(),
            bond
        );
    }

    fn hop(subscription: u128, revision: u64, event: &str, request: &str) -> CausalHop {
        CausalHop {
            subscription: EntityKey(subscription),
            subscription_revision: Revision(revision),
            event_id: event.into(),
            request_id: request.into(),
        }
    }

    fn request_with_ancestry(
        subscription: u128,
        revision: u64,
        event_id: &str,
        request_id: &str,
        ancestry: Vec<CausalHop>,
    ) -> ExecutionRequest {
        ExecutionRequest {
            contract_version: TRIGGER_CONTRACT_VERSION,
            request_id: request_id.into(),
            idempotency_key: "trigger-fixture:key".into(),
            subscription: EntityKey(subscription),
            subscription_revision: Revision(revision),
            code_definition: EntityKey(0x6002),
            code_revision: Revision(11),
            code_hash: "a".repeat(64),
            starting_universe_revision: Revision(7),
            issued_at_tick: Tick(20),
            deadline_tick: Tick(23),
            trigger: TriggerEvent {
                event_id: event_id.into(),
                kind: TriggerEventKind::LocalObservation,
                source_revision: Revision(7),
                occurred_at: Tick(18),
                observed_at: Tick(19),
                evidence: Epistemic::NotMeasured,
                causal_ancestry: ancestry,
            },
            causal_depth: 1,
            budgets: TriggerBudgets {
                fuel: 64,
                max_mutations: 4,
                max_ticks: 3,
            },
        }
    }

    #[test]
    fn descendant_ancestry_appends_this_request_hop_in_order() {
        let parent = hop(0x6001, 7, "event-root", "req-root");
        let request =
            request_with_ancestry(0x6001, 7, "event-child", "req-child", vec![parent.clone()]);
        let descendant = request.descendant_causal_ancestry();
        assert_eq!(descendant.len(), 2);
        assert_eq!(descendant[0], parent);
        assert_eq!(descendant[1], request.execution_hop());
        assert_eq!(descendant[1], hop(0x6001, 7, "event-child", "req-child"));
    }

    #[test]
    fn descendant_ancestry_preserves_multiplicity_without_dedup() {
        let repeated = hop(0x6001, 7, "event-root", "req-root");
        let request = request_with_ancestry(
            0x6002,
            7,
            "event-child",
            "req-child",
            vec![repeated.clone(), repeated.clone()],
        );
        let descendant = request.descendant_causal_ancestry();
        assert_eq!(descendant.len(), 3);
        assert_eq!(descendant[0], repeated);
        assert_eq!(descendant[1], repeated);
        assert_eq!(descendant[2], request.execution_hop());
    }

    #[test]
    fn causal_tokens_are_deterministic_and_unambiguous_across_event_ids() {
        // An event id ending in a colon must not be confusable with the next
        // field; the length prefix keeps the boundary recoverable.
        let ambiguous = hop(0x6001, 7, "abc:", "req");
        let shifted = hop(0x6001, 7, "abc", ":req");
        assert_ne!(ambiguous.canonical_token(), shifted.canonical_token());
        assert_eq!(ambiguous.canonical_token(), ambiguous.canonical_token());
        assert!(ambiguous.canonical_token().starts_with("trigger-hop:v0:"));
    }

    #[test]
    fn descendant_tokens_match_structured_ancestry() {
        let parent = hop(0x6001, 7, "event-root", "req-root");
        let request =
            request_with_ancestry(0x6001, 7, "event-child", "req-child", vec![parent.clone()]);
        let tokens = request.descendant_causal_tokens();
        let expected: Vec<String> = request
            .descendant_causal_ancestry()
            .iter()
            .map(CausalHop::canonical_token)
            .collect();
        assert_eq!(tokens, expected);
        assert_eq!(tokens.len(), 2);
    }
}
