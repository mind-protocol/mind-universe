//! What the Anthropic provider would put on the wire, proven without a key.
//!
//! No `ANTHROPIC_API_KEY` is present in this environment, so the remote
//! provider is never really called anywhere in this crate. That is exactly the
//! situation in which it is easiest to ship a provider that is wired wrong and
//! never find out. These tests therefore assert the *whole* outbound request
//! byte-for-byte against a capturing transport, driven by the SAME authored
//! fixture the proof binary commits — so a wiring mistake fails here rather
//! than on someone's first paid call.
//!
//! Everything here is stubbed. Nothing in this file measures Anthropic.

use std::sync::{Arc, Mutex};

use universe_inference::contract::{InferenceProvider, InferenceRequest};
use universe_inference::routing::{ProviderSpec, RoutingTable};
use universe_inference::transport::{TransportReadiness, WireRequest, WireTransport};
use universe_inference::{HttpJsonProvider, InferenceOutcome, CREDENTIAL_MARKER};
use universe_core::Tick;

const FIXTURE: &str = include_str!("../fixtures/inference-routing-v0.json");
const ANTHROPIC_PROVIDER: &str = "anthropic-claude-opus-5";
const FAKE_KEY: &str = "sk-ant-api03-FAKE-KEY-FOR-WIRING-TESTS-ONLY";

/// Captures the request and returns a scripted response.
struct Capture {
    seen: Arc<Mutex<Vec<WireRequest>>>,
    status: u16,
    body: Vec<u8>,
}

impl WireTransport for Capture {
    fn transport_id(&self) -> &str {
        "capture"
    }
    fn readiness(&self) -> TransportReadiness {
        TransportReadiness::Ready
    }
    fn send(&mut self, request: &WireRequest) -> Result<Vec<u8>, String> {
        self.seen.lock().unwrap().push(request.clone());
        let mut raw = format!(
            "HTTP/1.1 {} CAPTURED\r\ncontent-type: application/json\r\n\r\n",
            self.status
        )
        .into_bytes();
        raw.extend_from_slice(&self.body);
        Ok(raw)
    }
}

fn table() -> RoutingTable {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture is JSON");
    RoutingTable::parse(fixture.get("content").expect("fixture has content"))
        .expect("authored routing table is valid")
}

fn anthropic_spec() -> ProviderSpec {
    table()
        .provider(ANTHROPIC_PROVIDER)
        .cloned()
        .expect("fixture declares the anthropic provider")
}

fn request(prompt: &str) -> InferenceRequest {
    InferenceRequest {
        turn_id: "turn-wiring".into(),
        actor_id: "actor:l1:mind-universe:remote-only".into(),
        observation: prompt.into(),
        observed_at_revision: 1,
        dispatched_at_tick: Tick(1),
        deadline_tick: Tick(9),
        offered_verbs: vec!["inspect".into()],
        offered_targets: vec!["thing:beacon-a".into()],
        causal_ancestry: vec!["wake:1".into()],
    }
}

fn run(
    status: u16,
    body: &str,
    prompt: &str,
) -> (
    universe_inference::ProviderAttempt,
    Vec<WireRequest>,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let capture = Capture {
        seen: Arc::clone(&seen),
        status,
        body: body.as_bytes().to_vec(),
    };
    let mut provider = HttpJsonProvider::new(anthropic_spec(), Box::new(capture));
    let attempt = provider.infer(&request(prompt));
    let captured = seen.lock().unwrap().clone();
    (attempt, captured)
}

/// One test, run serially, because it mutates a process-wide env var.
#[test]
fn the_anthropic_provider_is_wired_to_the_messages_api() {
    // ---- with no credential, nothing is transported at all ---------------
    std::env::remove_var("ANTHROPIC_API_KEY");
    let (attempt, captured) = run(200, "{}", "unused");
    assert!(
        captured.is_empty(),
        "an unconfigured provider must not transport anything"
    );
    match &attempt.outcome {
        InferenceOutcome::NotConfigured { missing, .. } => {
            assert_eq!(missing, "ANTHROPIC_API_KEY")
        }
        other => panic!("expected not_configured, got {other:?}"),
    }

    // ---- with a credential, assert the ENTIRE outbound request -----------
    std::env::set_var("ANTHROPIC_API_KEY", FAKE_KEY);

    let prompt = "Available verbs: inspect\nReachable targets: thing:beacon-a\n";
    let success = r#"{"id":"msg_01","type":"message","role":"assistant","model":"claude-opus-5","stop_reason":"end_turn","content":[{"type":"text","text":"inspect thing:beacon-a"}]}"#;
    let (attempt, captured) = run(200, success, prompt);
    assert_eq!(captured.len(), 1, "exactly one request");
    let sent = &captured[0];

    // Endpoint, method and headers.
    assert_eq!(sent.endpoint, "https://api.anthropic.com/v1/messages");
    assert_eq!(sent.method, "POST");
    let header = |name: &str| {
        sent.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    assert_eq!(header("content-type").as_deref(), Some("application/json"));
    assert_eq!(header("anthropic-version").as_deref(), Some("2023-06-01"));
    assert_eq!(
        header("x-api-key").as_deref(),
        Some(FAKE_KEY),
        "the credential must go in x-api-key, read from the env at call time"
    );
    assert!(
        header("authorization").is_none(),
        "an API key belongs on x-api-key, not Authorization"
    );

    // The body must be a valid Messages API request.
    let body: serde_json::Value =
        serde_json::from_slice(&sent.body).expect("request body is JSON");
    assert_eq!(body["model"], serde_json::json!("claude-opus-5"));
    assert_eq!(body["max_tokens"], serde_json::json!(256));
    assert!(body["system"].is_string(), "system prompt is authored data");
    let messages = body["messages"].as_array().expect("messages is an array");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], serde_json::json!("user"));
    assert_eq!(
        messages[0]["content"],
        serde_json::json!(prompt),
        "the observation must reach the model verbatim"
    );
    // No stray keys that the Messages API would reject.
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        vec!["max_tokens", "messages", "model", "system"],
        "unexpected top-level request keys"
    );

    // And the completion is read with the authored pointer.
    assert_eq!(
        attempt.outcome,
        InferenceOutcome::Answered {
            completion: "inspect thing:beacon-a".into()
        }
    );
    assert_eq!(
        attempt.record.answered_model.value().map(String::as_str),
        Some("claude-opus-5")
    );

    // ---- a refusal stop reason is a refusal, not a failure ---------------
    let refusal = r#"{"model":"claude-opus-5","stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"},"content":[]}"#;
    let (attempt, _) = run(200, refusal, prompt);
    match &attempt.outcome {
        InferenceOutcome::Refused { category, .. } => assert_eq!(category, "refusal"),
        other => panic!("expected refused, got {other:?}"),
    }

    // ---- a 401 is measured failure, and never leaks the key --------------
    let unauthorized =
        r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
    let (attempt, _) = run(401, unauthorized, prompt);
    assert!(matches!(
        attempt.outcome,
        InferenceOutcome::MeasurementFailed { .. }
    ));
    let evidence = serde_json::to_string(&attempt).expect("attempt serializes");
    assert!(
        !evidence.contains(FAKE_KEY),
        "credential leaked into evidence: {evidence}"
    );
    assert!(
        attempt
            .record
            .header_names
            .iter()
            .any(|name| name == "x-api-key"),
        "the header NAME is recorded so the call stays explainable"
    );

    // ---- an upstream error quoting the key back is scrubbed --------------
    struct Leaky;
    impl WireTransport for Leaky {
        fn transport_id(&self) -> &str {
            "leaky"
        }
        fn readiness(&self) -> TransportReadiness {
            TransportReadiness::Ready
        }
        fn send(&mut self, _request: &WireRequest) -> Result<Vec<u8>, String> {
            Err(format!("proxy rejected credential {FAKE_KEY}"))
        }
    }
    let mut provider = HttpJsonProvider::new(anthropic_spec(), Box::new(Leaky));
    let attempt = provider.infer(&request(prompt));
    let evidence = serde_json::to_string(&attempt).expect("attempt serializes");
    assert!(!evidence.contains(FAKE_KEY), "credential leaked: {evidence}");
    assert!(evidence.contains(CREDENTIAL_MARKER));

    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[test]
fn the_authored_fixture_is_a_valid_routing_table() {
    let table = table();
    assert_eq!(table.routing_id, "inference-routing-v0");
    assert_eq!(table.admission.order, "wake_queue");
    assert_eq!(table.admission.clock, "tick_boundary");

    // The captain falls back to the remote model on failure but NOT on a
    // refusal — that is authored policy, and it is what makes this a
    // collective rather than a retry loop.
    let captain = table
        .route_for("actor:l1:mind-universe:captain")
        .expect("captain route");
    assert_eq!(captain.route_id, "route:l1:captain");
    assert_eq!(captain.chain[0].provider_id, "local-ollama-qwen3vl-2b");
    assert!(captain.chain[0]
        .advance_on
        .contains(&"measurement_failed".to_string()));
    assert!(
        !captain.chain[0].advance_on.contains(&"refused".to_string()),
        "buying a second opinion on a refusal is policy the city did not author"
    );
    assert_eq!(captain.chain[1].provider_id, ANTHROPIC_PROVIDER);

    // An unrouted actor gets the local-only catch-all: never a silent spend.
    let default = table.route_for("actor:l1:nobody").expect("catch-all route");
    assert_eq!(default.route_id, "route:default");
    assert!(default
        .chain
        .iter()
        .all(|link| link.provider_id != ANTHROPIC_PROVIDER));
}
