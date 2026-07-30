//! The native side of the Mind Desktop transport bridge (G3 item 1).
//!
//! A webview cannot open a raw TCP socket, so the app connects to the
//! authenticated loopback protocol through NATIVE code: this crate connects a
//! [`ProtocolClient`], drains its frames, and hands each one to a sink callback.
//! The Tauri command layer supplies a sink that forwards frames to the webview;
//! tests supply a sink that collects them. Keeping the drain loop here — free of
//! any UI dependency — makes it directly testable against a real server.

use std::net::SocketAddr;
use universe_protocol::{
    AuthenticationSecret, ProtocolClient, ProtocolHello, ProtocolReadEvent, ProtocolTransportConfig,
    ProtocolTransportError, ResumeResult, ServerFrame,
};

/// What the sink asks the drain loop to do after each frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameFlow {
    Continue,
    Stop,
}

/// How the drain loop ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BridgeOutcome {
    pub delivered: usize,
    /// The peer closed the connection.
    pub peer_closed: bool,
    /// The server asked for a full resync (its buffer no longer covers us).
    pub resynchronization_required: bool,
    /// The sink asked to stop.
    pub stopped_by_sink: bool,
}

/// Connects, delivers any recovery frames, then streams live frames to `on_frame`
/// until the sink stops, the peer closes, or the server requires a resync. The
/// connection handshake (including HMAC authentication) is performed by
/// [`ProtocolClient::connect`]; a wrong secret surfaces here as an error.
pub fn run_stream_bridge(
    address: SocketAddr,
    secret: &AuthenticationSecret,
    hello: ProtocolHello,
    config: ProtocolTransportConfig,
    mut on_frame: impl FnMut(ServerFrame) -> FrameFlow,
) -> Result<BridgeOutcome, ProtocolTransportError> {
    let mut connection = ProtocolClient::connect(address, secret, hello, config)?;
    let mut outcome = BridgeOutcome {
        delivered: 0,
        peer_closed: false,
        resynchronization_required: false,
        stopped_by_sink: false,
    };

    // The bounded situation delivered with the handshake, if any.
    if let ResumeResult::Frames { frames, .. } = connection.recovery() {
        for frame in frames.clone() {
            outcome.delivered += 1;
            if on_frame(frame) == FrameFlow::Stop {
                outcome.stopped_by_sink = true;
                return Ok(outcome);
            }
        }
    }

    loop {
        match connection.read_event() {
            Ok(ProtocolReadEvent::Frame(frame)) => {
                outcome.delivered += 1;
                if on_frame(frame) == FrameFlow::Stop {
                    outcome.stopped_by_sink = true;
                    return Ok(outcome);
                }
            }
            Ok(ProtocolReadEvent::SnapshotRequired { .. }) => {
                outcome.resynchronization_required = true;
                return Ok(outcome);
            }
            Err(ProtocolTransportError::PeerClosed) => {
                outcome.peer_closed = true;
                return Ok(outcome);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use universe_core::{EntityKey, Revision, Tick, UniverseId};
    use universe_protocol::{
        CorrelationId, HeartbeatMessage, OperationState, QueryCompletion, RunningProtocolServer,
        ServerPayload, SituationSnapshotMessage, StreamBudget,
    };

    const SECRET: &[u8] = b"a-sufficiently-long-shared-secret-value";

    fn config() -> ProtocolTransportConfig {
        ProtocolTransportConfig {
            wire_max_frame_bytes: 16 * 1024 * 1024,
            max_connections: 4,
            io_timeout: Duration::from_secs(5),
            heartbeat_interval: Duration::from_secs(2),
        }
    }

    fn hello() -> ProtocolHello {
        ProtocolHello {
            minimum_version: 0,
            maximum_version: 0,
            client_id: "bridge-test".to_owned(),
            resume_after: None,
        }
    }

    fn empty_situation() -> SituationSnapshotMessage {
        SituationSnapshotMessage {
            universe: UniverseId(0x1),
            revision: Revision(0),
            tick: Tick(0),
            origin: EntityKey(0),
            completion: QueryCompletion::Complete,
            entities: Vec::new(),
            relations: Vec::new(),
        }
    }

    fn server() -> RunningProtocolServer {
        RunningProtocolServer::bind_ephemeral_loopback(
            AuthenticationSecret::new(SECRET).unwrap(),
            "bridge-test-stream",
            StreamBudget {
                max_pending_frames: 1024,
                max_pending_bytes: 8 * 1024 * 1024,
                max_frame_bytes: 4 * 1024 * 1024,
            },
            config(),
            HeartbeatMessage {
                revision: Revision(0),
                tick: Tick(0),
                readiness: OperationState::Committed,
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn drains_published_frames_to_the_sink() {
        let server = server();
        server
            .publish(
                CorrelationId("boot".to_owned()),
                ServerPayload::Snapshot(empty_situation()),
            )
            .unwrap();

        let mut received: Vec<ServerFrame> = Vec::new();
        let outcome = run_stream_bridge(
            server.local_addr(),
            &AuthenticationSecret::new(SECRET).unwrap(),
            hello(),
            config(),
            |frame| {
                let is_snapshot = matches!(&frame.payload, ServerPayload::Snapshot(_));
                received.push(frame);
                if is_snapshot {
                    FrameFlow::Stop
                } else {
                    FrameFlow::Continue
                }
            },
        )
        .unwrap();

        assert!(outcome.stopped_by_sink);
        assert!(received
            .iter()
            .any(|frame| matches!(&frame.payload, ServerPayload::Snapshot(_))));
    }

    #[test]
    fn a_wrong_secret_fails_to_connect() {
        let server = server();
        let result = run_stream_bridge(
            server.local_addr(),
            &AuthenticationSecret::new(b"a-different-and-wrong-secret-value").unwrap(),
            hello(),
            config(),
            |_frame| FrameFlow::Continue,
        );
        assert!(result.is_err(), "a wrong secret must fail the handshake");
    }
}
