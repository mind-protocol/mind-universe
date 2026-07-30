//! Generic effect authorization and measured transport receipts.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use universe_core::{Tick, UniverseError};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EffectIntent {
    pub capability: String,
    pub idempotency_key: String,
    pub payload: Vec<u8>,
    pub deadline_tick: Tick,
    pub causal_ancestry: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EffectReceipt {
    TransportSucceeded { response: Vec<u8> },
    TransportFailed { reason: String },
}

/// Evidence for one capability execution. `transport_attempted` distinguishes a
/// real adapter result from a failure observed before any external action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EffectExecutionReceipt {
    pub capability: String,
    pub idempotency_key: String,
    pub observed_at_tick: Tick,
    pub causal_ancestry: Vec<String>,
    pub transport_attempted: bool,
    pub outcome: EffectReceipt,
}

pub trait EffectAdapter {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String>;
}

/// Marker substituted for the transport response or failure reason of a
/// capability declared `sensitive`, so a secret never reaches a persisted
/// receipt, snapshot, log, trace, or error.
pub const REDACTED_MARKER: &str = "[redacted:sensitive-capability]";

/// One graph-owned capability declaration. This is pure graph data
/// (materialized into the registry from the store or a fixture): the host holds
/// no policy of its own, it only enforces what a declaration states.
///
/// `principal`, `target`, and `cooldown` limits are intentionally absent until
/// `EffectIntent` carries a principal/target/timing so they can be enforced
/// against real evidence instead of invented defaults.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityDeclaration {
    pub capability: String,
    /// Declaration version as recorded in the graph. Bumped when the promise or
    /// its limits change; carried into evidence so a receipt is attributable to
    /// an exact declared contract.
    pub version: String,
    /// Maximum accepted payload size in bytes. `None` means the declaration does
    /// not bound payload size (unknown, not "unlimited by policy").
    #[serde(default)]
    pub max_payload_bytes: Option<u32>,
    /// Maximum accepted causal ancestry depth. `None` means unbounded by this
    /// declaration.
    #[serde(default)]
    pub max_causal_depth: Option<u32>,
    /// When true, the transport response and any failure reason are redacted
    /// from the persisted/returned receipt.
    #[serde(default)]
    pub sensitive: bool,
}

/// A versioned, graph-owned map of capability declarations. The `version`
/// identifies the registry revision materialized from the graph.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityRegistry {
    pub version: String,
    #[serde(default)]
    pub declarations: BTreeMap<String, CapabilityDeclaration>,
}

impl CapabilityRegistry {
    pub fn declaration(&self, capability: &str) -> Option<&CapabilityDeclaration> {
        self.declarations.get(capability)
    }
}

#[derive(Default)]
pub struct CapabilityHost {
    adapters: BTreeMap<String, Box<dyn EffectAdapter>>,
    completed: BTreeMap<String, (EffectIntent, EffectExecutionReceipt)>,
    /// Optional graph-owned registry. When present, every intent must resolve to
    /// a declaration and satisfy its limits before transport. When absent, the
    /// host preserves its prior declaration-by-adapter-registration behavior.
    registry: Option<CapabilityRegistry>,
}

impl CapabilityHost {
    pub fn register(&mut self, name: impl Into<String>, adapter: Box<dyn EffectAdapter>) {
        self.adapters.insert(name.into(), adapter);
    }

    /// Attaches the graph-owned registry, enabling declaration and limit
    /// enforcement for every subsequent intent.
    pub fn with_registry(mut self, registry: CapabilityRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn set_registry(&mut self, registry: CapabilityRegistry) {
        self.registry = Some(registry);
    }

    /// The active registry version, or `None` when no registry is attached.
    pub fn registry_version(&self) -> Option<&str> {
        self.registry
            .as_ref()
            .map(|registry| registry.version.as_str())
    }

    pub fn execute(
        &mut self,
        now: Tick,
        intent: &EffectIntent,
    ) -> Result<EffectReceipt, UniverseError> {
        self.execute_measured(now, intent)
            .map(|receipt| receipt.outcome)
    }

    pub fn execute_measured(
        &mut self,
        now: Tick,
        intent: &EffectIntent,
    ) -> Result<EffectExecutionReceipt, UniverseError> {
        if let Some((completed_intent, receipt)) = self.completed.get(&intent.idempotency_key) {
            if completed_intent != intent {
                return Err(UniverseError::Validation(format!(
                    "effect idempotency key collision: {}",
                    intent.idempotency_key
                )));
            }
            return Ok(receipt.clone());
        }
        // Graph-owned enforcement: an intent must resolve to a declaration and
        // satisfy its limits before any transport. Undeclared capabilities are
        // denied outright; limit violations persist a pre-transport failure
        // receipt so the denial is durable, idempotent, and observable.
        let declaration = match &self.registry {
            Some(registry) => {
                let declaration = registry.declaration(&intent.capability).ok_or_else(|| {
                    UniverseError::CapabilityDenied(format!(
                        "{} is not declared in capability registry {}",
                        intent.capability, registry.version
                    ))
                })?;
                Some(declaration.clone())
            }
            None => None,
        };
        if let Some(declaration) = &declaration {
            if let Some(reason) = limit_denial(declaration, intent) {
                let receipt = EffectExecutionReceipt {
                    capability: intent.capability.clone(),
                    idempotency_key: intent.idempotency_key.clone(),
                    observed_at_tick: now,
                    causal_ancestry: intent.causal_ancestry.clone(),
                    transport_attempted: false,
                    outcome: EffectReceipt::TransportFailed { reason },
                };
                self.completed.insert(
                    intent.idempotency_key.clone(),
                    (intent.clone(), receipt.clone()),
                );
                return Ok(receipt);
            }
        }
        if now > intent.deadline_tick {
            let receipt = EffectExecutionReceipt {
                capability: intent.capability.clone(),
                idempotency_key: intent.idempotency_key.clone(),
                observed_at_tick: now,
                causal_ancestry: intent.causal_ancestry.clone(),
                transport_attempted: false,
                outcome: EffectReceipt::TransportFailed {
                    reason: "deadline exceeded before transport".into(),
                },
            };
            self.completed.insert(
                intent.idempotency_key.clone(),
                (intent.clone(), receipt.clone()),
            );
            return Ok(receipt);
        }
        let adapter = self
            .adapters
            .get_mut(&intent.capability)
            .ok_or_else(|| UniverseError::CapabilityDenied(intent.capability.clone()))?;
        let outcome = match adapter.transport(&intent.payload) {
            Ok(response) => EffectReceipt::TransportSucceeded { response },
            Err(reason) => EffectReceipt::TransportFailed { reason },
        };
        let outcome = match &declaration {
            Some(declaration) if declaration.sensitive => redact_outcome(outcome),
            _ => outcome,
        };
        let receipt = EffectExecutionReceipt {
            capability: intent.capability.clone(),
            idempotency_key: intent.idempotency_key.clone(),
            observed_at_tick: now,
            causal_ancestry: intent.causal_ancestry.clone(),
            transport_attempted: true,
            outcome,
        };
        self.completed.insert(
            intent.idempotency_key.clone(),
            (intent.clone(), receipt.clone()),
        );
        Ok(receipt)
    }
}

/// Returns an explicit denial reason when the intent violates a declared limit,
/// or `None` when every declared limit is satisfied. Reasons carry only sizes
/// and counts, never payload bytes, so they are safe to persist even for a
/// sensitive capability.
fn limit_denial(declaration: &CapabilityDeclaration, intent: &EffectIntent) -> Option<String> {
    if let Some(max) = declaration.max_payload_bytes {
        if intent.payload.len() as u64 > u64::from(max) {
            return Some(format!(
                "payload {} bytes exceeds declared limit {} bytes",
                intent.payload.len(),
                max
            ));
        }
    }
    if let Some(max) = declaration.max_causal_depth {
        if intent.causal_ancestry.len() as u64 > u64::from(max) {
            return Some(format!(
                "causal depth {} exceeds declared limit {}",
                intent.causal_ancestry.len(),
                max
            ));
        }
    }
    None
}

/// Replaces a sensitive capability's transport response or failure reason with
/// the redaction marker, keeping the secret out of the persisted receipt.
fn redact_outcome(outcome: EffectReceipt) -> EffectReceipt {
    match outcome {
        EffectReceipt::TransportSucceeded { .. } => EffectReceipt::TransportSucceeded {
            response: REDACTED_MARKER.as_bytes().to_vec(),
        },
        EffectReceipt::TransportFailed { .. } => EffectReceipt::TransportFailed {
            reason: REDACTED_MARKER.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl EffectAdapter for Echo {
        fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
            Ok(payload.to_vec())
        }
    }

    #[test]
    fn receipt_contains_actual_adapter_result() {
        let mut host = CapabilityHost::default();
        host.register("safe.echo", Box::new(Echo));
        let intent = EffectIntent {
            capability: "safe.echo".into(),
            idempotency_key: "effect-1".into(),
            payload: b"observed".to_vec(),
            deadline_tick: Tick(2),
            causal_ancestry: vec!["decision-1".into()],
        };
        let receipt = host.execute_measured(Tick(1), &intent).unwrap();
        assert_eq!(
            receipt,
            EffectExecutionReceipt {
                capability: "safe.echo".into(),
                idempotency_key: "effect-1".into(),
                observed_at_tick: Tick(1),
                causal_ancestry: vec!["decision-1".into()],
                transport_attempted: true,
                outcome: EffectReceipt::TransportSucceeded {
                    response: b"observed".to_vec()
                }
            }
        );
        assert_eq!(host.execute_measured(Tick(1), &intent).unwrap(), receipt);
    }

    #[test]
    fn pre_transport_failure_and_idempotency_collision_are_explicit() {
        let mut host = CapabilityHost::default();
        host.register("safe.echo", Box::new(Echo));
        let intent = EffectIntent {
            capability: "safe.echo".into(),
            idempotency_key: "expired-effect".into(),
            payload: b"never transported".to_vec(),
            deadline_tick: Tick(1),
            causal_ancestry: vec!["decision-expired".into()],
        };
        let receipt = host.execute_measured(Tick(2), &intent).unwrap();
        assert!(!receipt.transport_attempted);
        assert_eq!(
            receipt.outcome,
            EffectReceipt::TransportFailed {
                reason: "deadline exceeded before transport".into()
            }
        );
        assert_eq!(host.execute_measured(Tick(3), &intent).unwrap(), receipt);

        let mut collision = intent;
        collision.payload = b"different".to_vec();
        assert!(matches!(
            host.execute_measured(Tick(3), &collision),
            Err(UniverseError::Validation(message))
                if message.contains("idempotency key collision")
        ));
    }

    fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    struct Secret;
    impl EffectAdapter for Secret {
        fn transport(&mut self, _payload: &[u8]) -> Result<Vec<u8>, String> {
            Ok(b"SECRET-TOKEN-abc123".to_vec())
        }
    }

    fn registry() -> CapabilityRegistry {
        let mut declarations = BTreeMap::new();
        declarations.insert(
            "safe.echo".to_string(),
            CapabilityDeclaration {
                capability: "safe.echo".into(),
                version: "1".into(),
                max_payload_bytes: Some(8),
                max_causal_depth: Some(1),
                sensitive: false,
            },
        );
        declarations.insert(
            "secret.mint".to_string(),
            CapabilityDeclaration {
                capability: "secret.mint".into(),
                version: "1".into(),
                max_payload_bytes: None,
                max_causal_depth: None,
                sensitive: true,
            },
        );
        CapabilityRegistry {
            version: "registry-v1".into(),
            declarations,
        }
    }

    #[test]
    fn registry_denies_undeclared_capability_and_exposes_version() {
        let mut host = CapabilityHost::default().with_registry(registry());
        host.register("undeclared.adapter", Box::new(Echo));
        assert_eq!(host.registry_version(), Some("registry-v1"));
        let intent = EffectIntent {
            capability: "undeclared.adapter".into(),
            idempotency_key: "effect-undeclared".into(),
            payload: b"x".to_vec(),
            deadline_tick: Tick(5),
            causal_ancestry: vec![],
        };
        assert!(matches!(
            host.execute_measured(Tick(1), &intent),
            Err(UniverseError::CapabilityDenied(message))
                if message.contains("not declared in capability registry registry-v1")
        ));
    }

    #[test]
    fn registry_enforces_limits_before_transport_and_persists_denial() {
        let mut host = CapabilityHost::default().with_registry(registry());
        host.register("safe.echo", Box::new(Echo));
        let oversized = EffectIntent {
            capability: "safe.echo".into(),
            idempotency_key: "effect-oversized".into(),
            payload: b"way too many bytes".to_vec(),
            deadline_tick: Tick(5),
            causal_ancestry: vec!["decision-1".into()],
        };
        let receipt = host.execute_measured(Tick(1), &oversized).unwrap();
        assert!(!receipt.transport_attempted);
        assert_eq!(
            receipt.outcome,
            EffectReceipt::TransportFailed {
                reason: "payload 18 bytes exceeds declared limit 8 bytes".into()
            }
        );
        // Denial is durable and idempotent.
        assert_eq!(host.execute_measured(Tick(4), &oversized).unwrap(), receipt);

        let too_deep = EffectIntent {
            capability: "safe.echo".into(),
            idempotency_key: "effect-deep".into(),
            payload: b"ok".to_vec(),
            deadline_tick: Tick(5),
            causal_ancestry: vec!["a".into(), "b".into()],
        };
        let receipt = host.execute_measured(Tick(1), &too_deep).unwrap();
        assert!(!receipt.transport_attempted);
        assert_eq!(
            receipt.outcome,
            EffectReceipt::TransportFailed {
                reason: "causal depth 2 exceeds declared limit 1".into()
            }
        );

        // A within-limits intent still transports.
        let ok = EffectIntent {
            capability: "safe.echo".into(),
            idempotency_key: "effect-ok".into(),
            payload: b"ok".to_vec(),
            deadline_tick: Tick(5),
            causal_ancestry: vec!["a".into()],
        };
        let receipt = host.execute_measured(Tick(1), &ok).unwrap();
        assert!(receipt.transport_attempted);
    }

    #[test]
    fn sensitive_capability_secret_never_reaches_receipt_snapshot_or_trace() {
        let mut host = CapabilityHost::default().with_registry(registry());
        host.register("secret.mint", Box::new(Secret));
        let intent = EffectIntent {
            capability: "secret.mint".into(),
            idempotency_key: "effect-secret".into(),
            payload: b"issue".to_vec(),
            deadline_tick: Tick(5),
            causal_ancestry: vec![],
        };
        let receipt = host.execute_measured(Tick(1), &intent).unwrap();
        assert!(receipt.transport_attempted);
        assert_eq!(
            receipt.outcome,
            EffectReceipt::TransportSucceeded {
                response: REDACTED_MARKER.as_bytes().to_vec()
            }
        );
        // The receipt is what flows into snapshots. Prove the secret bytes are
        // absent from the response bytes actually stored.
        let response_bytes = match &receipt.outcome {
            EffectReceipt::TransportSucceeded { response } => response.clone(),
            EffectReceipt::TransportFailed { .. } => panic!("expected success"),
        };
        assert!(!contains_subsequence(
            &response_bytes,
            b"SECRET-TOKEN-abc123"
        ));
        assert_eq!(response_bytes, REDACTED_MARKER.as_bytes());
        // Read the persisted receipt back: the secret is not stored either.
        let persisted = host.execute_measured(Tick(2), &intent).unwrap();
        assert_eq!(persisted, receipt);
    }
}
