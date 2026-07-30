//! Versioned, bounded headless protocol contracts.

mod transport;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use universe_core::{EntityKey, Epistemic, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_physics::{
    assess_atom_physical_health, AtomExecutionReceipt, AtomPhysicalHealthCheck,
    AtomPhysicalHealthObservation, AtomTransfer, BondPolarity,
};
use universe_store::{EntityRecord, RelationRecord};
use universe_supervisor::{RuntimeInventory, Supervisor};

pub use transport::{
    AuthenticationSecret, ProtocolClient, ProtocolConnection, ProtocolReadEvent,
    ProtocolTransportConfig, ProtocolTransportError, RunningProtocolServer,
};

pub const PROTOCOL_VERSION: u16 = 0;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrelationId(pub String);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct StreamId(pub String);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct StreamSequence(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StreamBudget {
    pub max_pending_frames: usize,
    pub max_pending_bytes: usize,
    pub max_frame_bytes: usize,
}

impl StreamBudget {
    fn validate(self) -> Result<Self, ProtocolStreamError> {
        if self.max_pending_frames == 0
            || self.max_pending_bytes == 0
            || self.max_frame_bytes == 0
            || self.max_frame_bytes > self.max_pending_bytes
        {
            return Err(ProtocolStreamError::InvalidBudget);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolHello {
    pub minimum_version: u16,
    pub maximum_version: u16,
    pub client_id: String,
    pub resume_after: Option<StreamSequence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolWelcome {
    pub selected_version: u16,
    pub stream_id: StreamId,
    pub earliest_available: StreamSequence,
    pub latest_published: StreamSequence,
    pub resynchronization_required: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryCompletion {
    Complete,
    Partial,
    BudgetExhausted,
    FrontierExhausted,
    Stale,
    Unknown,
    MeasurementFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Accepted,
    Committed,
    Executing,
    Measured,
    Failed,
    Stale,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SituationSnapshotMessage {
    pub universe: UniverseId,
    pub revision: Revision,
    pub tick: Tick,
    pub origin: EntityKey,
    pub completion: QueryCompletion,
    pub entities: Vec<EntityRecord>,
    pub relations: Vec<RelationRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SituationDeltaMessage {
    pub universe: UniverseId,
    pub from_revision: Revision,
    pub to_revision: Revision,
    pub tick: Tick,
    pub entity_upserts: Vec<EntityRecord>,
    pub relation_upserts: Vec<RelationRecord>,
    pub relation_tombstones: Vec<RelationKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UniverseEventMessage {
    pub universe: UniverseId,
    pub revision: Revision,
    pub tick: Tick,
    pub event_id: String,
    pub event_type: String,
    pub state: OperationState,
    pub evidence_hash: Option<String>,
}

pub const VISUAL_MICROUNITS: u32 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnergyTransferDirection {
    SourceToTarget,
    TargetToSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnergyTransferPolarity {
    Support,
    Inhibit,
    Neutral,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnergyTransferOutcome {
    Measured,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnergyTransferEpistemic {
    Observed,
    Measured,
    KnownAbsent,
    Unknown,
    NotMeasured,
    MeasurementFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnergyTransferVisualPrimitive {
    EnergyPacket,
    InhibitoryWave,
    Rupture,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnergyTransferVisualMessage {
    pub primitive: EnergyTransferVisualPrimitive,
    pub color: String,
    pub emissive: String,
    pub emissive_intensity_microunits: u32,
    pub radius_microunits: u32,
    pub opacity_microunits: u32,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnergyTransferMessage {
    pub universe: UniverseId,
    pub revision: Revision,
    pub tick: Tick,
    pub transfer_id: String,
    pub execution_id: String,
    pub intention_id: String,
    pub relation_id: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub direction: EnergyTransferDirection,
    pub polarity: EnergyTransferPolarity,
    pub energy: u64,
    pub gate_microunits: u32,
    pub outcome: EnergyTransferOutcome,
    pub epistemic: EnergyTransferEpistemic,
    pub visual: EnergyTransferVisualMessage,
}

impl EnergyTransferMessage {
    pub fn validate(&self) -> Result<(), ProtocolStreamError> {
        for (field, value) in [
            ("transfer_id", self.transfer_id.as_str()),
            ("execution_id", self.execution_id.as_str()),
            ("intention_id", self.intention_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ProtocolStreamError::InvalidEnergyTransfer(format!(
                    "{field} must be non-empty"
                )));
            }
        }
        if self.source == self.target {
            return Err(ProtocolStreamError::InvalidEnergyTransfer(
                "source and target must differ".into(),
            ));
        }
        if self.gate_microunits > VISUAL_MICROUNITS {
            return Err(ProtocolStreamError::InvalidEnergyTransfer(
                "gate_microunits exceeds one".into(),
            ));
        }
        if self.epistemic != EnergyTransferEpistemic::Measured {
            return Err(ProtocolStreamError::InvalidEnergyTransfer(
                "only measured transfers may be published".into(),
            ));
        }
        if self.visual.radius_microunits == 0
            || self.visual.opacity_microunits > VISUAL_MICROUNITS
            || self.visual.duration_ms == 0
        {
            return Err(ProtocolStreamError::InvalidEnergyTransfer(
                "visual descriptor is outside its bounded domain".into(),
            ));
        }
        for (field, color) in [
            ("color", self.visual.color.as_str()),
            ("emissive", self.visual.emissive.as_str()),
        ] {
            if !is_hex_color(color) {
                return Err(ProtocolStreamError::InvalidEnergyTransfer(format!(
                    "{field} must be a six-digit hex color"
                )));
            }
        }
        Ok(())
    }
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Classifies the epistemic provenance of energy taken from a bounded physical
/// execution, using the independent physical-health observer rather than
/// trusting the caller's own claim.
///
/// Energy is `Measured` only when the observer finds the run **conserved its
/// energy** and **reached quiescence** — a settled deterministic descent.
/// Anything less is honestly downgraded (`NotMeasured` / `MeasurementFailed`) so
/// the streaming membrane refuses to render an unsettled or unverifiable descent
/// as a felt glide. This never upgrades a derived value: a magnitude that was
/// not produced by a real execution has no receipt to pass through here.
pub fn measured_transfer_epistemic(receipt: &AtomExecutionReceipt) -> EnergyTransferEpistemic {
    let health = assess_atom_physical_health(AtomPhysicalHealthObservation::Executed(receipt));
    let conserved = matches!(
        health.energy_conservation,
        Epistemic::Measured(AtomPhysicalHealthCheck::Pass)
    );
    let converged = matches!(
        health.convergence,
        Epistemic::Measured(AtomPhysicalHealthCheck::Pass)
    );
    if conserved && converged {
        EnergyTransferEpistemic::Measured
    } else if matches!(
        health.energy_conservation,
        Epistemic::MeasurementFailed { .. }
    ) {
        EnergyTransferEpistemic::MeasurementFailed
    } else {
        EnergyTransferEpistemic::NotMeasured
    }
}

/// Builds a membrane-eligible [`EnergyTransferMessage`] from a **measured**
/// physical execution.
///
/// This is the legitimate path from a bounded [`AtomExecutionReceipt`] to a
/// streamable energy transfer. It does two things the raw struct literal cannot:
///
/// 1. It proves the transfer **belongs to the executed run** — the same bond,
///    endpoints, polarity, and energy the deterministic solver actually
///    recorded. A fabricated or code-derived magnitude has no matching recorded
///    transfer and is refused here, before the membrane.
/// 2. It derives the epistemic tag from the independent physical-health observer
///    ([`measured_transfer_epistemic`]) instead of asserting `Measured`. A
///    genuinely conserved, quiescent descent yields `Measured` and crosses the
///    membrane; an unsettled or unverifiable one yields a non-`Measured` tag and
///    the membrane's own [`EnergyTransferMessage::validate`] refuses it.
///
/// The energy magnitude carried on the wire is exactly the value the solver
/// moved across the bond — a read-back measurement, not a relabel.
#[allow(clippy::too_many_arguments)]
pub fn energy_transfer_from_execution(
    universe: UniverseId,
    revision: Revision,
    tick: Tick,
    transfer_id: impl Into<String>,
    execution_id: impl Into<String>,
    intention_id: impl Into<String>,
    receipt: &AtomExecutionReceipt,
    transfer: &AtomTransfer,
    gate_microunits: u32,
    visual: EnergyTransferVisualMessage,
) -> Result<EnergyTransferMessage, ProtocolStreamError> {
    let belongs = receipt
        .run
        .steps
        .iter()
        .any(|step| step.transfers.iter().any(|recorded| recorded == transfer));
    if !belongs {
        return Err(ProtocolStreamError::InvalidEnergyTransfer(
            "transfer is not part of the measured execution receipt".into(),
        ));
    }
    let epistemic = measured_transfer_epistemic(receipt);
    let outcome = if epistemic == EnergyTransferEpistemic::Measured {
        EnergyTransferOutcome::Measured
    } else {
        EnergyTransferOutcome::Rejected
    };
    let message = EnergyTransferMessage {
        universe,
        revision,
        tick,
        transfer_id: transfer_id.into(),
        execution_id: execution_id.into(),
        intention_id: intention_id.into(),
        relation_id: transfer.bond,
        source: transfer.source,
        target: transfer.target,
        direction: EnergyTransferDirection::SourceToTarget,
        polarity: match transfer.polarity {
            BondPolarity::Support => EnergyTransferPolarity::Support,
            BondPolarity::Inhibit => EnergyTransferPolarity::Inhibit,
            BondPolarity::Neutral => EnergyTransferPolarity::Neutral,
        },
        energy: transfer.energy,
        gate_microunits,
        outcome,
        epistemic,
        visual,
    };
    message.validate()?;
    Ok(message)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalQueryRequest {
    pub request_id: String,
    pub origin: EntityKey,
    pub max_entities: u32,
    pub max_relations: u32,
    pub max_depth: u16,
    pub timeout_ticks: u32,
    pub allow_approximate_summaries: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalQueryResultMessage {
    pub request_id: String,
    pub revision: Revision,
    pub tick: Tick,
    pub completion: QueryCompletion,
    pub entities: Vec<EntityKey>,
    pub relations: Vec<RelationKey>,
    pub frontier: Vec<EntityKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptMessage {
    pub receipt_id: String,
    pub correlation: CorrelationId,
    pub revision: Revision,
    pub tick: Tick,
    pub state: OperationState,
    pub receipt_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatMessage {
    pub revision: Revision,
    pub tick: Tick,
    pub readiness: OperationState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
pub enum ServerPayload {
    Snapshot(SituationSnapshotMessage),
    Delta(SituationDeltaMessage),
    Event(UniverseEventMessage),
    EnergyTransfer(EnergyTransferMessage),
    QueryResult(LocalQueryResultMessage),
    ReadEntity(ReadEntityResponse),
    ContentListing(ContentListingResponse),
    HydratedContent(HydrateContentResponse),
    Receipt(ReceiptMessage),
    Heartbeat(HeartbeatMessage),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AcknowledgeMessage {
    pub through: StreamSequence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResynchronizeRequest {
    pub origin: EntityKey,
    pub max_entities: u32,
    pub max_relations: u32,
    pub timeout_ticks: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
pub enum ClientPayload {
    Query(LocalQueryRequest),
    ReadEntity(ReadEntityRequest),
    ListContent(ContentListingRequest),
    HydrateContent(HydrateContentRequest),
    Acknowledge(AcknowledgeMessage),
    Resynchronize(ResynchronizeRequest),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientFrame {
    pub protocol_version: u16,
    pub stream_id: StreamId,
    pub correlation: CorrelationId,
    pub payload: ClientPayload,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerFrame {
    pub protocol_version: u16,
    pub stream_id: StreamId,
    pub sequence: StreamSequence,
    pub correlation: CorrelationId,
    pub payload: ServerPayload,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResumeResult {
    Frames {
        frames: Vec<ServerFrame>,
        latest_published: StreamSequence,
    },
    SnapshotRequired {
        requested_after: StreamSequence,
        earliest_available: StreamSequence,
        latest_published: StreamSequence,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolStreamError {
    InvalidBudget,
    EmptyStreamId,
    EmptyClientId,
    UnsupportedVersionRange {
        minimum: u16,
        maximum: u16,
    },
    SequenceOverflow,
    FrameTooLarge {
        bytes: usize,
        limit: usize,
    },
    Backpressured {
        pending_frames: usize,
        pending_bytes: usize,
        frame_bytes: usize,
    },
    InvalidAcknowledgement {
        acknowledged: StreamSequence,
        latest_published: StreamSequence,
    },
    InvalidCursor {
        requested_after: StreamSequence,
        latest_published: StreamSequence,
    },
    InvalidEnergyTransfer(String),
    InvalidVisibilityRequest(String),
    Serialization(String),
}

#[derive(Clone, Debug)]
struct QueuedFrame {
    frame: ServerFrame,
    encoded_bytes: usize,
}

/// One bounded, monotonic stream for one connected client/session.
///
/// Full queues reject publication explicitly. Frames are never silently
/// dropped, and a cursor older than acknowledged retention requires a new
/// bounded snapshot.
pub struct ProtocolStream {
    stream_id: StreamId,
    budget: StreamBudget,
    next_sequence: StreamSequence,
    retired_through: StreamSequence,
    pending_bytes: usize,
    pending: VecDeque<QueuedFrame>,
}

impl ProtocolStream {
    pub fn new(
        stream_id: impl Into<String>,
        budget: StreamBudget,
    ) -> Result<Self, ProtocolStreamError> {
        let stream_id = StreamId(stream_id.into());
        if stream_id.0.trim().is_empty() {
            return Err(ProtocolStreamError::EmptyStreamId);
        }
        Ok(Self {
            stream_id,
            budget: budget.validate()?,
            next_sequence: StreamSequence(1),
            retired_through: StreamSequence(0),
            pending_bytes: 0,
            pending: VecDeque::new(),
        })
    }

    pub fn negotiate(&self, hello: &ProtocolHello) -> Result<ProtocolWelcome, ProtocolStreamError> {
        if hello.client_id.trim().is_empty() {
            return Err(ProtocolStreamError::EmptyClientId);
        }
        if !(hello.minimum_version..=hello.maximum_version).contains(&PROTOCOL_VERSION) {
            return Err(ProtocolStreamError::UnsupportedVersionRange {
                minimum: hello.minimum_version,
                maximum: hello.maximum_version,
            });
        }
        let latest = self.latest_published();
        if let Some(after) = hello.resume_after {
            if after > latest {
                return Err(ProtocolStreamError::InvalidCursor {
                    requested_after: after,
                    latest_published: latest,
                });
            }
        }
        Ok(ProtocolWelcome {
            selected_version: PROTOCOL_VERSION,
            stream_id: self.stream_id.clone(),
            earliest_available: self.earliest_available(),
            latest_published: latest,
            resynchronization_required: hello
                .resume_after
                .is_some_and(|after| after < self.retired_through),
        })
    }

    pub fn publish(
        &mut self,
        correlation: CorrelationId,
        payload: ServerPayload,
    ) -> Result<ServerFrame, ProtocolStreamError> {
        let queued = self.prepare_frame(correlation, payload)?;
        if self.pending.len() >= self.budget.max_pending_frames
            || self
                .pending_bytes
                .checked_add(queued.encoded_bytes)
                .is_none_or(|bytes| bytes > self.budget.max_pending_bytes)
        {
            return Err(ProtocolStreamError::Backpressured {
                pending_frames: self.pending.len(),
                pending_bytes: self.pending_bytes,
                frame_bytes: queued.encoded_bytes,
            });
        }
        Ok(self.enqueue(queued))
    }

    pub fn acknowledge(&mut self, sequence: StreamSequence) -> Result<usize, ProtocolStreamError> {
        let latest = self.latest_published();
        if sequence > latest {
            return Err(ProtocolStreamError::InvalidAcknowledgement {
                acknowledged: sequence,
                latest_published: latest,
            });
        }
        if sequence <= self.retired_through {
            return Ok(0);
        }
        let mut removed = 0;
        while self
            .pending
            .front()
            .is_some_and(|queued| queued.frame.sequence <= sequence)
        {
            let queued = self.pending.pop_front().expect("front frame exists");
            self.pending_bytes -= queued.encoded_bytes;
            removed += 1;
        }
        self.retired_through = sequence;
        Ok(removed)
    }

    pub fn resume(&self, after: StreamSequence) -> Result<ResumeResult, ProtocolStreamError> {
        let latest = self.latest_published();
        if after > latest {
            return Err(ProtocolStreamError::InvalidCursor {
                requested_after: after,
                latest_published: latest,
            });
        }
        if after < self.retired_through {
            return Ok(ResumeResult::SnapshotRequired {
                requested_after: after,
                earliest_available: self.earliest_available(),
                latest_published: latest,
            });
        }
        Ok(ResumeResult::Frames {
            frames: self
                .pending
                .iter()
                .filter(|queued| queued.frame.sequence > after)
                .map(|queued| queued.frame.clone())
                .collect(),
            latest_published: latest,
        })
    }

    /// Retires the session's pending delta history and starts a new recovery
    /// boundary with a bounded authoritative situation snapshot.
    pub fn resynchronize(
        &mut self,
        correlation: CorrelationId,
        snapshot: SituationSnapshotMessage,
    ) -> Result<ServerFrame, ProtocolStreamError> {
        let queued = self.prepare_frame(correlation, ServerPayload::Snapshot(snapshot))?;
        if queued.encoded_bytes > self.budget.max_pending_bytes {
            return Err(ProtocolStreamError::FrameTooLarge {
                bytes: queued.encoded_bytes,
                limit: self.budget.max_pending_bytes,
            });
        }
        self.pending.clear();
        self.pending_bytes = 0;
        self.retired_through = self.latest_published();
        Ok(self.enqueue(queued))
    }

    pub fn pending_frames(&self) -> usize {
        self.pending.len()
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    pub fn latest_published(&self) -> StreamSequence {
        StreamSequence(self.next_sequence.0.saturating_sub(1))
    }

    pub fn earliest_available(&self) -> StreamSequence {
        self.pending
            .front()
            .map(|queued| queued.frame.sequence)
            .unwrap_or(self.next_sequence)
    }

    fn prepare_frame(
        &self,
        correlation: CorrelationId,
        payload: ServerPayload,
    ) -> Result<QueuedFrame, ProtocolStreamError> {
        if let ServerPayload::EnergyTransfer(transfer) = &payload {
            transfer.validate()?;
        }
        if self.next_sequence.0 == u64::MAX {
            return Err(ProtocolStreamError::SequenceOverflow);
        }
        let frame = ServerFrame {
            protocol_version: PROTOCOL_VERSION,
            stream_id: self.stream_id.clone(),
            sequence: self.next_sequence,
            correlation,
            payload,
        };
        let encoded_bytes = serde_json::to_vec(&frame)
            .map_err(|error| ProtocolStreamError::Serialization(error.to_string()))?
            .len();
        if encoded_bytes > self.budget.max_frame_bytes {
            return Err(ProtocolStreamError::FrameTooLarge {
                bytes: encoded_bytes,
                limit: self.budget.max_frame_bytes,
            });
        }
        Ok(QueuedFrame {
            frame,
            encoded_bytes,
        })
    }

    fn enqueue(&mut self, queued: QueuedFrame) -> ServerFrame {
        let frame = queued.frame.clone();
        self.pending_bytes += queued.encoded_bytes;
        self.pending.push_back(queued);
        self.next_sequence = StreamSequence(
            self.next_sequence
                .0
                .checked_add(1)
                .expect("sequence overflow is checked before publication"),
        );
        frame
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadEntityRequest {
    pub protocol_version: u16,
    pub correlation: CorrelationId,
    pub key: EntityKey,
    pub max_entities: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadEntityResponse {
    pub correlation: CorrelationId,
    pub revision: Revision,
    pub tick: Tick,
    pub result: Epistemic<EntityRecord>,
}

/// A capability label an actor must hold to observe a content ref.
///
/// Labels are materialized from the graph's visibility relations and supplied
/// to the protocol as data. They are never a constant policy compiled into the
/// dispatcher, so a change in what an actor may see is a change in graph data,
/// not in this crate.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct VisibilityLabel(pub String);

/// One content ref together with the label the graph requires to observe it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VisibleContent {
    pub required_label: VisibilityLabel,
    pub record: EntityRecord,
}

/// The bounded, graph-materialized visibility projection for one local
/// situation.
///
/// This is data provided by the caller from the graph's visibility relations.
/// The protocol consults it; it does not invent visibility. Listing is bounded
/// by an explicit per-request budget, so there is no whole-situation export
/// path, and hydration is refused for any ref whose required label the actor
/// does not present.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VisibilityCatalog {
    pub revision: Revision,
    pub tick: Tick,
    pub content: Vec<VisibleContent>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentListingRequest {
    pub request_id: String,
    pub actor: EntityKey,
    pub presented_labels: Vec<VisibilityLabel>,
    pub max_refs: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentListingResponse {
    pub request_id: String,
    pub revision: Revision,
    pub tick: Tick,
    pub completion: QueryCompletion,
    /// Only the content refs the presented labels permit, bounded by the
    /// request budget.
    pub visible: Vec<EntityKey>,
    /// Count of catalog refs the actor is not permitted to observe. Reported so
    /// a filtered listing is never mistaken for proven absence of content.
    pub withheld: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HydrateContentRequest {
    pub request_id: String,
    pub actor: EntityKey,
    pub presented_labels: Vec<VisibilityLabel>,
    pub key: EntityKey,
}

/// The honest outcome of a hydration attempt.
///
/// `VisibilityDenied` (the ref exists but its label was not presented) is kept
/// distinct from `Absent` (the ref is not in the catalog). A denied ref is
/// never collapsed into known-absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum HydrationOutcome {
    Delivered(EntityRecord),
    VisibilityDenied { required_label: VisibilityLabel },
    Absent,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HydrateContentResponse {
    pub request_id: String,
    pub revision: Revision,
    pub tick: Tick,
    pub outcome: HydrationOutcome,
}

impl VisibilityCatalog {
    /// Returns only the content refs the presented labels permit, bounded by
    /// `max_refs`. When more permitted refs exist than the budget admits, the
    /// completion is `BudgetExhausted`; there is no unbounded export path.
    pub fn list(
        &self,
        request: &ContentListingRequest,
    ) -> Result<ContentListingResponse, ProtocolStreamError> {
        if request.max_refs == 0 {
            return Err(ProtocolStreamError::InvalidVisibilityRequest(
                "max_refs must be non-zero".into(),
            ));
        }
        let held: BTreeSet<&VisibilityLabel> = request.presented_labels.iter().collect();
        let mut visible = Vec::new();
        let mut permitted_total: usize = 0;
        let mut withheld: usize = 0;
        for item in &self.content {
            if held.contains(&item.required_label) {
                permitted_total += 1;
                if (visible.len() as u64) < u64::from(request.max_refs) {
                    visible.push(item.record.key);
                }
            } else {
                withheld += 1;
            }
        }
        let completion = if permitted_total > visible.len() {
            QueryCompletion::BudgetExhausted
        } else {
            QueryCompletion::Complete
        };
        Ok(ContentListingResponse {
            request_id: request.request_id.clone(),
            revision: self.revision,
            tick: self.tick,
            completion,
            visible,
            withheld,
        })
    }

    /// Delivers a content record only when the presented labels satisfy the
    /// ref's required label. A ref present but not permitted is refused with
    /// `VisibilityDenied`; a ref absent from the catalog is `Absent`. A ref the
    /// actor never listed is still gated: crafting the request directly cannot
    /// bypass the label check.
    pub fn hydrate(&self, request: &HydrateContentRequest) -> HydrateContentResponse {
        let held: BTreeSet<&VisibilityLabel> = request.presented_labels.iter().collect();
        let outcome = match self
            .content
            .iter()
            .find(|item| item.record.key == request.key)
        {
            None => HydrationOutcome::Absent,
            Some(item) if held.contains(&item.required_label) => {
                HydrationOutcome::Delivered(item.record.clone())
            }
            Some(item) => HydrationOutcome::VisibilityDenied {
                required_label: item.required_label.clone(),
            },
        };
        HydrateContentResponse {
            request_id: request.request_id.clone(),
            revision: self.revision,
            tick: self.tick,
            outcome,
        }
    }
}

pub struct HeadlessProtocol<'a> {
    supervisor: &'a Supervisor,
}

impl<'a> HeadlessProtocol<'a> {
    pub fn new(supervisor: &'a Supervisor) -> Self {
        Self { supervisor }
    }

    /// Opens a fresh store replay through the supervisor; it never serves the
    /// commit receipt or the supervisor's in-memory projection as readback.
    pub fn read_entity(
        &self,
        request: ReadEntityRequest,
    ) -> Result<ReadEntityResponse, UniverseError> {
        if request.protocol_version != PROTOCOL_VERSION {
            return Err(UniverseError::UnsupportedVersion(request.protocol_version));
        }
        if request.max_entities == 0 {
            return Err(UniverseError::BudgetExhausted(
                "max_entities must be non-zero".into(),
            ));
        }
        let snapshot = self.supervisor.independent_readback()?;
        let result = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == request.key)
            .cloned()
            .map(Epistemic::Observed)
            .unwrap_or(Epistemic::KnownAbsent);
        Ok(ReadEntityResponse {
            correlation: request.correlation,
            revision: snapshot.revision,
            tick: snapshot.tick,
            result,
        })
    }

    /// Lists the content refs a presented capability set may observe within a
    /// graph-materialized visibility projection. The projection is supplied by
    /// the caller from graph visibility relations; the protocol never invents
    /// it. The listing is bounded by the request budget, so it is not a
    /// whole-situation export.
    pub fn list_content(
        &self,
        catalog: &VisibilityCatalog,
        request: ContentListingRequest,
    ) -> Result<ContentListingResponse, ProtocolStreamError> {
        catalog.list(&request)
    }

    /// Hydrates a single content ref only when the presented capability set
    /// satisfies the ref's graph-required label. Refusal (`VisibilityDenied`)
    /// is honest and distinct from `Absent`.
    pub fn hydrate_content(
        &self,
        catalog: &VisibilityCatalog,
        request: HydrateContentRequest,
    ) -> HydrateContentResponse {
        catalog.hydrate(&request)
    }

    pub fn runtime_inventory(&self) -> RuntimeInventory {
        self.supervisor.runtime_inventory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(max_pending_frames: usize) -> StreamBudget {
        StreamBudget {
            max_pending_frames,
            max_pending_bytes: 16 * 1024,
            max_frame_bytes: 8 * 1024,
        }
    }

    fn heartbeat(tick: u64, state: OperationState) -> ServerPayload {
        ServerPayload::Heartbeat(HeartbeatMessage {
            revision: Revision(tick),
            tick: Tick(tick),
            readiness: state,
        })
    }

    fn snapshot(revision: u64) -> SituationSnapshotMessage {
        SituationSnapshotMessage {
            universe: UniverseId(1),
            revision: Revision(revision),
            tick: Tick(revision),
            origin: EntityKey(1),
            completion: QueryCompletion::Complete,
            entities: Vec::new(),
            relations: Vec::new(),
        }
    }

    fn energy_transfer() -> EnergyTransferMessage {
        EnergyTransferMessage {
            universe: UniverseId(1),
            revision: Revision(7),
            tick: Tick(12),
            transfer_id: "transfer-1".into(),
            execution_id: "execution-1".into(),
            intention_id: "intention-1".into(),
            relation_id: RelationKey(3),
            source: EntityKey(1),
            target: EntityKey(2),
            direction: EnergyTransferDirection::SourceToTarget,
            polarity: EnergyTransferPolarity::Support,
            energy: 100,
            gate_microunits: VISUAL_MICROUNITS,
            outcome: EnergyTransferOutcome::Measured,
            epistemic: EnergyTransferEpistemic::Measured,
            visual: EnergyTransferVisualMessage {
                primitive: EnergyTransferVisualPrimitive::EnergyPacket,
                color: "#fff3c4".into(),
                emissive: "#ffd76a".into(),
                emissive_intensity_microunits: 4 * VISUAL_MICROUNITS,
                radius_microunits: 90_000,
                opacity_microunits: VISUAL_MICROUNITS,
                duration_ms: 240,
            },
        }
    }

    #[test]
    fn sequence_ack_resume_and_snapshot_recovery_are_explicit() {
        let mut stream = ProtocolStream::new("actor-1", budget(4)).unwrap();
        let first = stream
            .publish(
                CorrelationId("correlation-1".into()),
                heartbeat(1, OperationState::Committed),
            )
            .unwrap();
        let second = stream
            .publish(
                CorrelationId("correlation-2".into()),
                heartbeat(2, OperationState::Measured),
            )
            .unwrap();
        assert_eq!(first.sequence, StreamSequence(1));
        assert_eq!(second.sequence, StreamSequence(2));

        assert_eq!(stream.acknowledge(StreamSequence(1)).unwrap(), 1);
        assert!(matches!(
            stream.resume(StreamSequence(0)).unwrap(),
            ResumeResult::SnapshotRequired {
                requested_after: StreamSequence(0),
                earliest_available: StreamSequence(2),
                latest_published: StreamSequence(2),
            }
        ));
        assert!(matches!(
            stream.resume(StreamSequence(1)).unwrap(),
            ResumeResult::Frames { frames, .. }
                if frames.iter().map(|frame| frame.sequence).collect::<Vec<_>>()
                    == vec![StreamSequence(2)]
        ));

        let welcome = stream
            .negotiate(&ProtocolHello {
                minimum_version: 0,
                maximum_version: 0,
                client_id: "desktop".into(),
                resume_after: Some(StreamSequence(0)),
            })
            .unwrap();
        assert!(welcome.resynchronization_required);
        let recovery = stream
            .resynchronize(CorrelationId("recovery".into()), snapshot(2))
            .unwrap();
        assert_eq!(recovery.sequence, StreamSequence(3));
        assert_eq!(stream.pending_frames(), 1);
        assert!(matches!(
            stream.resume(StreamSequence(2)).unwrap(),
            ResumeResult::Frames { frames, .. }
                if frames == vec![recovery]
        ));
    }

    #[test]
    fn bounded_queue_backpressures_without_consuming_a_sequence() {
        let mut stream = ProtocolStream::new("actor-1", budget(1)).unwrap();
        stream
            .publish(
                CorrelationId("first".into()),
                heartbeat(1, OperationState::Accepted),
            )
            .unwrap();
        let error = stream
            .publish(
                CorrelationId("second".into()),
                heartbeat(2, OperationState::Executing),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ProtocolStreamError::Backpressured {
                pending_frames: 1,
                ..
            }
        ));
        assert_eq!(stream.latest_published(), StreamSequence(1));
        stream.acknowledge(StreamSequence(1)).unwrap();
        let second = stream
            .publish(
                CorrelationId("second".into()),
                heartbeat(2, OperationState::Executing),
            )
            .unwrap();
        assert_eq!(second.sequence, StreamSequence(2));
    }

    #[test]
    fn frame_size_and_version_ranges_fail_closed() {
        let mut stream = ProtocolStream::new(
            "observer-1",
            StreamBudget {
                max_pending_frames: 2,
                max_pending_bytes: 512,
                max_frame_bytes: 256,
            },
        )
        .unwrap();
        let error = stream
            .publish(
                CorrelationId("oversize".into()),
                ServerPayload::Event(UniverseEventMessage {
                    universe: UniverseId(1),
                    revision: Revision(1),
                    tick: Tick(1),
                    event_id: "event-1".into(),
                    event_type: "x".repeat(1_024),
                    state: OperationState::Unknown,
                    evidence_hash: None,
                }),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ProtocolStreamError::FrameTooLarge { limit: 256, .. }
        ));
        assert_eq!(stream.latest_published(), StreamSequence(0));

        assert!(matches!(
            stream.negotiate(&ProtocolHello {
                minimum_version: 1,
                maximum_version: 2,
                client_id: "future-client".into(),
                resume_after: None,
            }),
            Err(ProtocolStreamError::UnsupportedVersionRange {
                minimum: 1,
                maximum: 2,
            })
        ));
    }

    #[test]
    fn wire_states_remain_distinct_after_serialization() {
        let states = [
            OperationState::Accepted,
            OperationState::Committed,
            OperationState::Executing,
            OperationState::Measured,
            OperationState::Failed,
            OperationState::Stale,
            OperationState::Unknown,
        ];
        for state in states {
            let payload = heartbeat(1, state);
            let encoded = serde_json::to_vec(&payload).unwrap();
            let decoded: ServerPayload = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded, payload);
        }
    }

    #[test]
    fn measured_energy_transfer_round_trips_on_the_bounded_stream() {
        let transfer = energy_transfer();
        transfer.validate().unwrap();
        let mut stream = ProtocolStream::new("desktop-energy", budget(2)).unwrap();
        let frame = stream
            .publish(
                CorrelationId("energy-correlation".into()),
                ServerPayload::EnergyTransfer(transfer.clone()),
            )
            .unwrap();
        let encoded = serde_json::to_vec(&frame).unwrap();
        let decoded: ServerFrame = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded, frame);
        assert!(matches!(
            decoded.payload,
            ServerPayload::EnergyTransfer(observed) if observed == transfer
        ));
    }

    #[test]
    fn energy_transfer_fails_closed_without_measured_provenance_or_bounds() {
        let mut transfer = energy_transfer();
        transfer.epistemic = EnergyTransferEpistemic::NotMeasured;
        assert!(matches!(
            transfer.validate(),
            Err(ProtocolStreamError::InvalidEnergyTransfer(_))
        ));

        transfer.epistemic = EnergyTransferEpistemic::Measured;
        transfer.gate_microunits = VISUAL_MICROUNITS + 1;
        let mut stream = ProtocolStream::new("desktop-energy", budget(1)).unwrap();
        assert!(matches!(
            stream.publish(
                CorrelationId("invalid-energy".into()),
                ServerPayload::EnergyTransfer(transfer)
            ),
            Err(ProtocolStreamError::InvalidEnergyTransfer(_))
        ));
        assert_eq!(stream.latest_published(), StreamSequence(0));
    }

    fn visual() -> EnergyTransferVisualMessage {
        EnergyTransferVisualMessage {
            primitive: EnergyTransferVisualPrimitive::EnergyPacket,
            color: "#fff3c4".into(),
            emissive: "#ffd76a".into(),
            emissive_intensity_microunits: 4 * VISUAL_MICROUNITS,
            radius_microunits: 90_000,
            opacity_microunits: VISUAL_MICROUNITS,
            duration_ms: 240,
        }
    }

    /// A support atom seeds 100, fires, and conducts 100 to a downstream atom;
    /// the bounded solver reaches quiescence and conserves energy.
    fn conserved_quiescent_receipt() -> universe_physics::AtomExecutionReceipt {
        universe_physics::execute_local_atom_cluster(
            universe_physics::LocalAtomCluster {
                atoms: vec![
                    universe_physics::AtomSpec {
                        key: EntityKey(1),
                        threshold: 100,
                        seed_energy: 100,
                        required_supports: Vec::new(),
                        inhibition_threshold: None,
                    },
                    universe_physics::AtomSpec {
                        key: EntityKey(2),
                        threshold: 100,
                        seed_energy: 0,
                        required_supports: vec![RelationKey(1)],
                        inhibition_threshold: None,
                    },
                ],
                bonds: vec![universe_physics::AtomBond {
                    key: RelationKey(1),
                    source: EntityKey(1),
                    target: EntityKey(2),
                    polarity: BondPolarity::Support,
                    energy: 100,
                }],
                injections: Vec::new(),
            },
            universe_physics::AtomExecutionBudget {
                max_atoms: 2,
                max_bonds: 1,
                max_steps: 3,
                max_total_energy: 100,
            },
        )
        .unwrap()
    }

    /// A three-atom chain forced to stop after a single step: a real, conserved,
    /// but UNSETTLED (step-budget-exhausted) execution.
    fn conserved_unsettled_receipt() -> universe_physics::AtomExecutionReceipt {
        universe_physics::execute_local_atom_cluster(
            universe_physics::LocalAtomCluster {
                atoms: vec![
                    universe_physics::AtomSpec {
                        key: EntityKey(1),
                        threshold: 100,
                        seed_energy: 100,
                        required_supports: Vec::new(),
                        inhibition_threshold: None,
                    },
                    universe_physics::AtomSpec {
                        key: EntityKey(2),
                        threshold: 100,
                        seed_energy: 0,
                        required_supports: vec![RelationKey(1)],
                        inhibition_threshold: None,
                    },
                    universe_physics::AtomSpec {
                        key: EntityKey(3),
                        threshold: 100,
                        seed_energy: 0,
                        required_supports: vec![RelationKey(2)],
                        inhibition_threshold: None,
                    },
                ],
                bonds: vec![
                    universe_physics::AtomBond {
                        key: RelationKey(1),
                        source: EntityKey(1),
                        target: EntityKey(2),
                        polarity: BondPolarity::Support,
                        energy: 100,
                    },
                    universe_physics::AtomBond {
                        key: RelationKey(2),
                        source: EntityKey(2),
                        target: EntityKey(3),
                        polarity: BondPolarity::Support,
                        energy: 100,
                    },
                ],
                injections: Vec::new(),
            },
            universe_physics::AtomExecutionBudget {
                max_atoms: 3,
                max_bonds: 2,
                max_steps: 1,
                max_total_energy: 100,
            },
        )
        .unwrap()
    }

    fn first_transfer(receipt: &universe_physics::AtomExecutionReceipt) -> AtomTransfer {
        receipt
            .run
            .steps
            .iter()
            .flat_map(|step| step.transfers.iter())
            .next()
            .cloned()
            .expect("execution recorded at least one transfer")
    }

    #[test]
    fn conserved_quiescent_execution_yields_a_measured_transfer_that_crosses_the_membrane() {
        let receipt = conserved_quiescent_receipt();
        assert_eq!(
            measured_transfer_epistemic(&receipt),
            EnergyTransferEpistemic::Measured
        );
        let transfer = first_transfer(&receipt);
        assert_eq!(
            transfer.energy, 100,
            "measured magnitude is the conducted energy"
        );

        let message = energy_transfer_from_execution(
            UniverseId(1),
            Revision(7),
            Tick(1),
            "transfer-1",
            "execution-1",
            "intention-1",
            &receipt,
            &transfer,
            VISUAL_MICROUNITS,
            visual(),
        )
        .expect("a genuinely measured transfer builds a valid message");
        assert_eq!(message.epistemic, EnergyTransferEpistemic::Measured);
        assert_eq!(message.outcome, EnergyTransferOutcome::Measured);
        assert_eq!(message.energy, transfer.energy);

        // The membrane accepts it on a real bounded stream.
        let mut stream = ProtocolStream::new("desktop-energy", budget(2)).unwrap();
        let frame = stream
            .publish(
                CorrelationId("energy".into()),
                ServerPayload::EnergyTransfer(message),
            )
            .expect("membrane admits the genuinely measured transfer");
        assert_eq!(frame.sequence, StreamSequence(1));
    }

    #[test]
    fn a_derived_magnitude_not_produced_by_the_execution_is_refused() {
        let receipt = conserved_quiescent_receipt();
        // Same shape as the recorded transfer but a code-derived magnitude the
        // solver never moved. It has no matching recorded transfer.
        let mut fabricated = first_transfer(&receipt);
        fabricated.energy += 1;

        let error = energy_transfer_from_execution(
            UniverseId(1),
            Revision(7),
            Tick(1),
            "transfer-1",
            "execution-1",
            "intention-1",
            &receipt,
            &fabricated,
            VISUAL_MICROUNITS,
            visual(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProtocolStreamError::InvalidEnergyTransfer(reason)
                if reason.contains("not part of the measured execution")
        ));
    }

    #[test]
    fn an_unsettled_but_real_execution_is_downgraded_and_refused_by_the_membrane() {
        let receipt = conserved_unsettled_receipt();
        assert_eq!(
            receipt.convergence,
            universe_physics::AtomConvergence::StepBudgetExhausted
        );
        assert!(
            receipt.energy.conserved,
            "the partial run still conserved energy"
        );
        // Conserved energy alone is not a settled descent: the observer withholds
        // Measured because the run did not reach quiescence.
        assert_eq!(
            measured_transfer_epistemic(&receipt),
            EnergyTransferEpistemic::NotMeasured
        );

        let transfer = first_transfer(&receipt);
        let error = energy_transfer_from_execution(
            UniverseId(1),
            Revision(7),
            Tick(1),
            "transfer-1",
            "execution-1",
            "intention-1",
            &receipt,
            &transfer,
            VISUAL_MICROUNITS,
            visual(),
        )
        .unwrap_err();
        // The membrane rule itself (EnergyTransferMessage::validate) refuses the
        // non-measured transfer.
        assert!(matches!(
            error,
            ProtocolStreamError::InvalidEnergyTransfer(reason)
                if reason.contains("only measured transfers may be published")
        ));
    }

    fn record(key: u128) -> EntityRecord {
        EntityRecord {
            key: EntityKey(key),
            generation: 0,
            symbol: 0,
            content: None,
        }
    }

    fn labelled(key: u128, label: &str) -> VisibleContent {
        VisibleContent {
            required_label: VisibilityLabel(label.into()),
            record: record(key),
        }
    }

    fn catalog() -> VisibilityCatalog {
        VisibilityCatalog {
            revision: Revision(9),
            tick: Tick(14),
            content: vec![
                labelled(1, "public"),
                labelled(2, "public"),
                labelled(3, "secret"),
            ],
        }
    }

    fn listing_request(labels: &[&str], max_refs: u32) -> ContentListingRequest {
        ContentListingRequest {
            request_id: "listing-1".into(),
            actor: EntityKey(100),
            presented_labels: labels
                .iter()
                .map(|l| VisibilityLabel((*l).into()))
                .collect(),
            max_refs,
        }
    }

    fn hydrate_request(key: u128, labels: &[&str]) -> HydrateContentRequest {
        HydrateContentRequest {
            request_id: "hydrate-1".into(),
            actor: EntityKey(100),
            presented_labels: labels
                .iter()
                .map(|l| VisibilityLabel((*l).into()))
                .collect(),
            key: EntityKey(key),
        }
    }

    #[test]
    fn visibility_gates_listing_and_distinguishes_denied_from_absent() {
        let catalog = catalog();

        // A public-only actor sees exactly the public refs; the secret ref is
        // withheld, not reported as absent.
        let listing = catalog.list(&listing_request(&["public"], 8)).unwrap();
        assert_eq!(listing.visible, vec![EntityKey(1), EntityKey(2)]);
        assert_eq!(listing.withheld, 1);
        assert_eq!(listing.completion, QueryCompletion::Complete);

        // A permitted ref hydrates.
        let delivered = catalog.hydrate(&hydrate_request(1, &["public"]));
        assert!(matches!(
            delivered.outcome,
            HydrationOutcome::Delivered(ref got) if *got == record(1)
        ));

        // Crafting a hydration request for a ref the actor never listed is still
        // refused: the label gate cannot be bypassed. Denial is NOT absence.
        let denied = catalog.hydrate(&hydrate_request(3, &["public"]));
        assert!(matches!(
            denied.outcome,
            HydrationOutcome::VisibilityDenied { ref required_label }
                if *required_label == VisibilityLabel("secret".into())
        ));
        assert_ne!(denied.outcome, HydrationOutcome::Absent);

        // A ref absent from the catalog is honestly Absent, distinct from denial.
        let absent = catalog.hydrate(&hydrate_request(999, &["public", "secret"]));
        assert_eq!(absent.outcome, HydrationOutcome::Absent);

        // Presenting the secret label unlocks the previously denied ref.
        let unlocked = catalog.hydrate(&hydrate_request(3, &["public", "secret"]));
        assert!(matches!(unlocked.outcome, HydrationOutcome::Delivered(_)));
    }

    #[test]
    fn visibility_listing_is_bounded_and_refuses_a_zero_budget() {
        let catalog = catalog();

        // A budget smaller than the permitted set yields a bounded page and an
        // honest BudgetExhausted completion: no whole-situation export.
        let bounded = catalog.list(&listing_request(&["public"], 1)).unwrap();
        assert_eq!(bounded.visible, vec![EntityKey(1)]);
        assert_eq!(bounded.completion, QueryCompletion::BudgetExhausted);

        // A zero budget is refused rather than silently treated as unbounded.
        assert!(matches!(
            catalog.list(&listing_request(&["public"], 0)),
            Err(ProtocolStreamError::InvalidVisibilityRequest(_))
        ));
    }

    #[test]
    fn visibility_messages_round_trip_on_the_wire() {
        let catalog = catalog();
        let stream_id = StreamId("visibility".into());

        // The listing response round-trips as a server frame.
        let listing = catalog.list(&listing_request(&["public"], 8)).unwrap();
        let listing_frame = ServerFrame {
            protocol_version: PROTOCOL_VERSION,
            stream_id: stream_id.clone(),
            sequence: StreamSequence(1),
            correlation: CorrelationId("listing".into()),
            payload: ServerPayload::ContentListing(listing),
        };
        let encoded = serde_json::to_vec(&listing_frame).unwrap();
        assert_eq!(
            serde_json::from_slice::<ServerFrame>(&encoded).unwrap(),
            listing_frame
        );

        // All three hydration outcomes round-trip and stay distinct.
        for hydrated in [
            catalog.hydrate(&hydrate_request(1, &["public"])),
            catalog.hydrate(&hydrate_request(3, &["public"])),
            catalog.hydrate(&hydrate_request(999, &["public"])),
        ] {
            let payload = ServerPayload::HydratedContent(hydrated);
            let bytes = serde_json::to_vec(&payload).unwrap();
            assert_eq!(
                serde_json::from_slice::<ServerPayload>(&bytes).unwrap(),
                payload
            );
        }

        // The requests round-trip as client frames.
        for payload in [
            ClientPayload::ListContent(listing_request(&["public"], 4)),
            ClientPayload::HydrateContent(hydrate_request(2, &["public"])),
        ] {
            let frame = ClientFrame {
                protocol_version: PROTOCOL_VERSION,
                stream_id: stream_id.clone(),
                correlation: CorrelationId("request".into()),
                payload,
            };
            let bytes = serde_json::to_vec(&frame).unwrap();
            assert_eq!(
                serde_json::from_slice::<ClientFrame>(&bytes).unwrap(),
                frame
            );
        }
    }
}
