//! Provider selection as Universe data.
//!
//! CLAUDE.md: *"If a rule can vary without changing the trusted computing base,
//! it belongs in the Universe."* Everything that can vary about collectivized
//! inference varies here, in graph content, not in Rust:
//!
//! * which providers exist, and their endpoint / model / decoding parameters;
//! * the **wire shape** of each provider — the request body is an authored JSON
//!   template with `{{prompt}}` / `{{model}}` slots, and the completion is read
//!   with an authored JSON pointer. Adding a third provider that speaks a new
//!   JSON dialect needs **zero** new Rust;
//! * which actor is routed to which chain, and with what priority;
//! * the fallback policy — `advance_on` lists the exact
//!   [`InferenceOutcome`](crate::contract::InferenceOutcome) labels that make
//!   the chain advance to the next provider. "Fall back when the local model is
//!   down but not when it refuses" is authored, not compiled;
//! * every bound: prompt size, timeout, attempts, deadline ticks, cost budget.
//!
//! What stays native is only: render a template, POST bytes, read a pointer,
//! walk a chain. No preference, no default provider, no implicit retry.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use universe_core::UniverseError;

/// How a provider's credential is supplied.
///
/// There is no variant that carries a literal secret. A key is read from the
/// process environment at call time and never enters graph content, a receipt,
/// or an error string.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthSpec {
    /// No credential required (a local endpoint).
    None,
    /// Read `env_var` from the environment and send it as `header`.
    /// If the variable is absent or empty the provider reports
    /// `not_configured` — it never sends an empty header and never pretends.
    EnvHeader { env_var: String, header: String },
    /// Read `env_var` and send it as `Authorization: <prefix> <value>`.
    EnvBearer { env_var: String, prefix: String },
}

impl AuthSpec {
    pub fn env_var(&self) -> Option<&str> {
        match self {
            AuthSpec::None => None,
            AuthSpec::EnvHeader { env_var, .. } | AuthSpec::EnvBearer { env_var, .. } => {
                Some(env_var.as_str())
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TransportSpec {
    /// `http` or `https`. Chooses the native byte transport. A scheme no
    /// transport can actually speak is a hard construction error, never a
    /// silent downgrade.
    pub scheme: String,
    pub endpoint: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default = "default_content_type")]
    pub content_type: String,
    pub timeout_ms: u64,
}

fn default_method() -> String {
    "POST".to_string()
}
fn default_content_type() -> String {
    "application/json".to_string()
}

/// How to read a provider's response. Pure data: JSON pointers into the body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResponseSpec {
    /// Pointer to the completion text, e.g. `/response` (Ollama) or
    /// `/content/0/text` (Anthropic Messages).
    pub completion_pointer: String,
    /// Pointer to the model the provider says answered. Absent means the wire
    /// shape carries no echo, and the attribution records `not_measured`.
    #[serde(default)]
    pub model_pointer: Option<String>,
    /// Pointer to a stop-reason / status field.
    #[serde(default)]
    pub stop_reason_pointer: Option<String>,
    /// The stop-reason value that means "the provider declined". When the
    /// pointer resolves to this, the outcome is `refused`, never `answered`
    /// and never `measurement_failed`.
    #[serde(default)]
    pub refusal_stop_reason: Option<String>,
    /// Pointer to a provider-side error object. When present in the body the
    /// outcome is `measurement_failed` carrying the provider's own message.
    #[serde(default)]
    pub error_pointer: Option<String>,
    /// HTTP statuses accepted as a reply worth parsing. Anything else is
    /// measured failure. Authored, so a provider with unusual success codes
    /// needs no code change.
    #[serde(default = "default_success_statuses")]
    pub success_statuses: Vec<u16>,
}

fn default_success_statuses() -> Vec<u16> {
    vec![200]
}

/// Per-provider declared bounds. These are enforced by the capability host
/// against real evidence; `None` means "this declaration does not bound it"
/// (unknown), never "unlimited by policy".
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderLimits {
    #[serde(default)]
    pub max_prompt_bytes: Option<u32>,
    #[serde(default)]
    pub max_request_bytes: Option<u32>,
    #[serde(default)]
    pub max_causal_depth: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderSpec {
    pub provider_id: String,
    /// Human-facing note on why this provider is in the collective. Carried so
    /// a route is explainable, per CLAUDE.md's justification requirement.
    #[serde(default)]
    pub justification: String,
    pub capability: String,
    pub model: String,
    pub transport: TransportSpec,
    #[serde(default = "auth_none")]
    pub auth: AuthSpec,
    /// Static headers, e.g. `anthropic-version`. Never credentials.
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    /// The request body as an authored JSON template. String values equal to
    /// `{{prompt}}` / `{{model}}` are replaced with the request's prompt and
    /// the provider's model. Everything else is transported verbatim.
    pub request_template: serde_json::Value,
    pub response: ResponseSpec,
    #[serde(default)]
    pub limits: ProviderLimits,
    /// Cost units this provider charges against a route's budget. Authored —
    /// the native floor has no idea what anything costs.
    #[serde(default)]
    pub cost_units: u64,
}

fn auth_none() -> AuthSpec {
    AuthSpec::None
}

/// One link in an authored fallback chain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChainLink {
    pub provider_id: String,
    /// Outcome labels that make the chain advance past this provider. An
    /// outcome not in this list ENDS the chain with that outcome. Empty means
    /// "this link is terminal whatever happens".
    #[serde(default)]
    pub advance_on: Vec<String>,
}

/// Which turns a route applies to. Matching is exact-id or catch-all; there is
/// no native heuristic, glob, or scoring.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RouteMatch {
    Actor { actor_id: String },
    Any { any: bool },
}

/// Bounds a route imposes on a whole turn, across all its attempts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteBudget {
    pub max_attempts: u32,
    pub max_prompt_bytes: u32,
    /// How many ticks after dispatch an answer may still land.
    pub deadline_ticks: u64,
    /// Total cost units the turn may spend across attempts.
    pub cost_units: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteSpec {
    pub route_id: String,
    #[serde(rename = "match")]
    pub matcher: RouteMatch,
    /// Higher wins. Ties are broken by `route_id` so selection is total and
    /// reproducible rather than dependent on document order.
    pub priority: i64,
    pub chain: Vec<ChainLink>,
    pub budget: RouteBudget,
    #[serde(default)]
    pub justification: String,
}

/// How the city admits inference results. This is the authored answer to
/// "what is the clock" — see [`crate::clock`] for the mechanism that enforces
/// it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdmissionSpec {
    /// Must be `wake_queue`: admission is ordered by the endogenous wake
    /// sequence the physics produced, NOT by which provider happened to answer
    /// first. Any other value is rejected at construction rather than silently
    /// treated as arrival order.
    pub order: String,
    /// Must be `tick_boundary`.
    pub clock: String,
    /// How many inferences may be in flight at once. This is the number that
    /// replaces "one call in flight".
    pub max_in_flight: u32,
    /// What to do with an answer whose observation revision is behind the
    /// world at admission. Must be `reject_with_receipt`.
    pub stale_policy: String,
}

/// The whole authored routing table.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RoutingTable {
    pub routing_id: String,
    pub version: String,
    #[serde(default)]
    pub objective: String,
    pub providers: Vec<ProviderSpec>,
    pub routes: Vec<RouteSpec>,
    pub admission: AdmissionSpec,
}

/// Where a routing table was read from. Carried into every attribution so a
/// run can never claim graph authority for a table that came off disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutingSource {
    /// Read back from a committed store, at an exact revision.
    Committed { store: String, revision: u64 },
    /// Read from an authoring fixture. Explicitly NOT committed authority.
    AuthoringFixture { path: String },
}

impl RoutingSource {
    pub fn describe(&self) -> String {
        match self {
            RoutingSource::Committed { store, revision } => {
                format!("committed store {store} @revision {revision}")
            }
            RoutingSource::AuthoringFixture { path } => {
                format!("authoring fixture {path} (NOT committed authority)")
            }
        }
    }
}

impl RoutingTable {
    /// Parse and validate an authored routing table.
    ///
    /// Validation is fail-closed: an unknown admission mode, a chain naming a
    /// provider that does not exist, a scheme no transport speaks, a template
    /// with no prompt slot, or a zero budget are all hard errors. A routing
    /// table is never partially honoured.
    pub fn parse(value: &serde_json::Value) -> Result<Self, UniverseError> {
        let table: RoutingTable = serde_json::from_value(value.clone())
            .map_err(|error| UniverseError::Validation(format!("routing table: {error}")))?;
        table.validate()?;
        Ok(table)
    }

    pub fn validate(&self) -> Result<(), UniverseError> {
        let invalid = |message: String| UniverseError::Validation(message);

        if self.providers.is_empty() {
            return Err(invalid("routing table declares no providers".into()));
        }
        if self.routes.is_empty() {
            return Err(invalid("routing table declares no routes".into()));
        }
        if self.admission.order != "wake_queue" {
            return Err(invalid(format!(
                "admission.order must be `wake_queue` (endogenous); \
                 {:?} would let provider latency schedule the city",
                self.admission.order
            )));
        }
        if self.admission.clock != "tick_boundary" {
            return Err(invalid(format!(
                "admission.clock must be `tick_boundary`; got {:?}",
                self.admission.clock
            )));
        }
        if self.admission.stale_policy != "reject_with_receipt" {
            return Err(invalid(format!(
                "admission.stale_policy must be `reject_with_receipt`; got {:?}",
                self.admission.stale_policy
            )));
        }
        if self.admission.max_in_flight == 0 {
            return Err(invalid("admission.max_in_flight must be >= 1".into()));
        }

        let mut seen = BTreeMap::new();
        for provider in &self.providers {
            if seen.insert(provider.provider_id.clone(), ()).is_some() {
                return Err(invalid(format!(
                    "duplicate provider_id {}",
                    provider.provider_id
                )));
            }
            match provider.transport.scheme.as_str() {
                "http" | "https" => {}
                other => {
                    return Err(invalid(format!(
                        "provider {} declares scheme {other:?}; only http and https \
                         have a native byte transport (no silent downgrade)",
                        provider.provider_id
                    )))
                }
            }
            if provider.transport.timeout_ms == 0 {
                return Err(invalid(format!(
                    "provider {} declares timeout_ms 0 — an unbounded call is never admitted",
                    provider.provider_id
                )));
            }
            if !template_has_slot(&provider.request_template, PROMPT_SLOT) {
                return Err(invalid(format!(
                    "provider {} request_template contains no {PROMPT_SLOT} slot, \
                     so the observation would never reach the model",
                    provider.provider_id
                )));
            }
            if provider.response.completion_pointer.is_empty() {
                return Err(invalid(format!(
                    "provider {} declares an empty completion_pointer",
                    provider.provider_id
                )));
            }
        }

        let mut route_ids = BTreeMap::new();
        for route in &self.routes {
            if route_ids.insert(route.route_id.clone(), ()).is_some() {
                return Err(invalid(format!("duplicate route_id {}", route.route_id)));
            }
            if route.chain.is_empty() {
                return Err(invalid(format!("route {} has an empty chain", route.route_id)));
            }
            if route.budget.max_attempts == 0 {
                return Err(invalid(format!(
                    "route {} allows 0 attempts",
                    route.route_id
                )));
            }
            if route.budget.deadline_ticks == 0 {
                return Err(invalid(format!(
                    "route {} declares deadline_ticks 0 — a turn must have a bound",
                    route.route_id
                )));
            }
            for link in &route.chain {
                if !seen.contains_key(&link.provider_id) {
                    return Err(invalid(format!(
                        "route {} chains to unknown provider {}",
                        route.route_id, link.provider_id
                    )));
                }
                for label in &link.advance_on {
                    if !KNOWN_OUTCOME_LABELS.contains(&label.as_str()) {
                        return Err(invalid(format!(
                            "route {} advance_on names unknown outcome label {label:?}; \
                             known labels are {KNOWN_OUTCOME_LABELS:?}",
                            route.route_id
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn provider(&self, provider_id: &str) -> Option<&ProviderSpec> {
        self.providers
            .iter()
            .find(|provider| provider.provider_id == provider_id)
    }

    /// Select the route for an actor. Exact-actor routes outrank catch-alls by
    /// authored priority; ties break on `route_id` so the result is total and
    /// does not depend on the order fields happen to appear in the document.
    ///
    /// Returns `None` when no route matches — the turn is then `not_attempted`
    /// with an explicit reason, never quietly sent to some default provider.
    pub fn route_for(&self, actor_id: &str) -> Option<&RouteSpec> {
        self.routes
            .iter()
            .filter(|route| match &route.matcher {
                RouteMatch::Actor { actor_id: wanted } => wanted == actor_id,
                RouteMatch::Any { any } => *any,
            })
            .max_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| b.route_id.cmp(&a.route_id))
            })
    }
}

pub const PROMPT_SLOT: &str = "{{prompt}}";
pub const MODEL_SLOT: &str = "{{model}}";

/// The outcome labels routing data may name in `advance_on`. Kept in sync with
/// [`crate::contract::InferenceOutcome::label`] by the test below.
pub const KNOWN_OUTCOME_LABELS: [&str; 5] = [
    "answered",
    "refused",
    "measurement_failed",
    "not_configured",
    "not_attempted",
];

fn template_has_slot(value: &serde_json::Value, slot: &str) -> bool {
    match value {
        serde_json::Value::String(text) => text.contains(slot),
        serde_json::Value::Array(items) => items.iter().any(|item| template_has_slot(item, slot)),
        serde_json::Value::Object(map) => {
            map.values().any(|item| template_has_slot(item, slot))
        }
        _ => false,
    }
}

/// Render an authored template by substituting the prompt and model slots.
///
/// Two properties matter here, and both are load-bearing:
///
/// 1. Substitution happens on the JSON *value* tree, so the prompt is escaped
///    by `serde_json` on serialization — a prompt containing quotes, newlines
///    or braces can never break out of the body it is placed in.
/// 2. Substitution is a SINGLE left-to-right pass, so inserted content is
///    never rescanned. A naive `replace(prompt).replace(model)` would let an
///    observation containing the literal text `{{model}}` have its own words
///    silently rewritten — the observation is graph-written text and must be
///    transported verbatim.
pub fn render_template(
    template: &serde_json::Value,
    prompt: &str,
    model: &str,
) -> serde_json::Value {
    match template {
        serde_json::Value::String(text) => {
            serde_json::Value::String(substitute_slots(text, prompt, model))
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| render_template(item, prompt, model))
                .collect(),
        ),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), render_template(item, prompt, model)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// One left-to-right pass. Substituted content is appended to the output and
/// never re-examined, so slot syntax occurring inside the prompt or the model
/// name is transported literally.
fn substitute_slots(text: &str, prompt: &str, model: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix(PROMPT_SLOT) {
            out.push_str(prompt);
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix(MODEL_SLOT) {
            out.push_str(model);
            rest = tail;
        } else {
            let character = rest.chars().next().expect("rest is non-empty");
            out.push(character);
            rest = &rest[character.len_utf8()..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::InferenceOutcome;

    fn minimal() -> serde_json::Value {
        serde_json::json!({
            "routing_id": "t", "version": "1",
            "providers": [{
                "provider_id": "p1",
                "capability": "infer.p1",
                "model": "m1",
                "transport": { "scheme": "http", "endpoint": "http://127.0.0.1:1/x", "timeout_ms": 100 },
                "request_template": { "model": "{{model}}", "prompt": "{{prompt}}" },
                "response": { "completion_pointer": "/response" }
            }],
            "routes": [{
                "route_id": "r1",
                "match": { "any": true },
                "priority": 0,
                "chain": [{ "provider_id": "p1", "advance_on": [] }],
                "budget": { "max_attempts": 1, "max_prompt_bytes": 100, "deadline_ticks": 2, "cost_units": 1 }
            }],
            "admission": {
                "order": "wake_queue", "clock": "tick_boundary",
                "max_in_flight": 4, "stale_policy": "reject_with_receipt"
            }
        })
    }

    #[test]
    fn known_labels_match_the_contract_exactly() {
        // If a new InferenceOutcome arm is added without extending this list,
        // authored routing could never react to it. Pin the correspondence.
        let arms = [
            InferenceOutcome::Answered {
                completion: String::new(),
            },
            InferenceOutcome::Refused {
                category: String::new(),
                detail: String::new(),
            },
            InferenceOutcome::MeasurementFailed {
                reason: String::new(),
            },
            InferenceOutcome::NotConfigured {
                missing: String::new(),
                detail: String::new(),
            },
            InferenceOutcome::NotAttempted {
                reason: String::new(),
            },
        ];
        for arm in &arms {
            assert!(
                KNOWN_OUTCOME_LABELS.contains(&arm.label()),
                "outcome {:?} is not addressable from routing data",
                arm.label()
            );
        }
        assert_eq!(arms.len(), KNOWN_OUTCOME_LABELS.len());
    }

    #[test]
    fn arrival_order_admission_is_refused_at_parse_time() {
        let mut value = minimal();
        value["admission"]["order"] = serde_json::json!("arrival");
        let error = RoutingTable::parse(&value).unwrap_err();
        assert!(
            format!("{error}").contains("wake_queue"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn unbounded_declarations_are_hard_errors() {
        let mut no_timeout = minimal();
        no_timeout["providers"][0]["transport"]["timeout_ms"] = serde_json::json!(0);
        assert!(RoutingTable::parse(&no_timeout).is_err());

        let mut no_deadline = minimal();
        no_deadline["routes"][0]["budget"]["deadline_ticks"] = serde_json::json!(0);
        assert!(RoutingTable::parse(&no_deadline).is_err());

        let mut no_attempts = minimal();
        no_attempts["routes"][0]["budget"]["max_attempts"] = serde_json::json!(0);
        assert!(RoutingTable::parse(&no_attempts).is_err());
    }

    #[test]
    fn a_template_that_drops_the_observation_is_refused() {
        let mut value = minimal();
        value["providers"][0]["request_template"] =
            serde_json::json!({ "model": "{{model}}", "prompt": "hardcoded" });
        let error = RoutingTable::parse(&value).unwrap_err();
        assert!(format!("{error}").contains("{{prompt}}"), "unexpected: {error}");
    }

    #[test]
    fn chain_to_unknown_provider_is_refused() {
        let mut value = minimal();
        value["routes"][0]["chain"][0]["provider_id"] = serde_json::json!("ghost");
        assert!(RoutingTable::parse(&value).is_err());
    }

    #[test]
    fn advance_on_with_an_unknown_label_is_refused() {
        let mut value = minimal();
        value["routes"][0]["chain"][0]["advance_on"] = serde_json::json!(["sometimes"]);
        let error = RoutingTable::parse(&value).unwrap_err();
        assert!(format!("{error}").contains("unknown outcome label"), "{error}");
    }

    #[test]
    fn an_unsupported_scheme_is_refused_rather_than_downgraded() {
        let mut value = minimal();
        value["providers"][0]["transport"]["scheme"] = serde_json::json!("grpc");
        let error = RoutingTable::parse(&value).unwrap_err();
        assert!(format!("{error}").contains("no silent downgrade"), "{error}");
    }

    #[test]
    fn exact_actor_route_outranks_catch_all_and_selection_is_total() {
        let mut value = minimal();
        value["routes"] = serde_json::json!([
            { "route_id": "r-any", "match": {"any": true}, "priority": 0,
              "chain": [{"provider_id":"p1","advance_on":[]}],
              "budget": {"max_attempts":1,"max_prompt_bytes":100,"deadline_ticks":2,"cost_units":1} },
            { "route_id": "r-captain", "match": {"actor_id": "actor:captain"}, "priority": 10,
              "chain": [{"provider_id":"p1","advance_on":[]}],
              "budget": {"max_attempts":1,"max_prompt_bytes":100,"deadline_ticks":2,"cost_units":1} }
        ]);
        let table = RoutingTable::parse(&value).unwrap();
        assert_eq!(table.route_for("actor:captain").unwrap().route_id, "r-captain");
        assert_eq!(table.route_for("actor:someone-else").unwrap().route_id, "r-any");
    }

    #[test]
    fn rendering_escapes_a_hostile_prompt_instead_of_breaking_the_body() {
        let template = serde_json::json!({ "model": "{{model}}", "prompt": "{{prompt}}" });
        // A prompt that tries to (a) break out of its JSON string and inject a
        // sibling key, and (b) smuggle slot syntax so a second substitution
        // pass would rewrite it.
        let hostile = "\"}, \"stream\": true, \"x\": \"\n{{model}}";
        let rendered = render_template(&template, hostile, "m1");
        // The prompt lands as ONE string value: quotes, newline AND its
        // literal `{{model}}` all intact, because substitution is single-pass.
        assert_eq!(rendered["prompt"], serde_json::json!(hostile));
        assert_eq!(rendered["model"], serde_json::json!("m1"));
        // Round-tripping through the wire keeps exactly two keys — the prompt
        // could not inject `stream`.
        let wire = serde_json::to_vec(&rendered).unwrap();
        let back: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(back.as_object().unwrap().len(), 2);
        assert_eq!(back["prompt"], serde_json::json!(hostile));
    }

    #[test]
    fn slot_syntax_inside_the_observation_is_transported_verbatim() {
        // The observation is graph-written text. If a thing in the world is
        // literally named `{{model}}`, the model must see that, not the model
        // id. A second replace pass would corrupt it silently.
        let template = serde_json::json!({ "model": "{{model}}", "prompt": "{{prompt}}" });
        let observation = "visible: thing:{{model}} and thing:{{prompt}}";
        let rendered = render_template(&template, observation, "qwen3-vl:2b-instruct");
        assert_eq!(rendered["prompt"], serde_json::json!(observation));
        assert_eq!(rendered["model"], serde_json::json!("qwen3-vl:2b-instruct"));
    }

    #[test]
    fn nested_template_slots_render() {
        let template = serde_json::json!({
            "model": "{{model}}",
            "messages": [{ "role": "user", "content": "{{prompt}}" }]
        });
        let rendered = render_template(&template, "hello", "claude-opus-5");
        assert_eq!(rendered["messages"][0]["content"], serde_json::json!("hello"));
        assert_eq!(rendered["model"], serde_json::json!("claude-opus-5"));
    }
}
