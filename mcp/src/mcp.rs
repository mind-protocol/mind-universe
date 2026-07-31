//! MCP method handlers: `initialize`, `tools/list`, `tools/call`.
//!
//! The adapter exposes exactly two tools — `sense` and `act` — the minimal
//! perceive/transform pair. Both are headless surfaces onto the same Universe
//! semantics as the embodied 3D world; they add no second ontology.

use serde_json::{json, Value};

use crate::act::{self, ActParams};
use crate::jsonrpc::{code, Request, Response};
use crate::sense::{self, SenseParams};
use crate::session::{AdmissionRequest, SessionRegistry};
use crate::world::World;

/// The MCP protocol revision this adapter speaks.
const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "mind-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Routes a decoded JSON-RPC request. Returns `None` for notifications (which
/// receive no reply). `registry` holds admitted sessions; `now` is wall-clock
/// unix seconds, injected so admission/expiry stay testable.
pub fn handle(
    world: &mut World,
    registry: &mut SessionRegistry,
    now: u64,
    request: &Request,
) -> Option<Response> {
    if request.is_notification() {
        // e.g. `notifications/initialized`; acknowledged by silence.
        return None;
    }
    let id = request.id.clone().unwrap_or(Value::Null);

    let result = match request.method.as_str() {
        "initialize" => Ok(initialize_result()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => tools_call(world, registry, now, &request.params),
        other => {
            return Some(Response::failure(
                id,
                code::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            ));
        }
    };

    Some(match result {
        Ok(value) => Response::success(id, value),
        Err(HandlerError { code, message }) => Response::failure(id, code, message),
    })
}

struct HandlerError {
    code: i64,
    message: String,
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        "instructions": "Two verbs into one Universe. `sense` perceives a bounded \
situation without mutating it. `act` requests a real transformation and returns \
a receipt — read the receipt, do not guess."
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "arrive",
                "description": "Arrive at Lumina Prime. The city Gate mints an ephemeral, traceable \
ActorSession (a TransientActor) at the Porte d'Arrivée with minimal safe capabilities. Being \
present is free; no missing authentication is ever read as a verified identity or a write \
authorization. Returns an admission receipt: SessionPassport, CapabilityEnvelope, ExpirationCondition.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": { "type": "string", "description": "A stable id for this session; used on later sense/act calls. Generated if omitted." },
                        "origin": { "type": "string", "description": "Declared origin, e.g. 'ChatGPT/OpenAI'. Traceable even when unauthenticated." },
                        "sponsor": { "type": "string", "description": "A sponsoring Citizen. A sponsor unlocks `propose` within a bounded scope. Optional." },
                        "requested_capabilities": { "type": "array", "items": { "type": "string" }, "description": "Capabilities requested; only observe/speak/propose are grantable to a visitor. Optional." },
                        "requested_scope": { "type": "array", "items": { "type": "string" }, "description": "Spaces the session asks to work in (granted only with a sponsor). Optional." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "sense",
                "description": "Perceive the Universe as a session, without changing its semantic \
state. An unsituated visitor perceives from the Porte d'Arrivée, facing Balise Zéro. Returns the \
POV (eye, look orientation) plus the text of sense (the visible spheres), objects, processes, \
changes, affordances, the session passport, an honest uncertainty tag, and — as an MCP `image` \
content block — a first-person JPEG frame of the physics-sphere (present whenever something is \
placed to see).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "actor_id": { "type": "string", "description": "The perceiving session id (from `arrive`). An unknown id is admitted as a traceable walk-in visitor." },
                        "where": { "type": "string", "description": "Place, Space, Actor or object to observe from (EntityKey hex or symbol name). Defaults to the actor. Optional." },
                        "focus": { "type": "string", "description": "What to understand. Optional." },
                        "scale": { "type": "string", "description": "Observation scale. Optional." },
                        "since": { "type": "string", "description": "A moment or receipt to observe changes since. Optional." }
                    },
                    "required": ["actor_id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "act",
                "description": "Request a real transformation of the Universe as a session. Commits \
the intent as an INERT proposal in the single reality (a `proposal` node with provenance:built and \
an authored construction_moment), advances the tick, and reads it back independently. Returns the \
SAME observation as `sense` — the actor's POV of the world AFTER — with the committed delta and its \
readback evidence (committed_effects, evidence, revision, remaining_gap) in `changes`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "actor_id": { "type": "string", "description": "The acting session id (from `arrive`); the transformation is admitted only against its capabilities." },
                        "intent": { "type": "string", "description": "What must become true (a human-readable label; also used as the proposal when no fixture is given)." },
                        "target": { "type": "string", "description": "Object or situation concerned. Optional." },
                        "constraints": { "type": "string", "description": "What must be preserved or avoided. Optional." },
                        "proof": { "type": "string", "description": "Expected observable result. Optional." },
                        "fixture": { "type": "string", "description": "Path to an authored fixture (root + members + relations) to inject as one atomic subgraph instead of a plain proposal. Optional." }
                    },
                    "required": ["actor_id", "intent"],
                    "additionalProperties": false
                }
            }
        ]
    })
}

fn tools_call(
    world: &mut World,
    registry: &mut SessionRegistry,
    now: u64,
    params: &Value,
) -> Result<Value, HandlerError> {
    let name = params.get("name").and_then(Value::as_str).ok_or(HandlerError {
        code: code::INVALID_PARAMS,
        message: "tools/call requires a string `name`".to_owned(),
    })?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    let mut structured = match name {
        "arrive" => {
            let request: AdmissionRequest =
                serde_json::from_value(arguments).map_err(invalid_params)?;
            let receipt = registry.admit(&request, now);
            serde_json::to_value(receipt).map_err(internal)?
        }
        "sense" => {
            let params: SenseParams = serde_json::from_value(arguments).map_err(invalid_params)?;
            // Every presence carries a session: resolve it, or mint a traceable
            // walk-in for an unknown id. The Gate never lets a presence exist
            // without a provenance.
            let session = params
                .actor_id
                .as_deref()
                .map(|id| registry.get_or_walk_in(id, now));
            let observation = match (world.snapshot(), world.runtime_inventory()) {
                (Some(snapshot), Some(inventory)) => {
                    sense::observe(snapshot, &inventory, &params, session.as_ref(), &|c| {
                        world.read_content(c)
                    })
                }
                _ => sense::observe_unmounted(
                    world.unmounted_reason().unwrap_or("no Universe mounted"),
                ),
            };
            serde_json::to_value(observation).map_err(internal)?
        }
        "act" => {
            let params: ActParams = serde_json::from_value(arguments).map_err(invalid_params)?;
            let session = registry
                .get_or_walk_in(params.actor_id.as_deref().unwrap_or("anonymous"), now);
            // `act` returns what `sense` returns: the actor's POV of the world
            // after the tick settles, with the action folded into `changes`.
            let observation = act::act(world, &params, &session);
            serde_json::to_value(observation).map_err(internal)?
        }
        other => {
            return Err(HandlerError {
                code: code::INVALID_PARAMS,
                message: format!("unknown tool: {other}"),
            });
        }
    };

    // Drop the SVG twin (`image`) — only the JPEG rides back. Its base64 lives in
    // `image_jpeg` and, for clients that render raster, in the MCP `image` block
    // below. Keeping both an SVG and a JPEG of the same frame just doubles the
    // payload for no gain.
    if let Some(obj) = structured.as_object_mut() {
        obj.remove("image");
    }

    // MCP tool results carry a human-readable `content` block plus the machine
    // `structuredContent`. The text is a pretty-printed mirror of the structure.
    let text = serde_json::to_string_pretty(&structured).map_err(internal)?;
    let mut content = vec![json!({ "type": "text", "text": text })];
    // Surface the actor's POV frame as a real MCP `image` block, not merely a
    // string buried in `structuredContent` — so a client can actually SEE what
    // the actor sees. `sense` and `act` both carry a precomputed base64 JPEG
    // (`image_jpeg`); `arrive` does not.
    if let Some(jpeg) = structured.get("image_jpeg").and_then(Value::as_str) {
        content.push(json!({
            "type": "image",
            "data": jpeg,
            "mimeType": "image/jpeg",
        }));
    }
    Ok(json!({
        "content": content,
        "structuredContent": structured,
        "isError": false
    }))
}

fn invalid_params(error: serde_json::Error) -> HandlerError {
    HandlerError {
        code: code::INVALID_PARAMS,
        message: format!("invalid arguments: {error}"),
    }
}

fn internal(error: serde_json::Error) -> HandlerError {
    HandlerError {
        code: code::INTERNAL_ERROR,
        message: format!("serialization failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, params: Value) -> Request {
        serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        }))
        .unwrap()
    }

    fn dispatch(world: &mut World, req: &Request) -> Option<Response> {
        handle(world, &mut SessionRegistry::default(), 0, req)
    }

    #[test]
    fn initialize_advertises_the_tools_capability() {
        let mut world = World::Unmounted { reason: "test".into() };
        let response = dispatch(&mut world, &request("initialize", json!({}))).unwrap();
        let result = response.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_exposes_arrive_sense_and_act() {
        let mut world = World::Unmounted { reason: "test".into() };
        let response = dispatch(&mut world, &request("tools/list", json!({}))).unwrap();
        let tools = response.result.unwrap()["tools"].clone();
        let names: Vec<String> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            vec!["arrive".to_owned(), "sense".to_owned(), "act".to_owned()]
        );
    }

    #[test]
    fn arrive_mints_a_traceable_session_passport() {
        let mut world = World::Unmounted { reason: "test".into() };
        let response = dispatch(
            &mut world,
            &request(
                "tools/call",
                json!({ "name": "arrive", "arguments": { "origin": "ChatGPT/OpenAI" } }),
            ),
        )
        .unwrap();
        let structured = &response.result.unwrap()["structuredContent"];
        assert_eq!(structured["admitted"], json!(true));
        assert_eq!(structured["passport"]["origin"], "ChatGPT/OpenAI");
        assert_eq!(structured["passport"]["status"], "unauthenticated_visitor");
        assert_eq!(structured["arrival_position"], json!([0.0, -500.0, 0.0]));
    }

    #[test]
    fn sense_on_an_unmounted_world_is_unknown() {
        let mut world = World::Unmounted { reason: "no store".into() };
        let response = dispatch(
            &mut world,
            &request(
                "tools/call",
                json!({ "name": "sense", "arguments": { "actor_id": "v1" } }),
            ),
        )
        .unwrap();
        let structured = &response.result.unwrap()["structuredContent"];
        assert_eq!(structured["uncertainty"], "unknown");
    }

    #[test]
    fn act_returns_a_sense_observation_with_the_action_in_changes() {
        let mut world = World::Unmounted { reason: "no store".into() };
        let response = dispatch(
            &mut world,
            &request(
                "tools/call",
                json!({ "name": "act", "arguments": { "actor_id": "v1", "intent": "connect two beacons" } }),
            ),
        )
        .unwrap();
        let structured = &response.result.unwrap()["structuredContent"];
        // It IS a sense observation (has uncertainty/objects), no status verdict.
        assert!(structured.get("status").is_none());
        assert!(structured.get("uncertainty").is_some());
        assert!(structured.get("objects").is_some());
        let changes = &structured["changes"];
        assert_eq!(changes["acted"], json!(true));
        // Unmounted: nothing committed, and the receipt says why.
        assert_eq!(changes["committed_effects"].as_array().unwrap().len(), 0);
        assert!(changes["remaining_gap"]
            .as_str()
            .unwrap()
            .contains("nothing committed"));
    }

    #[test]
    fn a_notification_receives_no_reply() {
        let mut world = World::Unmounted { reason: "test".into() };
        let note: Request = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .unwrap();
        assert!(dispatch(&mut world, &note).is_none());
    }

    #[test]
    fn an_unknown_method_is_method_not_found() {
        let mut world = World::Unmounted { reason: "test".into() };
        let response = dispatch(&mut world, &request("nonsense/method", json!({}))).unwrap();
        assert_eq!(response.error.unwrap().code, code::METHOD_NOT_FOUND);
    }
}
