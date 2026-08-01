//! MCP method handlers: `initialize`, `tools/list`, `tools/call`.
//!
//! The adapter exposes `arrive` and `sense`. There is no transform verb:
//! changing the world is not a transport's to offer, and the adapter decides
//! nothing about what a caller learns. It frames, encodes, and routes — the
//! observation it serialises, image included, is composed by perception.

use serde_json::{json, Value};

use universe_supervisor::perception::{observe, observe_unmounted, SenseParams};

use crate::jsonrpc::{code, Request, Response};
use crate::session::{AdmissionRequest, Capability, SessionRegistry};
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
        "instructions": "A window onto one Universe. `arrive` announces you; \
`sense` perceives a bounded situation without changing it. There is no transform \
verb — nothing you say here can steer a change to the world. The one write is \
`arrive`'s own: a SPONSORED session is embodied as a durable inhabitant, reported \
in the receipt's `embodiment` block."
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
                        "session_id": { "type": "string", "description": "A stable id for this session; used on later `sense` calls. Generated if omitted." },
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
state. Omit `actor_id` to perceive through an arbitrary L1 inhabitant — a random situated actor — \
so you see from WITHIN the city rather than floating outside; when the world holds no L1 actor you \
observe from the outside instead (the resolution is reported honestly in `situation.actor_resolution` \
and `situation.embodied_actor`). Returns the POV (eye, look orientation) plus the text of sense (the \
visible spheres), objects, processes, changes, affordances, the session passport, an honest \
uncertainty tag, and — as an MCP `image` content block — a first-person JPEG frame of the \
physics-sphere (present whenever something is placed to see).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "actor_id": { "type": "string", "description": "The perceiving session id (from `arrive`). An unknown id is admitted as a traceable walk-in visitor. Omit it to embody a random L1 inhabitant." },
                        "where": { "type": "string", "description": "Place, Space, Actor or object to observe from (EntityKey hex or symbol name). Defaults to the actor. Optional." },
                        "focus": { "type": "string", "description": "What to understand. Optional." },
                        "scale": { "type": "string", "description": "Observation scale. Optional." },
                        "since": { "type": "string", "description": "A moment or receipt to observe changes since. Optional." }
                    },
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
            // Does this presence already HAVE a body here? A returning session is
            // recognised from the durable node it built, so its standing is
            // restored from committed evidence instead of dying with the process
            // that first minted it. A first arrival recalls nothing and is
            // admitted exactly as before.
            let recalled = request
                .session_id
                .as_deref()
                .and_then(|id| world.recall_body(id));
            let receipt = registry.admit(&request, now, recalled.as_ref());
            let mut structured = serde_json::to_value(&receipt).map_err(internal)?;
            // A SPONSORED visitor (one holding the `Propose` write capability) is
            // not merely admitted — it is EMBODIED: a durable, idempotent `actor`
            // node is written into the one reality so the presence persists as an
            // L1 inhabitant (and appears on the live desktop). A walk-in without
            // `Propose` is admitted only, and nothing is written — today's
            // fail-closed behavior, unchanged.
            if let Some(session_id) = receipt.passport.get("session_id").and_then(Value::as_str) {
                let session = registry.get_or_walk_in(session_id, now, recalled.as_ref());
                if session.has(Capability::Propose) {
                    let embodiment = match world.materialize_actor(&session) {
                        Ok(outcome) => outcome.to_json(),
                        Err(error) => json!({
                            "materialized": false,
                            "error": error.to_string(),
                        }),
                    };
                    if let Some(object) = structured.as_object_mut() {
                        object.insert("embodiment".to_owned(), embodiment);
                    }
                }
            }
            structured
        }
        "sense" => {
            let params: SenseParams = serde_json::from_value(arguments).map_err(invalid_params)?;
            // Every presence carries a session: resolve it, or mint a traceable
            // walk-in for an unknown id. The Gate never lets a presence exist
            // without a provenance.
            // Same recognition on the perceive path: `sense` is where a returning
            // session is normally met (the prompt hook senses far more often than
            // it arrives), so this is the call that would otherwise keep reporting
            // an inhabitant of this city as an anonymous stranger.
            let session = params.actor_id.as_deref().map(|id| {
                let recalled = world.recall_body(id);
                registry.get_or_walk_in(id, now, recalled.as_ref())
            });
            let observation = match (world.snapshot(), world.runtime_inventory()) {
                (Some(snapshot), Some(inventory)) => observe(
                    snapshot,
                    &inventory,
                    &params,
                    session.as_ref().map(|s| s.passport()),
                    &|c| world.read_content(c),
                ),
                _ => observe_unmounted(world.unmounted_reason().unwrap_or("no Universe mounted")),
            };
            // Serialise what perception handed over — nothing more. The
            // observation composes its OWN frame (`Observation::with_frame`), so
            // `image_jpeg` is already on the value. The adapter used to re-render
            // it here; that made the transport a second, unattributable observer,
            // deciding whether there was anything to see and captioning the frame
            // with the revision it was taken at (CLAUDE.md, "MCP is a pipe").
            serde_json::to_value(&observation).map_err(internal)?
        }
        other => {
            return Err(HandlerError {
                code: code::INVALID_PARAMS,
                message: format!("unknown tool: {other}"),
            });
        }
    };

    // MCP tool results carry a human-readable `content` block plus the machine
    // `structuredContent`. The text is a pretty-printed mirror of the structure.
    let mut text = serde_json::to_string_pretty(&structured).map_err(internal)?;
    // Cap the human-readable text so a result does not flood the caller's
    // context. Uniform across tools: the assembly layer carries no per-tool
    // logic. The machine `structuredContent` below is left whole. Truncate on a
    // char boundary.
    const TEXT_MAX_CHARS: usize = 5000;
    if text.chars().count() > TEXT_MAX_CHARS {
        let cut = text
            .char_indices()
            .nth(TEXT_MAX_CHARS)
            .map_or(text.len(), |(i, _)| i);
        text.truncate(cut);
        text.push_str("\n… [truncated: text capped at 5000 chars]");
    }
    let mut content = vec![json!({ "type": "text", "text": text })];
    // Surface the actor's POV frame as a real MCP `image` block, not a base64
    // string buried in `structuredContent` — so a client can actually SEE what
    // the actor sees. The frame is MOVED, not copied: carrying the same ~19 KB
    // of base64 twice in one result costs every caller a second read for no
    // additional information. A `null` stays in place — "no frame" is a real
    // signal, and a reader must still be able to tell it from "no such field".
    let frame = match structured.get("image_jpeg") {
        Some(Value::String(_)) => {
            structured.as_object_mut().and_then(|obj| obj.remove("image_jpeg"))
        }
        _ => None,
    };
    if let Some(Value::String(jpeg)) = frame {
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
    fn tools_list_exposes_arrive_and_sense_only() {
        let mut world = World::Unmounted { reason: "test".into() };
        let response = dispatch(&mut world, &request("tools/list", json!({}))).unwrap();
        let tools = response.result.unwrap()["tools"].clone();
        let names: Vec<String> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        // No write verb. Changing the world is not the pipe's to offer.
        assert_eq!(names, vec!["arrive".to_owned(), "sense".to_owned()]);
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
    fn sense_does_not_require_an_actor_id() {
        // `sense` accepts a call with no actor: none is required by the schema, so
        // a caller can perceive through a random L1 inhabitant.
        let mut world = World::Unmounted { reason: "test".into() };
        let response = dispatch(&mut world, &request("tools/list", json!({}))).unwrap();
        let tools = response.result.unwrap()["tools"].clone();
        let sense = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "sense")
            .unwrap()
            .clone();
        let required = sense["inputSchema"].get("required");
        assert!(
            required.is_none()
                || !required
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v == "actor_id"),
            "sense must not force actor_id"
        );
    }

    #[test]
    fn sense_with_no_actor_is_accepted() {
        // The full dispatch path accepts empty arguments (no actor_id) end-to-end.
        let mut world = World::Unmounted { reason: "no store".into() };
        let response = dispatch(
            &mut world,
            &request("tools/call", json!({ "name": "sense", "arguments": {} })),
        )
        .unwrap();
        let structured = &response.result.unwrap()["structuredContent"];
        assert_eq!(structured["uncertainty"], "unknown");
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
    fn the_frame_is_moved_out_of_structured_content_never_copied() {
        let mut world = World::Unmounted { reason: "no store".into() };
        let result = dispatch(
            &mut world,
            &request("tools/call", json!({ "name": "sense", "arguments": {} })),
        )
        .unwrap()
        .result
        .unwrap();
        // The base64 never rides in `structuredContent`; the MCP `image` block is
        // its single carrier.
        assert!(!result["structuredContent"]["image_jpeg"].is_string());
        // Unmounted: there is no frame at all, so the honest `null` stays put —
        // "no frame" must stay distinguishable from "no such field".
        assert_eq!(result["structuredContent"]["image_jpeg"], Value::Null);
        let content = result["content"].as_array().unwrap();
        assert!(content.iter().all(|c| c["type"] != "image"), "no frame, no image block");
    }

    #[test]
    fn there_is_no_write_verb() {
        // The adapter is a pipe. A caller asking it to change the world is told
        // plainly that it cannot, rather than being handed a transaction.
        let mut world = World::Unmounted { reason: "no store".into() };
        let response = dispatch(
            &mut world,
            &request(
                "tools/call",
                json!({ "name": "act", "arguments": { "actor_id": "v1", "intent": "connect two beacons" } }),
            ),
        )
        .unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());
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
