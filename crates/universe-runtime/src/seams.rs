//! The one seam the resident loop leaves open for someone else to fill.
//!
//! # The inference seam
//!
//! An L1 actor's turn is one local inference, serial, one call in flight. The
//! runtime owns exactly that shape: it hands a prompt to an [`InferencePort`] and
//! takes back text. It does NOT own which model, which provider, which decoding
//! options, how the prompt was assembled, or how the reply becomes an intent —
//! all of that belongs to the provider layer and to the Universe, not to the
//! floor.
//!
//! There is deliberately no second seam here. Durability is not a seam: the city
//! lives in memory and takes a backup now and then (see
//! [`crate::resident::BackupPolicy`]), which is a call to an existing store
//! method, not an architecture.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use universe_core::EntityKey;

/// One turn's request for cognition.
///
/// `prompt` is already-serialised text: assembling the bounded WorldObservation
/// (two frames + action verbs + context) is Universe work, not runtime work. The
/// runtime carries `actor` only so the reply is attributable.
#[derive(Clone, Debug)]
pub struct InferenceCall {
    /// The actor whose turn this is. Attribution, not routing.
    pub actor: EntityKey,
    /// The serialised observation the actor is thinking from.
    pub prompt: String,
    /// How long the runtime is willing to hold the loop for this call. The loop
    /// is serial, so this is the city's heartbeat pause. The runtime CARRIES this
    /// value; enforcing it is the provider's job.
    pub deadline: Duration,
}

/// What came back. Text only: parsing it into a verb + target + justification is
/// Universe data, not the floor's business.
#[derive(Clone, Debug)]
pub struct InferenceReply {
    pub text: String,
}

/// Why a turn produced no thought. Never collapsed into an empty reply — an
/// absent inference must stay distinguishable from a silent one.
#[derive(Clone, Debug)]
pub enum InferenceError {
    /// No provider is attached.
    Unavailable(String),
    /// The provider did not answer within [`InferenceCall::deadline`].
    Timeout,
    /// The transport itself failed.
    Transport(String),
    /// The provider answered, refusing to produce a completion.
    Refused(String),
}

impl std::fmt::Display for InferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "inference unavailable: {reason}"),
            Self::Timeout => write!(f, "inference timed out"),
            Self::Transport(reason) => write!(f, "inference transport failed: {reason}"),
            Self::Refused(reason) => write!(f, "inference refused: {reason}"),
        }
    }
}

impl std::error::Error for InferenceError {}

/// Cognition's attachment point — the whole seam.
///
/// # Contract
///
/// * `&mut self` plus a serial loop means **one call in flight**, structurally.
///   The runtime additionally guards re-entrancy and counts violations rather
///   than assuming there are none.
/// * The port MUST NOT touch the store. It receives text and returns text.
/// * The port MUST NOT fabricate a reply when it cannot reach a provider: that
///   is [`InferenceError::Unavailable`], and the turn produces no thought.
pub trait InferencePort: Send {
    fn infer(&mut self, call: &InferenceCall) -> Result<InferenceReply, InferenceError>;
}

/// The default port: there is no provider, and it says so. It never invents a
/// completion, so a resident Universe running without cognition is visibly
/// running without cognition.
#[derive(Debug, Default)]
pub struct UnavailableInference {
    pub refusals: u64,
}

impl InferencePort for UnavailableInference {
    fn infer(&mut self, _call: &InferenceCall) -> Result<InferenceReply, InferenceError> {
        self.refusals += 1;
        Err(InferenceError::Unavailable(
            "no provider is attached to the inference seam".into(),
        ))
    }
}

/// What one call to the inference seam measured. Kept on the runtime so a turn's
/// cognition is as inspectable as its physics.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferenceTrace {
    pub actor: EntityKey,
    pub prompt_bytes: usize,
    /// `Some(text)` on success; `None` with `error` set otherwise. A failed turn
    /// is never recorded as an empty completion.
    pub reply: Option<String>,
    pub error: Option<String>,
    pub latency_nanos: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default inference port refuses instead of inventing a completion.
    #[test]
    fn the_default_inference_port_refuses_rather_than_fabricate() {
        let mut port = UnavailableInference::default();
        let result = port.infer(&InferenceCall {
            actor: EntityKey(1),
            prompt: "observe".into(),
            deadline: Duration::from_secs(1),
        });
        assert!(matches!(result, Err(InferenceError::Unavailable(_))));
        assert_eq!(port.refusals, 1);
    }
}
