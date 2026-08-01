//! Walking an authored provider chain.
//!
//! The router is a interpreter for routing data and nothing else. It holds no
//! preference between providers, no default, no implicit retry, no timeout of
//! its own. Every decision it makes — which route, which provider, when to
//! advance, when to stop — is read out of the [`RoutingTable`].
//!
//! One `CollectiveRouter` is ONE dispatch lane: `dispatch` is synchronous and
//! borrows its providers mutably. Collectivized inference gets its parallelism
//! by running several lanes (or several routers) concurrently and landing their
//! results into a single [`AdmissionGate`](crate::clock::AdmissionGate) — the
//! gate, not the router, is what re-serialises the city.

use std::collections::BTreeMap;

use universe_core::Tick;

use crate::contract::{
    digest, InferenceAttribution, InferenceObservation, InferenceOutcome, InferenceProvider,
    InferenceRequest,
};
use crate::routing::{RoutingSource, RoutingTable};

pub struct CollectiveRouter {
    table: RoutingTable,
    source: RoutingSource,
    providers: BTreeMap<String, Box<dyn InferenceProvider>>,
}

impl CollectiveRouter {
    pub fn new(table: RoutingTable, source: RoutingSource) -> Self {
        Self {
            table,
            source,
            providers: BTreeMap::new(),
        }
    }

    /// Register a provider instance for an authored `provider_id`.
    pub fn install(&mut self, provider: Box<dyn InferenceProvider>) {
        self.providers
            .insert(provider.provider_id().to_string(), provider);
    }

    pub fn table(&self) -> &RoutingTable {
        &self.table
    }

    pub fn source(&self) -> &RoutingSource {
        &self.source
    }

    /// Provider ids the routing names but that were never installed. A chain
    /// reaching one of these produces `not_attempted` with an explicit reason
    /// — never a silent skip to the next link.
    pub fn uninstalled_providers(&self) -> Vec<String> {
        self.table
            .providers
            .iter()
            .map(|provider| provider.provider_id.clone())
            .filter(|id| !self.providers.contains_key(id))
            .collect()
    }

    /// Run one turn through its authored chain.
    ///
    /// Returns the outcome that ENDED the chain plus attribution covering every
    /// attempt that got there. Fallbacks are visible, never hidden.
    pub fn dispatch(&mut self, request: &InferenceRequest, now: Tick) -> InferenceObservation {
        let prompt_digest = digest(request.observation.as_bytes());
        let prompt_bytes = request.observation.len();

        let Some(route) = self.table.route_for(&request.actor_id).cloned() else {
            let reason = format!(
                "routing table {} v{} has no route matching actor {}",
                self.table.routing_id, self.table.version, request.actor_id
            );
            return InferenceObservation {
                outcome: InferenceOutcome::NotAttempted {
                    reason: reason.clone(),
                },
                attribution: InferenceAttribution {
                    turn_id: request.turn_id.clone(),
                    actor_id: request.actor_id.clone(),
                    route_id: "known_absent".into(),
                    routing_id: self.table.routing_id.clone(),
                    routing_version: self.table.version.clone(),
                    routing_source: self.source.describe(),
                    attempts: Vec::new(),
                    served_by: None,
                    dispatched_at_tick: request.dispatched_at_tick,
                    observed_at_tick: now,
                    deadline_tick: request.deadline_tick,
                    observed_at_revision: request.observed_at_revision,
                    total_latency_ms: 0,
                    prompt_bytes,
                    prompt_digest,
                    budget_charged: 0,
                    budget_allowed: 0,
                },
            };
        };

        let mut attempts = Vec::new();
        let mut charged = 0u64;
        let mut total_latency = 0u64;
        let mut served_by = None;

        // Route-level prompt bound, checked once for the whole turn.
        let mut outcome = if prompt_bytes as u64 > u64::from(route.budget.max_prompt_bytes) {
            InferenceOutcome::NotAttempted {
                reason: format!(
                    "prompt {prompt_bytes} bytes exceeds route {} budget.max_prompt_bytes {}",
                    route.route_id, route.budget.max_prompt_bytes
                ),
            }
        } else {
            InferenceOutcome::NotAttempted {
                reason: format!("route {} chain made no attempt", route.route_id),
            }
        };
        let prompt_within_budget = prompt_bytes as u64 <= u64::from(route.budget.max_prompt_bytes);

        if prompt_within_budget {
            for link in &route.chain {
                if attempts.len() as u64 >= u64::from(route.budget.max_attempts) {
                    outcome = InferenceOutcome::NotAttempted {
                        reason: format!(
                            "route {} budget.max_attempts {} exhausted before provider {}",
                            route.route_id, route.budget.max_attempts, link.provider_id
                        ),
                    };
                    break;
                }

                let Some(provider) = self.providers.get_mut(&link.provider_id) else {
                    outcome = InferenceOutcome::NotAttempted {
                        reason: format!(
                            "routing names provider {} but no instance is installed",
                            link.provider_id
                        ),
                    };
                    break;
                };

                let cost = self
                    .table
                    .providers
                    .iter()
                    .find(|spec| spec.provider_id == link.provider_id)
                    .map(|spec| spec.cost_units)
                    .unwrap_or(0);
                if charged.saturating_add(cost) > route.budget.cost_units {
                    outcome = InferenceOutcome::NotAttempted {
                        reason: format!(
                            "route {} budget.cost_units {} would be exceeded by provider {} \
                             (cost {cost}, already charged {charged})",
                            route.route_id, route.budget.cost_units, link.provider_id
                        ),
                    };
                    break;
                }

                let attempt = provider.infer(request);
                // Charge only for attempts that actually reached the provider.
                // A `not_configured` link never sent a byte and costs nothing;
                // charging it would let a misconfigured provider silently
                // consume the budget its own fallback needs. Attempt COUNT is
                // still bounded independently by `budget.max_attempts`.
                if attempt.record.transport_attempted {
                    charged = charged.saturating_add(cost);
                }
                total_latency = total_latency.saturating_add(attempt.record.latency_ms);
                let label = attempt.outcome.label().to_string();
                if attempt.outcome.is_answered() {
                    served_by = Some(link.provider_id.clone());
                }
                attempts.push(attempt.record);
                outcome = attempt.outcome;

                // ADVANCE is authored. An outcome not named in `advance_on`
                // ends the chain with that outcome.
                if !link.advance_on.iter().any(|allowed| allowed == &label) {
                    break;
                }
            }
        }

        InferenceObservation {
            outcome,
            attribution: InferenceAttribution {
                turn_id: request.turn_id.clone(),
                actor_id: request.actor_id.clone(),
                route_id: route.route_id.clone(),
                routing_id: self.table.routing_id.clone(),
                routing_version: self.table.version.clone(),
                routing_source: self.source.describe(),
                attempts,
                served_by,
                dispatched_at_tick: request.dispatched_at_tick,
                observed_at_tick: now,
                deadline_tick: request.deadline_tick,
                observed_at_revision: request.observed_at_revision,
                total_latency_ms: total_latency,
                prompt_bytes,
                prompt_digest,
                budget_charged: charged,
                budget_allowed: route.budget.cost_units,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{AttemptRecord, Measured, ProviderAttempt, ProviderReadiness};

    /// A provider that returns a scripted outcome without any transport, so a
    /// chain-walking test measures the ROUTER, not the network.
    struct Scripted {
        id: String,
        outcome: InferenceOutcome,
        calls: usize,
        transported: bool,
    }

    impl Scripted {
        fn new(id: &str, outcome: InferenceOutcome) -> Self {
            Self {
                id: id.into(),
                outcome,
                calls: 0,
                transported: true,
            }
        }
    }

    impl InferenceProvider for Scripted {
        fn provider_id(&self) -> &str {
            &self.id
        }
        fn readiness(&self) -> ProviderReadiness {
            ProviderReadiness::Ready
        }
        fn infer(&mut self, _request: &InferenceRequest) -> ProviderAttempt {
            self.calls += 1;
            ProviderAttempt {
                outcome: self.outcome.clone(),
                record: AttemptRecord {
                    provider_id: self.id.clone(),
                    endpoint: "scripted://".into(),
                    requested_model: "scripted".into(),
                    answered_model: Measured::not_measured("scripted"),
                    capability: format!("infer.{}", self.id),
                    header_names: vec![],
                    transport_attempted: self.transported,
                    effect_idempotency_key: format!("scripted:{}", self.id),
                    http_status: Measured::not_measured("scripted"),
                    outcome_label: self.outcome.label().into(),
                    detail: "scripted".into(),
                    request_bytes: 0,
                    response_bytes: Measured::not_measured("scripted"),
                    latency_ms: 1,
                    request_digest: String::new(),
                    response_digest: Measured::not_measured("scripted"),
                },
            }
        }
    }

    fn table(chain: serde_json::Value, budget: serde_json::Value) -> RoutingTable {
        let value = serde_json::json!({
            "routing_id": "t", "version": "1",
            "providers": [
                { "provider_id": "a", "capability": "infer.a", "model": "m", "cost_units": 1,
                  "transport": {"scheme":"http","endpoint":"http://127.0.0.1:1/x","timeout_ms":100},
                  "request_template": {"prompt":"{{prompt}}"},
                  "response": {"completion_pointer":"/response"} },
                { "provider_id": "b", "capability": "infer.b", "model": "m", "cost_units": 5,
                  "transport": {"scheme":"http","endpoint":"http://127.0.0.1:1/x","timeout_ms":100},
                  "request_template": {"prompt":"{{prompt}}"},
                  "response": {"completion_pointer":"/response"} }
            ],
            "routes": [{ "route_id": "r", "match": {"any": true}, "priority": 0,
                         "chain": chain, "budget": budget }],
            "admission": { "order":"wake_queue", "clock":"tick_boundary",
                           "max_in_flight": 4, "stale_policy":"reject_with_receipt" }
        });
        RoutingTable::parse(&value).expect("table parses")
    }

    fn request() -> InferenceRequest {
        InferenceRequest {
            turn_id: "turn-1".into(),
            actor_id: "actor:any".into(),
            observation: "obs".into(),
            observed_at_revision: 1,
            dispatched_at_tick: Tick(1),
            deadline_tick: Tick(5),
            offered_verbs: vec!["inspect".into()],
            offered_targets: vec!["thing:a".into()],
            causal_ancestry: vec![],
        }
    }

    fn source() -> RoutingSource {
        RoutingSource::AuthoringFixture {
            path: "test".into(),
        }
    }

    fn generous() -> serde_json::Value {
        serde_json::json!({"max_attempts": 4, "max_prompt_bytes": 4096, "deadline_ticks": 3, "cost_units": 100})
    }

    #[test]
    fn fallback_happens_only_on_authored_labels() {
        // Authored: advance past `a` on measurement_failed. `a` fails, so `b`
        // answers.
        let table = table(
            serde_json::json!([
                {"provider_id": "a", "advance_on": ["measurement_failed"]},
                {"provider_id": "b", "advance_on": []}
            ]),
            generous(),
        );
        let mut router = CollectiveRouter::new(table, source());
        router.install(Box::new(Scripted::new(
            "a",
            InferenceOutcome::MeasurementFailed {
                reason: "down".into(),
            },
        )));
        router.install(Box::new(Scripted::new(
            "b",
            InferenceOutcome::Answered {
                completion: "inspect thing:a".into(),
            },
        )));
        let observation = router.dispatch(&request(), Tick(2));
        assert!(observation.outcome.is_answered());
        assert_eq!(observation.attribution.attempts.len(), 2);
        assert_eq!(observation.attribution.served_by.as_deref(), Some("b"));
        // Both cost units were charged, and the fallback is visible.
        assert_eq!(observation.attribution.budget_charged, 6);
        assert_eq!(observation.attribution.attempts[0].provider_id, "a");
    }

    #[test]
    fn an_outcome_outside_advance_on_ends_the_chain() {
        // Same providers, but the authored policy is "do NOT fall back on a
        // refusal". Changing only DATA changes the behaviour.
        let table = table(
            serde_json::json!([
                {"provider_id": "a", "advance_on": ["measurement_failed"]},
                {"provider_id": "b", "advance_on": []}
            ]),
            generous(),
        );
        let mut router = CollectiveRouter::new(table, source());
        router.install(Box::new(Scripted::new(
            "a",
            InferenceOutcome::Refused {
                category: "policy".into(),
                detail: "declined".into(),
            },
        )));
        router.install(Box::new(Scripted::new(
            "b",
            InferenceOutcome::Answered {
                completion: "never reached".into(),
            },
        )));
        let observation = router.dispatch(&request(), Tick(2));
        assert!(matches!(observation.outcome, InferenceOutcome::Refused { .. }));
        assert_eq!(observation.attribution.attempts.len(), 1);
        assert_eq!(observation.attribution.served_by, None);
    }

    #[test]
    fn a_call_that_never_transported_costs_nothing() {
        // Provider `a` is unconfigured and never sends a byte; `b` costs 5.
        // If the unconfigured link were charged, the route's 6-unit budget
        // would be eaten before the fallback it exists to reach.
        let table = table(
            serde_json::json!([
                {"provider_id": "a", "advance_on": ["not_configured"]},
                {"provider_id": "b", "advance_on": []}
            ]),
            serde_json::json!({"max_attempts": 4, "max_prompt_bytes": 4096, "deadline_ticks": 3, "cost_units": 6}),
        );
        let mut router = CollectiveRouter::new(table, source());
        let mut unconfigured = Scripted::new(
            "a",
            InferenceOutcome::NotConfigured {
                missing: "SOME_KEY".into(),
                detail: "unset".into(),
            },
        );
        unconfigured.transported = false;
        router.install(Box::new(unconfigured));
        router.install(Box::new(Scripted::new(
            "b",
            InferenceOutcome::Answered {
                completion: "ok".into(),
            },
        )));
        let observation = router.dispatch(&request(), Tick(2));
        assert!(observation.outcome.is_answered());
        // Only the provider that actually transported was charged.
        assert_eq!(observation.attribution.budget_charged, 5);
    }

    #[test]
    fn not_configured_is_an_authorable_fallback_trigger() {
        let table = table(
            serde_json::json!([
                {"provider_id": "a", "advance_on": ["not_configured"]},
                {"provider_id": "b", "advance_on": []}
            ]),
            generous(),
        );
        let mut router = CollectiveRouter::new(table, source());
        router.install(Box::new(Scripted::new(
            "a",
            InferenceOutcome::NotConfigured {
                missing: "SOME_KEY".into(),
                detail: "unset".into(),
            },
        )));
        router.install(Box::new(Scripted::new(
            "b",
            InferenceOutcome::Answered {
                completion: "ok".into(),
            },
        )));
        let observation = router.dispatch(&request(), Tick(2));
        assert!(observation.outcome.is_answered());
        assert_eq!(observation.attribution.attempts[0].outcome_label, "not_configured");
    }

    #[test]
    fn the_cost_budget_stops_an_expensive_fallback() {
        // Provider b costs 5; the route only allows 3 units total.
        let table = table(
            serde_json::json!([
                {"provider_id": "a", "advance_on": ["measurement_failed"]},
                {"provider_id": "b", "advance_on": []}
            ]),
            serde_json::json!({"max_attempts": 4, "max_prompt_bytes": 4096, "deadline_ticks": 3, "cost_units": 3}),
        );
        let mut router = CollectiveRouter::new(table, source());
        router.install(Box::new(Scripted::new(
            "a",
            InferenceOutcome::MeasurementFailed {
                reason: "down".into(),
            },
        )));
        router.install(Box::new(Scripted::new(
            "b",
            InferenceOutcome::Answered {
                completion: "too expensive to reach".into(),
            },
        )));
        let observation = router.dispatch(&request(), Tick(2));
        assert!(matches!(
            observation.outcome,
            InferenceOutcome::NotAttempted { .. }
        ));
        assert_eq!(observation.attribution.attempts.len(), 1);
        assert_eq!(observation.attribution.budget_charged, 1);
        assert_eq!(observation.attribution.budget_allowed, 3);
    }

    #[test]
    fn max_attempts_bounds_the_chain() {
        let table = table(
            serde_json::json!([
                {"provider_id": "a", "advance_on": ["measurement_failed"]},
                {"provider_id": "b", "advance_on": ["measurement_failed"]}
            ]),
            serde_json::json!({"max_attempts": 1, "max_prompt_bytes": 4096, "deadline_ticks": 3, "cost_units": 100}),
        );
        let mut router = CollectiveRouter::new(table, source());
        router.install(Box::new(Scripted::new(
            "a",
            InferenceOutcome::MeasurementFailed {
                reason: "down".into(),
            },
        )));
        router.install(Box::new(Scripted::new(
            "b",
            InferenceOutcome::MeasurementFailed {
                reason: "also down".into(),
            },
        )));
        let observation = router.dispatch(&request(), Tick(2));
        assert_eq!(observation.attribution.attempts.len(), 1);
        assert!(format!("{:?}", observation.outcome).contains("max_attempts"));
    }

    #[test]
    fn an_uninstalled_provider_is_reported_never_skipped() {
        let table = table(
            serde_json::json!([
                {"provider_id": "a", "advance_on": ["measurement_failed"]},
                {"provider_id": "b", "advance_on": []}
            ]),
            generous(),
        );
        let mut router = CollectiveRouter::new(table, source());
        router.install(Box::new(Scripted::new(
            "a",
            InferenceOutcome::MeasurementFailed {
                reason: "down".into(),
            },
        )));
        // `b` is deliberately not installed.
        assert_eq!(router.uninstalled_providers(), vec!["b".to_string()]);
        let observation = router.dispatch(&request(), Tick(2));
        assert!(format!("{:?}", observation.outcome).contains("no instance is installed"));
        assert_eq!(observation.attribution.attempts.len(), 1);
    }

    #[test]
    fn attribution_names_where_the_routing_came_from() {
        let table = table(
            serde_json::json!([{"provider_id": "a", "advance_on": []}]),
            generous(),
        );
        let mut router = CollectiveRouter::new(
            table,
            RoutingSource::Committed {
                store: "artifacts/copy/store".into(),
                revision: 42,
            },
        );
        router.install(Box::new(Scripted::new(
            "a",
            InferenceOutcome::Answered {
                completion: "ok".into(),
            },
        )));
        let observation = router.dispatch(&request(), Tick(2));
        assert_eq!(
            observation.attribution.routing_source,
            "committed store artifacts/copy/store @revision 42"
        );
    }
}
