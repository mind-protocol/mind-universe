//! `act` — request a real transformation of the Universe, *as a session*.
//!
//! **`act` returns what `sense` returns.** There is only one reality, so the
//! honest result of acting is the world *after*, perceived. `act` commits a real
//! mutation, the tick advances, and it returns the actor's POV of the new
//! revision — exactly the [`Observation`] shape `sense` produces — with the
//! committed delta and its independent-readback evidence folded into `changes`.
//!
//! What it commits today is a **real graph mutation**: the intent is written into
//! the one reality as a `construct` entity with `provenance: "built"` and an
//! authored `construction_moment`, committed atomically through the four-verb
//! write path and read back from a fresh store replay. This is a genuine
//! transformation (the revision advances, the nodes persist and survive an
//! independent reopen). It is WRITTEN, not RUNNING: it does not yet assemble and
//! fire the live mechanism. That realisation is the next step, and
//! `remaining_gap` says so.

use serde::Deserialize;
use serde_json::json;

use universe_supervisor::perception::{observe, observe_unmounted, Observation, SenseParams};

use crate::session::ActorSession;
use crate::world::World;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ActParams {
    /// The acting session's id (see `arrive`). Recorded as the builder of the
    /// committed proposal — traceable, never anonymous.
    #[serde(default)]
    pub actor_id: Option<String>,
    /// What must become true. May stay in natural language.
    pub intent: String,
    #[serde(default)]
    pub target: Option<String>,
    /// Vantage to observe the world-AFTER from (EntityKey hex or symbol name).
    /// The perturbation commits regardless of this — `where` only situates the
    /// readback POV, exactly like `sense`'s `where`. Defaults to `target`, then
    /// to the actor's own position, when absent.
    #[serde(default)]
    pub r#where: Option<String>,
    #[serde(default)]
    pub constraints: Option<String>,
    #[serde(default)]
    pub proof: Option<String>,
    /// Optional path to an authored fixture (root + members + relations). When
    /// present, `act` injects that whole subgraph as one atomic transaction
    /// instead of recording a plain proposal.
    #[serde(default)]
    pub fixture: Option<String>,
}

/// What `act` committed this call.
enum Done {
    Empty,
    Proposal(Result<crate::world::ProposalOutcome, String>),
    Injection(Result<crate::world::InjectionOutcome, String>),
}

/// Acts as `session`: commits the intent as an inert proposal, then returns the
/// actor's POV of the resulting revision — the same [`Observation`] `sense`
/// returns, with the committed delta folded into `changes`.
pub fn act(world: &mut World, params: &ActParams, session: &ActorSession) -> Observation {
    // 1. Commit the mutation (mounted). A fixture injects a whole subgraph; else
    //    a non-empty intent records an inert proposal. The tick advances.
    let done = if let Some(fixture) = params.fixture.as_deref() {
        Done::Injection(world.inject_fixture(fixture, session))
    } else if params.intent.trim().is_empty() {
        Done::Empty
    } else {
        Done::Proposal(
            world
                .commit_proposal(&params.intent, params.target.as_deref(), session)
                .map_err(|error| error.to_string()),
        )
    };

    // 2. Observe the world AFTER the commit (the advanced revision).
    let sense_params = SenseParams {
        actor_id: params.actor_id.clone(),
        // Observe the world-after from the explicit vantage when given, else from
        // the proposal's target, else the actor. The commit above is unaffected.
        r#where: params.r#where.clone().or_else(|| params.target.clone()),
        focus: Some(params.intent.clone()),
        scale: None,
        since: None,
        radius_m: None,
    };
    let mut observation = match (world.snapshot(), world.runtime_inventory()) {
        (Some(snapshot), Some(inventory)) => observe(
            snapshot,
            &inventory,
            &sense_params,
            Some(session.passport()),
            &|c| world.read_content(c),
        ),
        _ => observe_unmounted(world.unmounted_reason().unwrap_or("no Universe mounted")),
    };

    attach_action(&mut observation, params, session, done);
    observation
}

/// Folds the committed action into the one reality — the world after IS the
/// receipt. `outcome` is `None` for an empty intent, `Ok` with the measured
/// delta + evidence, or `Err` with why nothing committed.
fn attach_action(
    observation: &mut Observation,
    params: &ActParams,
    session: &ActorSession,
    done: Done,
) {
    let interpreted = json!({
        "builder": session.session_id,
        "raw": params.intent,
        "target": params.target,
        "fixture": params.fixture,
        "constraints": params.constraints,
        "expected_proof": params.proof,
        "compiled": false,
    });

    let Some(changes) = observation.changes.as_object_mut() else {
        return;
    };
    changes.insert("acted".into(), json!(true));
    changes.insert("intent".into(), interpreted);

    match done {
        Done::Empty => {
            changes.insert("committed_effects".into(), json!([]));
            changes.insert("evidence".into(), json!([]));
            changes.insert("remaining_gap".into(), json!("intent must be non-empty"));
        }
        Done::Proposal(Ok(o)) => {
            changes.insert("committed_effects".into(), json!(o.committed_effects));
            changes.insert("evidence".into(), json!(o.evidence));
            changes.insert(
                "revision".into(),
                json!({ "from": o.from_revision, "to": o.to_revision }),
            );
            changes.insert("idempotent".into(), json!(o.idempotent));
            changes.insert(
                "remaining_gap".into(),
                json!("graph_status: WRITTEN. The intent was committed as a real `construct` node \
(revision advanced, read back independently from a fresh store replay) via the four-verb write path. \
It is written, not running: wiring and firing the live mechanism is a further step."),
            );
        }
        Done::Injection(Ok(o)) => {
            changes.insert("committed_effects".into(), json!(o.committed_effects));
            changes.insert("evidence".into(), json!(o.evidence));
            changes.insert(
                "revision".into(),
                json!({ "from": o.from_revision, "to": o.to_revision }),
            );
            changes.insert("idempotent".into(), json!(o.idempotent));
            changes.insert(
                "injection".into(),
                json!({
                    "fixture_id": o.fixture_id,
                    "nodes_injected": o.nodes_injected,
                    "relations_kept": o.relations_kept,
                    "relations_dropped": o.relations_dropped,
                    "interned_symbols": o.interned_symbols,
                }),
            );
            changes.insert(
                "remaining_gap".into(),
                json!("graph_status: WRITTEN. The fixture subgraph is committed and read back \
independently; wiring / runtime / health remain not_wired / not_running / not_measured — a written \
loop is not a running one."),
            );
        }
        Done::Proposal(Err(reason)) | Done::Injection(Err(reason)) => {
            changes.insert("committed_effects".into(), json!([]));
            changes.insert("evidence".into(), json!([]));
            changes.insert(
                "remaining_gap".into(),
                json!(format!("nothing committed: {reason}")),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{admit, AdmissionRequest};
    use universe_supervisor::perception::Uncertainty;

    fn unmounted() -> World {
        World::Unmounted {
            reason: "no store".into(),
        }
    }

    fn visitor() -> ActorSession {
        admit(&AdmissionRequest::default(), "v".into(), 0, 100).0
    }

    #[test]
    fn act_returns_a_sense_observation_shape() {
        let params = ActParams {
            intent: "connect two beacons".into(),
            ..Default::default()
        };
        let obs = act(&mut unmounted(), &params, &visitor());
        assert_eq!(obs.uncertainty, Uncertainty::Unknown);
        assert_eq!(obs.changes.as_object().unwrap()["acted"], json!(true));
    }

    #[test]
    fn act_on_an_unmounted_world_commits_nothing_and_says_why() {
        let params = ActParams {
            intent: "build a room here".into(),
            ..Default::default()
        };
        let obs = act(&mut unmounted(), &params, &visitor());
        let changes = obs.changes.as_object().unwrap();
        assert_eq!(changes["committed_effects"].as_array().unwrap().len(), 0);
        assert!(changes["remaining_gap"]
            .as_str()
            .unwrap()
            .contains("nothing committed"));
    }

    #[test]
    fn empty_intent_commits_nothing() {
        let obs = act(&mut unmounted(), &ActParams::default(), &visitor());
        let changes = obs.changes.as_object().unwrap();
        assert_eq!(
            changes["remaining_gap"].as_str(),
            Some("intent must be non-empty")
        );
    }
}
