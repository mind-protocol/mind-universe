//! A provider built entirely from authored data.
//!
//! [`HttpJsonProvider`] is the only [`InferenceProvider`] implementation in
//! this crate, and it is generic: it renders the authored request template,
//! transports the bytes through the graph-owned capability host, and reads the
//! completion back with the authored JSON pointer. Ollama and the Anthropic
//! Messages API are two *data* instances of it, not two code paths.
//!
//! Adding a third provider that speaks a different JSON dialect is a routing
//! edit. Adding one that speaks a non-JSON protocol would need a new
//! `WireTransport` — a genuine trusted-computing-base change, and the correct
//! place to draw that line.
//!
//! # Credential discipline
//!
//! A credential is read from the environment at call time. It is never in
//! graph content, never in argv, never in an attribution, and never in an
//! error string: every recorded string passes through [`scrub`], which
//! replaces any occurrence of the credential value with a marker.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use universe_capabilities::{
    CapabilityDeclaration, CapabilityHost, CapabilityRegistry, EffectAdapter, EffectIntent,
    EffectReceipt,
};
use universe_core::UniverseError;

use crate::contract::{
    digest, AttemptRecord, InferenceOutcome, InferenceProvider, InferenceRequest, Measured,
    ProviderAttempt, ProviderReadiness,
};
use crate::routing::{render_template, AuthSpec, ProviderSpec};
use crate::transport::{split_http, TransportReadiness, WireRequest, WireTransport};

/// Marker substituted wherever a credential value would otherwise be recorded.
pub const CREDENTIAL_MARKER: &str = "[redacted:credential]";

/// Longest provider-error snippet recorded into evidence.
const MAX_DETAIL_BYTES: usize = 256;

/// Replaces every occurrence of `secret` in `text` with [`CREDENTIAL_MARKER`].
/// A no-op when there is no secret. Applied to EVERY string that reaches an
/// attribution, a receipt, or an error.
pub fn scrub(text: &str, secret: Option<&str>) -> String {
    match secret {
        Some(secret) if !secret.is_empty() => text.replace(secret, CREDENTIAL_MARKER),
        _ => text.to_string(),
    }
}

/// The bridge between the capability host's byte-level `EffectAdapter` and a
/// [`WireTransport`]. It holds the endpoint and header plan so the payload it
/// receives is only ever the rendered request body.
struct TransportAdapter {
    endpoint: String,
    method: String,
    content_type: String,
    timeout: Duration,
    headers: Vec<(String, String)>,
    transport: Box<dyn WireTransport>,
    secret: Option<String>,
}

impl EffectAdapter for TransportAdapter {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut headers = vec![("content-type".to_string(), self.content_type.clone())];
        headers.extend(self.headers.iter().cloned());
        let request = WireRequest {
            endpoint: self.endpoint.clone(),
            method: self.method.clone(),
            headers,
            body: payload.to_vec(),
            timeout: self.timeout,
        };
        self.transport
            .send(&request)
            .map_err(|reason| scrub(&reason, self.secret.as_deref()))
    }
}

pub struct HttpJsonProvider {
    spec: ProviderSpec,
    host: CapabilityHost,
    /// Readiness decided at construction: whether the declared credential was
    /// present in the environment, and whether the transport can run.
    readiness: ProviderReadiness,
    header_names: Vec<String>,
    transport_id: String,
    secret: Option<String>,
    /// True when this provider is backed by a stub rather than a real network
    /// transport. Recorded in evidence so a stubbed attempt is never reported
    /// as a real measurement of the remote provider.
    stubbed: bool,
}

impl HttpJsonProvider {
    /// Build a provider from its authored spec and a byte transport.
    ///
    /// Reads the declared credential env var exactly once, here. An absent or
    /// empty variable makes the provider permanently `not_configured` for this
    /// process — it will never send an empty header and never guess.
    pub fn new(spec: ProviderSpec, transport: Box<dyn WireTransport>) -> Self {
        Self::build(spec, transport, false)
    }

    /// Same, but marked as stub-backed so the evidence says so.
    pub fn stubbed(spec: ProviderSpec, transport: Box<dyn WireTransport>) -> Self {
        Self::build(spec, transport, true)
    }

    fn build(spec: ProviderSpec, transport: Box<dyn WireTransport>, stubbed: bool) -> Self {
        let transport_id = transport.transport_id().to_string();
        let transport_readiness = transport.readiness();

        let (headers, secret, credential_readiness) = resolve_headers(&spec);
        let readiness = match (credential_readiness, transport_readiness) {
            (ProviderReadiness::NotConfigured { missing, detail }, _) => {
                ProviderReadiness::NotConfigured { missing, detail }
            }
            (ProviderReadiness::Ready, TransportReadiness::Unavailable { reason }) => {
                ProviderReadiness::NotConfigured {
                    missing: format!("transport:{transport_id}"),
                    detail: reason,
                }
            }
            (ProviderReadiness::Ready, TransportReadiness::Ready) => ProviderReadiness::Ready,
        };

        let mut header_names = vec!["content-type".to_string()];
        header_names.extend(headers.iter().map(|(name, _)| name.clone()));

        // The capability registry is derived from the AUTHORED provider spec,
        // not from native policy: the host enforces exactly the limits the
        // Universe declared, and denies any capability it did not declare.
        let mut declarations = BTreeMap::new();
        declarations.insert(
            spec.capability.clone(),
            CapabilityDeclaration {
                capability: spec.capability.clone(),
                version: format!("provider:{}", spec.provider_id),
                max_payload_bytes: spec.limits.max_request_bytes,
                max_causal_depth: spec.limits.max_causal_depth,
                // Deliberately false. Redaction here would replace the whole
                // transport response, including the completion we must read.
                // The credential is protected by never entering the payload
                // (it is a header) and by `scrub` on every recorded string.
                sensitive: false,
            },
        );
        let registry = CapabilityRegistry {
            version: format!("routing-derived:{}", spec.provider_id),
            declarations,
        };

        let mut host = CapabilityHost::default().with_registry(registry);
        host.register(
            spec.capability.clone(),
            Box::new(TransportAdapter {
                endpoint: spec.transport.endpoint.clone(),
                method: spec.transport.method.clone(),
                content_type: spec.transport.content_type.clone(),
                timeout: Duration::from_millis(spec.transport.timeout_ms),
                headers,
                transport,
                secret: secret.clone(),
            }),
        );

        Self {
            spec,
            host,
            readiness,
            header_names,
            transport_id,
            secret,
            stubbed,
        }
    }

    pub fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    pub fn transport_id(&self) -> &str {
        &self.transport_id
    }

    pub fn is_stubbed(&self) -> bool {
        self.stubbed
    }

    fn blank_record(&self, outcome_label: &str, detail: String) -> AttemptRecord {
        AttemptRecord {
            provider_id: self.spec.provider_id.clone(),
            endpoint: self.spec.transport.endpoint.clone(),
            requested_model: self.spec.model.clone(),
            answered_model: Measured::not_measured("no transport was attempted"),
            capability: self.spec.capability.clone(),
            header_names: self.header_names.clone(),
            transport_attempted: false,
            effect_idempotency_key: String::new(),
            http_status: Measured::not_measured("no transport was attempted"),
            outcome_label: outcome_label.to_string(),
            detail: scrub(&detail, self.secret.as_deref()),
            request_bytes: 0,
            response_bytes: Measured::not_measured("no transport was attempted"),
            latency_ms: 0,
            request_digest: String::new(),
            response_digest: Measured::not_measured("no transport was attempted"),
        }
    }
}

fn resolve_headers(spec: &ProviderSpec) -> (Vec<(String, String)>, Option<String>, ProviderReadiness) {
    let mut headers: Vec<(String, String)> = spec
        .extra_headers
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();

    match &spec.auth {
        AuthSpec::None => (headers, None, ProviderReadiness::Ready),
        AuthSpec::EnvHeader { env_var, header } => match read_env(env_var) {
            Some(value) => {
                headers.push((header.clone(), value.clone()));
                (headers, Some(value), ProviderReadiness::Ready)
            }
            None => (
                headers,
                None,
                ProviderReadiness::NotConfigured {
                    missing: env_var.clone(),
                    detail: format!(
                        "environment variable {env_var} is unset or empty; provider \
                         {} is not configured (this is known-absent, not a failed call)",
                        spec.provider_id
                    ),
                },
            ),
        },
        AuthSpec::EnvBearer { env_var, prefix } => match read_env(env_var) {
            Some(value) => {
                headers.push(("authorization".to_string(), format!("{prefix} {value}")));
                (headers, Some(value), ProviderReadiness::Ready)
            }
            None => (
                headers,
                None,
                ProviderReadiness::NotConfigured {
                    missing: env_var.clone(),
                    detail: format!(
                        "environment variable {env_var} is unset or empty; provider \
                         {} is not configured (this is known-absent, not a failed call)",
                        spec.provider_id
                    ),
                },
            ),
        },
    }
}

fn read_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

impl InferenceProvider for HttpJsonProvider {
    fn provider_id(&self) -> &str {
        &self.spec.provider_id
    }

    fn readiness(&self) -> ProviderReadiness {
        self.readiness.clone()
    }

    fn infer(&mut self, request: &InferenceRequest) -> ProviderAttempt {
        // (1) Known-absent preconditions: never conflated with failure.
        if let ProviderReadiness::NotConfigured { missing, detail } = &self.readiness {
            return ProviderAttempt {
                outcome: InferenceOutcome::NotConfigured {
                    missing: missing.clone(),
                    detail: detail.clone(),
                },
                record: self.blank_record("not_configured", detail.clone()),
            };
        }

        // (2) Declared prompt bound, enforced before anything is rendered.
        if let Some(max) = self.spec.limits.max_prompt_bytes {
            if request.observation.len() as u64 > u64::from(max) {
                let reason = format!(
                    "prompt {} bytes exceeds provider {} declared max_prompt_bytes {}",
                    request.observation.len(),
                    self.spec.provider_id,
                    max
                );
                return ProviderAttempt {
                    outcome: InferenceOutcome::NotAttempted {
                        reason: reason.clone(),
                    },
                    record: self.blank_record("not_attempted", reason),
                };
            }
        }

        // (3) Render the AUTHORED template. No native decoding parameter, no
        //     native system prompt, no native model choice.
        let body = render_template(
            &self.spec.request_template,
            &request.observation,
            &self.spec.model,
        );
        let payload = match serde_json::to_vec(&body) {
            Ok(payload) => payload,
            Err(error) => {
                let reason = format!("authored request template did not serialize: {error}");
                return ProviderAttempt {
                    outcome: InferenceOutcome::NotAttempted {
                        reason: reason.clone(),
                    },
                    record: self.blank_record("not_attempted", reason),
                };
            }
        };
        let request_digest = digest(&payload);
        let request_bytes = payload.len();

        let idempotency_key = format!("{}:{}", request.turn_id, self.spec.provider_id);
        let intent = EffectIntent {
            capability: self.spec.capability.clone(),
            idempotency_key: idempotency_key.clone(),
            payload,
            deadline_tick: request.deadline_tick,
            causal_ancestry: request.causal_ancestry.clone(),
        };

        // (4) Transport through the graph-owned capability host: declaration,
        //     limits, deadline and idempotency are enforced there.
        let started = Instant::now();
        let execution = self.host.execute_measured(request.dispatched_at_tick, &intent);
        let latency_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

        let receipt = match execution {
            Ok(receipt) => receipt,
            Err(UniverseError::CapabilityDenied(message)) => {
                let reason = format!("capability denied before transport: {message}");
                let mut record = self.blank_record("not_attempted", reason.clone());
                record.effect_idempotency_key = idempotency_key;
                record.request_digest = request_digest;
                record.request_bytes = request_bytes;
                return ProviderAttempt {
                    outcome: InferenceOutcome::NotAttempted { reason },
                    record,
                };
            }
            Err(error) => {
                let reason = format!("capability host refused the intent: {error}");
                let mut record = self.blank_record("not_attempted", reason.clone());
                record.effect_idempotency_key = idempotency_key;
                record.request_digest = request_digest;
                record.request_bytes = request_bytes;
                return ProviderAttempt {
                    outcome: InferenceOutcome::NotAttempted { reason },
                    record,
                };
            }
        };

        let mut record = AttemptRecord {
            provider_id: self.spec.provider_id.clone(),
            endpoint: self.spec.transport.endpoint.clone(),
            requested_model: self.spec.model.clone(),
            answered_model: Measured::not_measured(
                "no usable response body, so no model echo was read",
            ),
            capability: self.spec.capability.clone(),
            header_names: self.header_names.clone(),
            transport_attempted: receipt.transport_attempted,
            effect_idempotency_key: idempotency_key,
            http_status: Measured::not_measured("no status line was received"),
            outcome_label: String::new(),
            detail: String::new(),
            request_bytes,
            response_bytes: Measured::not_measured("no response bytes were received"),
            latency_ms,
            request_digest,
            response_digest: Measured::not_measured("no response bytes were received"),
        };

        let raw = match &receipt.outcome {
            EffectReceipt::TransportFailed { reason } => {
                let scrubbed = scrub(reason, self.secret.as_deref());
                // The host reports pre-transport denials (declared limit,
                // deadline) with `transport_attempted == false`. Those are
                // `not_attempted`, not evidence about the provider.
                let (outcome, label) = if receipt.transport_attempted {
                    (
                        InferenceOutcome::MeasurementFailed {
                            reason: scrubbed.clone(),
                        },
                        "measurement_failed",
                    )
                } else {
                    (
                        InferenceOutcome::NotAttempted {
                            reason: scrubbed.clone(),
                        },
                        "not_attempted",
                    )
                };
                record.outcome_label = label.to_string();
                record.detail = scrubbed;
                return ProviderAttempt { outcome, record };
            }
            EffectReceipt::TransportSucceeded { response } => response.clone(),
        };

        record.response_bytes = Measured::measured(raw.len());
        record.response_digest = Measured::measured(digest(&raw));

        let (status, body) = split_http(&raw);
        if let Some(code) = status {
            record.http_status = Measured::measured(code);
        }

        let Some(body) = body else {
            let reason = "response carried no body separator; nothing to parse".to_string();
            record.outcome_label = "measurement_failed".into();
            record.detail = reason.clone();
            return ProviderAttempt {
                outcome: InferenceOutcome::MeasurementFailed { reason },
                record,
            };
        };

        let accepted = status
            .map(|code| self.spec.response.success_statuses.contains(&code))
            .unwrap_or(false);
        if !accepted {
            let reason = format!(
                "http status {} is not in the authored success_statuses {:?}: {}",
                status
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "known_absent".to_string()),
                self.spec.response.success_statuses,
                snippet(body)
            );
            let reason = scrub(&reason, self.secret.as_deref());
            record.outcome_label = "measurement_failed".into();
            record.detail = reason.clone();
            return ProviderAttempt {
                outcome: InferenceOutcome::MeasurementFailed { reason },
                record,
            };
        }

        let parsed: serde_json::Value = match serde_json::from_slice(body) {
            Ok(value) => value,
            Err(error) => {
                let reason = scrub(
                    &format!(
                        "body of {} bytes did not parse as JSON: {error}: {}",
                        body.len(),
                        snippet(body)
                    ),
                    self.secret.as_deref(),
                );
                record.outcome_label = "measurement_failed".into();
                record.detail = reason.clone();
                return ProviderAttempt {
                    outcome: InferenceOutcome::MeasurementFailed { reason },
                    record,
                };
            }
        };

        if let Some(pointer) = &self.spec.response.model_pointer {
            match parsed.pointer(pointer).and_then(|value| value.as_str()) {
                Some(model) => record.answered_model = Measured::measured(model.to_string()),
                None => {
                    record.answered_model =
                        Measured::not_measured(format!("response has no string at {pointer}"))
                }
            }
        } else {
            record.answered_model =
                Measured::not_measured("the authored wire shape declares no model echo");
        }

        // Provider-side error object, if the wire shape declares one.
        if let Some(pointer) = &self.spec.response.error_pointer {
            if let Some(error) = parsed.pointer(pointer) {
                if !error.is_null() {
                    let reason = scrub(
                        &format!("provider reported an error at {pointer}: {}", trim(error)),
                        self.secret.as_deref(),
                    );
                    record.outcome_label = "measurement_failed".into();
                    record.detail = reason.clone();
                    return ProviderAttempt {
                        outcome: InferenceOutcome::MeasurementFailed { reason },
                        record,
                    };
                }
            }
        }

        // An explicit decline: the transport worked, the provider declined.
        if let (Some(pointer), Some(refusal)) = (
            &self.spec.response.stop_reason_pointer,
            &self.spec.response.refusal_stop_reason,
        ) {
            if let Some(stop) = parsed.pointer(pointer).and_then(|value| value.as_str()) {
                if stop == refusal {
                    let detail = format!("provider declined with {pointer} = {stop:?}");
                    record.outcome_label = "refused".into();
                    record.detail = detail.clone();
                    return ProviderAttempt {
                        outcome: InferenceOutcome::Refused {
                            category: stop.to_string(),
                            detail,
                        },
                        record,
                    };
                }
            }
        }

        match parsed
            .pointer(&self.spec.response.completion_pointer)
            .and_then(|value| value.as_str())
        {
            Some(completion) => {
                record.outcome_label = "answered".into();
                record.detail = format!("{} completion chars", completion.chars().count());
                ProviderAttempt {
                    outcome: InferenceOutcome::Answered {
                        completion: completion.to_string(),
                    },
                    record,
                }
            }
            None => {
                let reason = scrub(
                    &format!(
                        "response has no string at the authored completion_pointer {}: {}",
                        self.spec.response.completion_pointer,
                        trim(&parsed)
                    ),
                    self.secret.as_deref(),
                );
                record.outcome_label = "measurement_failed".into();
                record.detail = reason.clone();
                ProviderAttempt {
                    outcome: InferenceOutcome::MeasurementFailed { reason },
                    record,
                }
            }
        }
    }
}

fn snippet(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out: String = text.chars().take(MAX_DETAIL_BYTES).collect();
    if text.chars().count() > MAX_DETAIL_BYTES {
        out.push_str("...[truncated]");
    }
    out.replace(['\r', '\n'], " ")
}

fn trim(value: &serde_json::Value) -> String {
    snippet(value.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::StubTransport;
    use universe_core::Tick;

    fn spec(json: serde_json::Value) -> ProviderSpec {
        serde_json::from_value(json).expect("provider spec parses")
    }

    fn ollama_shaped(capability: &str) -> ProviderSpec {
        spec(serde_json::json!({
            "provider_id": "p-local",
            "capability": capability,
            "model": "qwen3-vl:2b-instruct",
            "transport": { "scheme": "http", "endpoint": "http://127.0.0.1:11434/api/generate", "timeout_ms": 5000 },
            "request_template": { "model": "{{model}}", "prompt": "{{prompt}}", "stream": false },
            "response": { "completion_pointer": "/response", "model_pointer": "/model" },
            "limits": { "max_prompt_bytes": 64, "max_request_bytes": 4096, "max_causal_depth": 2 }
        }))
    }

    fn request() -> InferenceRequest {
        InferenceRequest {
            turn_id: "turn-1".into(),
            actor_id: "actor:test".into(),
            observation: "choose a verb".into(),
            observed_at_revision: 1,
            dispatched_at_tick: Tick(1),
            deadline_tick: Tick(5),
            offered_verbs: vec!["inspect".into()],
            offered_targets: vec!["thing:a".into()],
            causal_ancestry: vec!["wake:1".into()],
        }
    }

    #[test]
    fn a_stubbed_wire_shape_is_read_with_the_authored_pointer() {
        let stub = StubTransport::new("stub", 200, br#"{"response":"inspect thing:a","model":"qwen3-vl:2b-instruct"}"#.to_vec());
        let mut provider = HttpJsonProvider::stubbed(ollama_shaped("infer.local"), Box::new(stub));
        let attempt = provider.infer(&request());
        assert_eq!(
            attempt.outcome,
            InferenceOutcome::Answered {
                completion: "inspect thing:a".into()
            }
        );
        assert!(attempt.record.transport_attempted);
        assert_eq!(attempt.record.http_status, Measured::measured(200));
        assert_eq!(
            attempt.record.answered_model,
            Measured::measured("qwen3-vl:2b-instruct".to_string())
        );
        assert!(provider.is_stubbed());
    }

    #[test]
    fn a_completely_different_wire_shape_needs_only_different_data() {
        // Anthropic Messages shape: nested content array, different pointer.
        let anthropic = spec(serde_json::json!({
            "provider_id": "p-remote",
            "capability": "infer.remote",
            "model": "claude-opus-5",
            "transport": { "scheme": "https", "endpoint": "https://api.anthropic.com/v1/messages", "timeout_ms": 5000 },
            "auth": { "kind": "none" },
            "extra_headers": { "anthropic-version": "2023-06-01" },
            "request_template": {
                "model": "{{model}}", "max_tokens": 64,
                "messages": [{ "role": "user", "content": "{{prompt}}" }]
            },
            "response": {
                "completion_pointer": "/content/0/text",
                "model_pointer": "/model",
                "stop_reason_pointer": "/stop_reason",
                "refusal_stop_reason": "refusal"
            }
        }));
        let stub = StubTransport::new(
            "stub",
            200,
            br#"{"model":"claude-opus-5","stop_reason":"end_turn","content":[{"type":"text","text":"inspect thing:a"}]}"#.to_vec(),
        );
        let mut provider = HttpJsonProvider::stubbed(anthropic, Box::new(stub));
        let attempt = provider.infer(&request());
        assert_eq!(
            attempt.outcome,
            InferenceOutcome::Answered {
                completion: "inspect thing:a".into()
            }
        );
        // Same Rust, entirely different wire shape.
        assert_eq!(attempt.record.requested_model, "claude-opus-5");
    }

    #[test]
    fn an_authored_refusal_stop_reason_is_refused_not_failed() {
        let anthropic = spec(serde_json::json!({
            "provider_id": "p-remote", "capability": "infer.remote", "model": "claude-opus-5",
            "transport": { "scheme": "https", "endpoint": "https://api.anthropic.com/v1/messages", "timeout_ms": 5000 },
            "request_template": { "model": "{{model}}", "messages": [{ "role": "user", "content": "{{prompt}}" }] },
            "response": {
                "completion_pointer": "/content/0/text",
                "stop_reason_pointer": "/stop_reason",
                "refusal_stop_reason": "refusal"
            }
        }));
        let stub = StubTransport::new("stub", 200, br#"{"stop_reason":"refusal","content":[]}"#.to_vec());
        let mut provider = HttpJsonProvider::stubbed(anthropic, Box::new(stub));
        let attempt = provider.infer(&request());
        assert_eq!(attempt.record.outcome_label, "refused");
        assert!(matches!(attempt.outcome, InferenceOutcome::Refused { .. }));
    }

    #[test]
    fn a_non_success_status_is_measured_failure_never_an_empty_completion() {
        let stub = StubTransport::new("stub", 500, br#"{"error":"boom"}"#.to_vec());
        let mut provider = HttpJsonProvider::stubbed(ollama_shaped("infer.local"), Box::new(stub));
        let attempt = provider.infer(&request());
        assert_eq!(attempt.record.outcome_label, "measurement_failed");
        assert_eq!(attempt.record.http_status, Measured::measured(500));
        assert!(matches!(
            attempt.outcome,
            InferenceOutcome::MeasurementFailed { .. }
        ));
    }

    #[test]
    fn a_missing_credential_is_not_configured_and_nothing_is_transported() {
        let mut authed = ollama_shaped("infer.local");
        authed.auth = AuthSpec::EnvHeader {
            env_var: "UNIVERSE_INFERENCE_TEST_KEY_DEFINITELY_UNSET".into(),
            header: "x-api-key".into(),
        };
        let stub = StubTransport::new("stub", 200, br#"{"response":"never"}"#.to_vec());
        let mut provider = HttpJsonProvider::new(authed, Box::new(stub));
        assert!(!provider.readiness().is_ready());
        let attempt = provider.infer(&request());
        assert!(!attempt.record.transport_attempted);
        match attempt.outcome {
            InferenceOutcome::NotConfigured { missing, .. } => {
                assert_eq!(missing, "UNIVERSE_INFERENCE_TEST_KEY_DEFINITELY_UNSET")
            }
            other => panic!("expected not_configured, got {other:?}"),
        }
    }

    #[test]
    fn the_declared_prompt_bound_stops_the_call_before_rendering() {
        let stub = StubTransport::new("stub", 200, br#"{"response":"never"}"#.to_vec());
        let mut provider = HttpJsonProvider::stubbed(ollama_shaped("infer.local"), Box::new(stub));
        let mut oversized = request();
        oversized.observation = "x".repeat(65); // declared max_prompt_bytes is 64
        let attempt = provider.infer(&oversized);
        assert!(!attempt.record.transport_attempted);
        assert_eq!(attempt.record.outcome_label, "not_attempted");
    }

    #[test]
    fn a_declared_causal_depth_limit_denies_before_transport() {
        let stub = StubTransport::new("stub", 200, br#"{"response":"never"}"#.to_vec());
        let mut provider = HttpJsonProvider::stubbed(ollama_shaped("infer.local"), Box::new(stub));
        let mut deep = request();
        deep.causal_ancestry = vec!["a".into(), "b".into(), "c".into()]; // declared max is 2
        let attempt = provider.infer(&deep);
        assert!(!attempt.record.transport_attempted);
        assert_eq!(attempt.record.outcome_label, "not_attempted");
        assert!(attempt.record.detail.contains("causal depth"), "{:?}", attempt.record.detail);
    }

    #[test]
    fn a_past_deadline_is_bounded_out_before_transport() {
        let stub = StubTransport::new("stub", 200, br#"{"response":"never"}"#.to_vec());
        let mut provider = HttpJsonProvider::stubbed(ollama_shaped("infer.local"), Box::new(stub));
        let mut late = request();
        late.dispatched_at_tick = Tick(9);
        late.deadline_tick = Tick(5);
        let attempt = provider.infer(&late);
        assert!(!attempt.record.transport_attempted);
        assert_eq!(attempt.record.outcome_label, "not_attempted");
        assert!(attempt.record.detail.contains("deadline"), "{:?}", attempt.record.detail);
    }

    #[test]
    fn a_credential_value_never_reaches_evidence_even_on_failure() {
        // A stub that fails with a message echoing the secret, which is the
        // worst realistic case (a provider quoting the key back in an error).
        struct Leaky(String);
        impl WireTransport for Leaky {
            fn transport_id(&self) -> &str {
                "leaky"
            }
            fn readiness(&self) -> TransportReadiness {
                TransportReadiness::Ready
            }
            fn send(&mut self, _request: &WireRequest) -> Result<Vec<u8>, String> {
                Err(format!("upstream rejected key {}", self.0))
            }
        }
        const KEY: &str = "sk-ant-test-DO-NOT-LEAK-0123456789";
        std::env::set_var("UNIVERSE_INFERENCE_TEST_KEY_PRESENT", KEY);
        let mut authed = ollama_shaped("infer.local");
        authed.auth = AuthSpec::EnvHeader {
            env_var: "UNIVERSE_INFERENCE_TEST_KEY_PRESENT".into(),
            header: "x-api-key".into(),
        };
        let mut provider =
            HttpJsonProvider::new(authed, Box::new(Leaky(KEY.to_string())));
        assert!(provider.readiness().is_ready());
        let attempt = provider.infer(&request());
        std::env::remove_var("UNIVERSE_INFERENCE_TEST_KEY_PRESENT");

        let evidence = serde_json::to_string(&attempt).unwrap();
        assert!(
            !evidence.contains(KEY),
            "credential leaked into evidence: {evidence}"
        );
        assert!(evidence.contains(CREDENTIAL_MARKER), "{evidence}");
        // Header NAMES are recorded so the call is explainable...
        assert!(attempt.record.header_names.iter().any(|n| n == "x-api-key"));
        // ...but no header VALUE is anywhere in the record.
        assert!(!evidence.contains("sk-ant-test"));
    }

    #[test]
    fn an_undeclared_capability_is_denied_before_transport() {
        // Build the provider with one capability, then ask the host for
        // another: the graph-owned registry denies it outright.
        let stub = StubTransport::new("stub", 200, br#"{"response":"never"}"#.to_vec());
        let mut provider = HttpJsonProvider::stubbed(ollama_shaped("infer.local"), Box::new(stub));
        let intent = EffectIntent {
            capability: "infer.undeclared".into(),
            idempotency_key: "probe".into(),
            payload: b"{}".to_vec(),
            deadline_tick: Tick(9),
            causal_ancestry: vec![],
        };
        assert!(matches!(
            provider.host.execute_measured(Tick(1), &intent),
            Err(UniverseError::CapabilityDenied(_))
        ));
    }
}
