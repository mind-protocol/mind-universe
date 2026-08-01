//! The inference seam: the one trait the native floor knows, and the total,
//! epistemically-complete vocabulary of what an inference can return.
//!
//! # Why this is native and not a toolkit
//!
//! Under CLAUDE.md's test — "generic mechanism, zero variable policy?" — the
//! *act of calling a provider* is mechanism: it moves bytes to a declared
//! endpoint and reads bytes back. Everything variable — which provider, which
//! model, which decoding parameters, which fallback order, which budget, which
//! actor gets which route — is Universe data (see [`crate::routing`]). The
//! native layer only knows how to CALL.
//!
//! # Totality
//!
//! [`InferenceProvider::infer`] returns [`ProviderAttempt`], not
//! `Result<_, _>`. That is deliberate. A `Result` invites the caller to
//! collapse every non-success into one bucket, and CLAUDE.md's epistemic
//! discipline forbids exactly that: `measurement_failed` (we tried and the
//! transport produced failure evidence) is a different state from
//! `not_configured` (we never had credentials, so nothing was measured) which
//! is different again from a real answer, a refusal, and from `unknown` (the
//! deadline passed and nothing landed at all — see
//! [`crate::clock::TurnDisposition::Unknown`]).
//!
//! Every arm below is a MEASURED state carrying its own evidence. Nothing is
//! silently substituted, and there is no "empty completion" fallback.

use serde::{Deserialize, Serialize};
use universe_core::Tick;

// ===========================================================================
// The request
// ===========================================================================

/// One bounded inference request.
///
/// It is self-contained by construction: a provider receives this and nothing
/// else, and never reads the store. That is what makes many inferences safe to
/// have in flight at once — an inference cannot race another inference because
/// it observes nothing mutable.
///
/// `observed_at_revision` is the Universe revision the observation was
/// serialized at. It travels with the request so the admission gate can detect
/// that the world moved underneath an answer while it was in flight.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InferenceRequest {
    /// Stable id for this turn. Also the idempotency root for the capability
    /// receipt of every attempt made on its behalf.
    pub turn_id: String,
    /// The L1 actor whose turn this is. Routing matches on it.
    pub actor_id: String,
    /// The serialized WorldObservation — frozen at dispatch. This is the
    /// prompt. The native floor does not author it and does not interpret it.
    pub observation: String,
    /// Revision the observation was serialized at.
    pub observed_at_revision: u64,
    /// Tick at which the turn was dispatched.
    pub dispatched_at_tick: Tick,
    /// Tick by which an answer must have landed. Past this the turn is
    /// admitted as `Unknown` — never waited on indefinitely, never dropped.
    pub deadline_tick: Tick,
    /// The reachable L2 affordances offered this turn. An answer naming no
    /// offered verb, or more than one, is rejected with a receipt — it is
    /// never coerced into a proposal.
    pub offered_verbs: Vec<String>,
    /// The exact proven targets offered this turn. CLAUDE.md: "an unproven
    /// target is never offered as a verb". An answer naming a target outside
    /// this set is rejected, not repaired.
    pub offered_targets: Vec<String>,
    /// Causal ancestry carried onto every capability intent this turn issues,
    /// so a declared `max_causal_depth` is enforced against real evidence.
    #[serde(default)]
    pub causal_ancestry: Vec<String>,
}

// ===========================================================================
// Readiness
// ===========================================================================

/// Whether a provider can be called at all, decided WITHOUT calling it.
///
/// `NotConfigured` is a first-class answer, never an error and never silently
/// swallowed: a missing credential is a known-absent precondition, not a
/// measurement failure and not a refusal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "readiness", rename_all = "snake_case")]
pub enum ProviderReadiness {
    Ready,
    NotConfigured {
        /// What is absent, named exactly (e.g. the env var name). Never the
        /// value of anything secret.
        missing: String,
        detail: String,
    },
}

impl ProviderReadiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, ProviderReadiness::Ready)
    }
}

// ===========================================================================
// The outcome vocabulary
// ===========================================================================

/// What one provider attempt produced. Total and mutually exclusive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum InferenceOutcome {
    /// The provider returned a completion. This means "bytes came back and
    /// parsed", NOT "the proposal is valid" — the world still disposes at
    /// admission.
    Answered { completion: String },
    /// The provider replied and the reply is an explicit decline (a refusal
    /// stop reason, a policy category). Distinct from failure: the transport
    /// worked perfectly.
    Refused { category: String, detail: String },
    /// Transport was attempted and produced measured failure evidence:
    /// connect refused, timeout, non-2xx, unparseable body. We know it failed.
    MeasurementFailed { reason: String },
    /// A precondition for calling at all is known-absent (no API key, no
    /// endpoint). Nothing was measured, and we do not pretend otherwise.
    NotConfigured { missing: String, detail: String },
    /// Bounded out before transport: deadline already passed, budget
    /// exhausted, declared payload limit exceeded, capability undeclared.
    /// Nothing was attempted; this is not evidence about the provider.
    NotAttempted { reason: String },
}

impl InferenceOutcome {
    /// The stable label used in receipts and routing `advance_on` data. This
    /// is the vocabulary the Universe's fallback policy is written against.
    pub fn label(&self) -> &'static str {
        match self {
            InferenceOutcome::Answered { .. } => "answered",
            InferenceOutcome::Refused { .. } => "refused",
            InferenceOutcome::MeasurementFailed { .. } => "measurement_failed",
            InferenceOutcome::NotConfigured { .. } => "not_configured",
            InferenceOutcome::NotAttempted { .. } => "not_attempted",
        }
    }

    pub fn is_answered(&self) -> bool {
        matches!(self, InferenceOutcome::Answered { .. })
    }
}

// ===========================================================================
// Attribution
// ===========================================================================

/// A value that may or may not have been measured. Used where a provider may
/// or may not echo a field (Ollama echoes `model`; a bare completions endpoint
/// might not). Absence is reported as `not_measured` with a reason — never as
/// an empty string or a guessed default.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Measured<T> {
    Measured { value: T },
    NotMeasured { why: String },
}

impl<T> Measured<T> {
    pub fn measured(value: T) -> Self {
        Measured::Measured { value }
    }
    pub fn not_measured(why: impl Into<String>) -> Self {
        Measured::NotMeasured { why: why.into() }
    }
    pub fn value(&self) -> Option<&T> {
        match self {
            Measured::Measured { value } => Some(value),
            Measured::NotMeasured { .. } => None,
        }
    }
}

/// Evidence for ONE attempt against ONE provider.
///
/// This is what keeps a turn attributable when its inference came from an
/// arbitrary provider: the record names the exact provider, endpoint, model,
/// capability and idempotency key that produced it, and whether a transport
/// was actually attempted. Fallbacks are never hidden — every attempt in a
/// chain gets a record, in order.
///
/// It records header NAMES only, never header values: a credential must never
/// reach a receipt, a snapshot, a log, or an error string.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub provider_id: String,
    pub endpoint: String,
    /// The model the ROUTING asked for.
    pub requested_model: String,
    /// The model the provider says answered. `not_measured` when the wire
    /// shape carries no model echo.
    pub answered_model: Measured<String>,
    pub capability: String,
    /// Names of the headers that were sent. Values are never recorded.
    pub header_names: Vec<String>,
    /// True only if bytes actually left for the endpoint.
    pub transport_attempted: bool,
    /// Idempotency key of the `EffectExecutionReceipt` this attempt produced,
    /// linking the turn to the capability host's own durable evidence.
    pub effect_idempotency_key: String,
    pub http_status: Measured<u16>,
    pub outcome_label: String,
    pub detail: String,
    pub request_bytes: usize,
    pub response_bytes: Measured<usize>,
    pub latency_ms: u64,
    /// sha256 of the exact request bytes transported.
    pub request_digest: String,
    /// sha256 of the exact response bytes received.
    pub response_digest: Measured<String>,
}

/// One provider attempt: its outcome plus its evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderAttempt {
    pub outcome: InferenceOutcome,
    pub record: AttemptRecord,
}

/// Full attribution for a turn, across every provider the route tried.
///
/// A turn stays attributable and bounded no matter which arbitrary provider
/// served it, because this carries: which authored route chose it and at what
/// routing version, where that routing was READ FROM (committed store vs
/// authoring fixture — never conflated), every attempt in order, who finally
/// served it, and the tick bounds the whole thing ran inside.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InferenceAttribution {
    pub turn_id: String,
    pub actor_id: String,
    pub route_id: String,
    pub routing_id: String,
    pub routing_version: String,
    /// Provenance of the routing data itself, e.g.
    /// `"committed store <path> @revision 5"` or
    /// `"authoring fixture <path> (NOT committed)"`. Never implicit.
    pub routing_source: String,
    /// Every attempt, in the order the authored chain made them.
    pub attempts: Vec<AttemptRecord>,
    /// Provider that answered, when one did.
    pub served_by: Option<String>,
    pub dispatched_at_tick: Tick,
    pub observed_at_tick: Tick,
    pub deadline_tick: Tick,
    pub observed_at_revision: u64,
    pub total_latency_ms: u64,
    pub prompt_bytes: usize,
    pub prompt_digest: String,
    /// Budget units the authored route charged for this turn.
    pub budget_charged: u64,
    /// Budget units the authored route allowed.
    pub budget_allowed: u64,
}

impl InferenceAttribution {
    /// The final outcome label of the chain: the last attempt's label, or
    /// `not_attempted` when the route made no attempt at all.
    pub fn final_label(&self) -> &str {
        self.attempts
            .last()
            .map(|attempt| attempt.outcome_label.as_str())
            .unwrap_or("not_attempted")
    }
}

/// What the router hands to the admission gate: the outcome that ended the
/// chain, plus the attribution covering every attempt that got there.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InferenceObservation {
    pub outcome: InferenceOutcome,
    pub attribution: InferenceAttribution,
}

// ===========================================================================
// The trait
// ===========================================================================

/// The inference seam.
///
/// This is the ONLY thing the native floor knows about inference providers.
/// Implementations are constructed from Universe data (see
/// [`crate::routing::ProviderSpec`]); the trait itself carries no notion of
/// which provider is preferred, what a fallback is, or what anything costs.
///
/// # Concurrency seam — deliberately NOT `Send`
///
/// A provider owns live resources: a socket, a subprocess handle, a
/// `CapabilityHost` with its boxed adapter. Requiring `Send` would force every
/// implementation to be thread-movable and would rule out reusing the
/// workspace's existing [`universe_capabilities::CapabilityHost`], whose
/// `Box<dyn EffectAdapter>` carries no `Send` marker.
///
/// So the unit of concurrency is the **lane**, not the provider. What crosses
/// a thread boundary is plain data at both ends:
///
/// ```text
/// [`crate::routing::RoutingTable`]  Send + Sync + Clone  ->  into the worker
/// [`InferenceObservation`]          Send + Sync          <-  back to the gate
/// ```
///
/// A worker thread clones the routing table, builds its own
/// [`CollectiveRouter`](crate::router::CollectiveRouter) and providers
/// *locally*, dispatches, and sends the resulting observation back to the
/// single [`AdmissionGate`](crate::clock::AdmissionGate). Providers never
/// migrate between threads, which is what you want anyway for something
/// holding a connection.
///
/// The `Send`/`Sync` requirement on those two data types is enforced by a
/// compile-time assertion in this module's tests.
pub trait InferenceProvider {
    /// Stable id. Matches `provider_id` in the routing data.
    fn provider_id(&self) -> &str;

    /// Whether this provider can be called, decided without calling it.
    /// Cheap and side-effect free.
    fn readiness(&self) -> ProviderReadiness;

    /// Perform ONE bounded call.
    ///
    /// Total: never panics, never returns `Err`. Every failure mode is a
    /// distinguishable [`InferenceOutcome`] carrying its own evidence. The
    /// implementation must not touch the Universe store, must not retry (the
    /// authored chain owns retry/fallback), and must not exceed the limits it
    /// was constructed with.
    fn infer(&mut self, request: &InferenceRequest) -> ProviderAttempt;
}

/// sha256 hex digest — used for request/response/prompt digests so bytes stay
/// attributable without a payload having to be persisted verbatim.
pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_labels_are_the_stable_routing_vocabulary() {
        // The authored `advance_on` lists in routing data are written against
        // exactly these strings. Changing one silently would change every
        // authored fallback policy in the Universe, so pin them.
        assert_eq!(
            InferenceOutcome::Answered {
                completion: String::new()
            }
            .label(),
            "answered"
        );
        assert_eq!(
            InferenceOutcome::Refused {
                category: String::new(),
                detail: String::new()
            }
            .label(),
            "refused"
        );
        assert_eq!(
            InferenceOutcome::MeasurementFailed {
                reason: String::new()
            }
            .label(),
            "measurement_failed"
        );
        assert_eq!(
            InferenceOutcome::NotConfigured {
                missing: String::new(),
                detail: String::new()
            }
            .label(),
            "not_configured"
        );
        assert_eq!(
            InferenceOutcome::NotAttempted {
                reason: String::new()
            }
            .label(),
            "not_attempted"
        );
    }

    #[test]
    fn not_configured_is_not_a_failure_and_not_an_empty_answer() {
        let absent = InferenceOutcome::NotConfigured {
            missing: "ANTHROPIC_API_KEY".into(),
            detail: "env var is unset".into(),
        };
        let failed = InferenceOutcome::MeasurementFailed {
            reason: "connect refused".into(),
        };
        let empty_answer = InferenceOutcome::Answered {
            completion: String::new(),
        };
        assert_ne!(absent, failed);
        assert_ne!(absent, empty_answer);
        assert_ne!(failed, empty_answer);
        // And they never collapse under serialization either — a receipt read
        // back from disk must still distinguish them.
        let json = |value: &InferenceOutcome| serde_json::to_string(value).unwrap();
        assert_ne!(json(&absent), json(&failed));
        assert_ne!(json(&absent), json(&empty_answer));
    }

    /// The concurrency seam, asserted at compile time.
    ///
    /// Providers are NOT `Send` (they own sockets and subprocess handles), so
    /// the two types that actually cross a thread boundary must be — otherwise
    /// the lane architecture described on [`InferenceProvider`] does not hold.
    #[test]
    fn the_types_that_cross_threads_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::routing::RoutingTable>();
        assert_send_sync::<InferenceObservation>();
        assert_send_sync::<InferenceRequest>();
        assert_send_sync::<crate::clock::TurnDisposition>();
    }

    #[test]
    fn measured_absence_carries_a_reason() {
        let m: Measured<String> = Measured::not_measured("wire shape carries no model echo");
        assert!(m.value().is_none());
        let text = serde_json::to_string(&m).unwrap();
        assert!(text.contains("not_measured"));
        assert!(text.contains("wire shape carries no model echo"));
    }
}
