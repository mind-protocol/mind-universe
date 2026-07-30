//! Boots a Universe from a store + genesis and SERVES its bounded situation over
//! the authenticated loopback protocol (`universe-protocol`), so Mind Desktop can
//! connect to a real snapshot/delta/event stream instead of a fixture.
//!
//! The transport is loopback-only and authenticated by a shared secret the
//! operator supplies via `UNIVERSE_STREAM_SECRET` — this binary never fabricates
//! a secret and never runs the socket unauthenticated.

use std::{env, net::SocketAddr, path::PathBuf, thread, time::Duration};
use universe_core::EntityKey;
use universe_protocol::{
    AuthenticationSecret, CorrelationId, HeartbeatMessage, OperationState, ProtocolTransportConfig,
    ProtocolTransportError, QueryCompletion, RunningProtocolServer, ServerPayload,
    SituationSnapshotMessage, StreamBudget,
};
use universe_store::UniverseSnapshot;
use universe_supervisor::Supervisor;

const STREAM_ID: &str = "universe-desktop";
const DEFAULT_ADDRESS: &str = "127.0.0.1:0";

fn main() {
    let mut args = env::args_os().skip(1);
    let store = args.next().map(PathBuf::from);
    let genesis = args.next().map(PathBuf::from);
    let (store, genesis) = match (store, genesis) {
        (Some(store), Some(genesis)) => (store, genesis),
        _ => {
            eprintln!("usage: universe-server <store-directory> <genesis-json>");
            eprintln!("  env UNIVERSE_STREAM_SECRET (required)  shared auth secret");
            eprintln!("  env UNIVERSE_STREAM_ADDR   (optional)  loopback bind addr [{DEFAULT_ADDRESS}]");
            std::process::exit(2);
        }
    };

    let supervisor = match Supervisor::boot(store, genesis) {
        Ok(supervisor) => supervisor,
        Err(error) => {
            eprintln!("blocked: {error}");
            std::process::exit(1);
        }
    };

    let secret = match env::var("UNIVERSE_STREAM_SECRET") {
        Ok(value) if !value.is_empty() => match AuthenticationSecret::new(value.as_bytes()) {
            Ok(secret) => secret,
            Err(error) => {
                eprintln!("blocked: invalid UNIVERSE_STREAM_SECRET: {error}");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("blocked: UNIVERSE_STREAM_SECRET must be set to serve the stream");
            std::process::exit(2);
        }
    };

    let address: SocketAddr = env::var("UNIVERSE_STREAM_ADDR")
        .unwrap_or_else(|_| DEFAULT_ADDRESS.to_owned())
        .parse()
        .unwrap_or_else(|_| {
            eprintln!("blocked: UNIVERSE_STREAM_ADDR is not a valid socket address");
            std::process::exit(2);
        });

    let server = match serve(supervisor.snapshot(), address, secret, STREAM_ID) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("blocked: {error}");
            std::process::exit(1);
        }
    };

    println!(
        "serving stream={STREAM_ID} on {} universe_revision={} tick={} entities={} relations={}",
        server.local_addr(),
        supervisor.revision().0,
        supervisor.tick().0,
        supervisor.snapshot().entities.len(),
        supervisor.snapshot().relations.len(),
    );

    // Keep the process (and the accept thread) alive.
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

/// Projects the Universe snapshot into a bounded situation message for the wire.
fn situation_snapshot(snapshot: &UniverseSnapshot) -> SituationSnapshotMessage {
    let origin = snapshot
        .entities
        .first()
        .map(|entity| entity.key)
        .unwrap_or(EntityKey(0));
    SituationSnapshotMessage {
        universe: snapshot.universe,
        revision: snapshot.revision,
        tick: snapshot.tick,
        origin,
        completion: QueryCompletion::Complete,
        entities: snapshot.entities.clone(),
        relations: snapshot.relations.clone(),
    }
}

fn stream_budget() -> StreamBudget {
    StreamBudget {
        max_pending_frames: 4096,
        max_pending_bytes: 64 * 1024 * 1024,
        max_frame_bytes: 16 * 1024 * 1024,
    }
}

fn transport_config() -> ProtocolTransportConfig {
    ProtocolTransportConfig {
        wire_max_frame_bytes: 16 * 1024 * 1024,
        max_connections: 32,
        io_timeout: Duration::from_secs(10),
        heartbeat_interval: Duration::from_secs(2),
    }
}

/// Binds the authenticated loopback server, seeds the recovery snapshot, and
/// publishes the boot situation so a connecting client receives the current
/// bounded world immediately.
fn serve(
    snapshot: &UniverseSnapshot,
    address: SocketAddr,
    secret: AuthenticationSecret,
    stream_id: &str,
) -> Result<RunningProtocolServer, ProtocolTransportError> {
    let situation = situation_snapshot(snapshot);
    let heartbeat = HeartbeatMessage {
        revision: snapshot.revision,
        tick: snapshot.tick,
        readiness: OperationState::Committed,
    };
    let server = RunningProtocolServer::bind_loopback(
        address,
        secret,
        stream_id.to_owned(),
        stream_budget(),
        transport_config(),
        heartbeat,
        Some(situation.clone()),
    )?;
    server.publish(
        CorrelationId("boot-snapshot".to_owned()),
        ServerPayload::Snapshot(situation),
    )?;
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_protocol::{
        ProtocolClient, ProtocolConnection, ProtocolHello, ResumeResult, ResynchronizeRequest,
        ServerFrame,
    };
    use universe_store::{GraphSeed, SeedEntity, UniverseStore};

    fn read_until_snapshot(connection: &mut ProtocolConnection) -> ServerFrame {
        for _ in 0..16 {
            match connection.read_next() {
                Ok(frame) => {
                    if matches!(&frame.payload, ServerPayload::Snapshot(_)) {
                        return frame;
                    }
                }
                Err(error) => panic!("read failed before snapshot: {error}"),
            }
        }
        panic!("client never received a snapshot frame");
    }

    fn seeded_snapshot() -> (tempfile::TempDir, UniverseSnapshot) {
        let temp = tempfile::tempdir().unwrap();
        let seed = GraphSeed {
            universe: universe_core::UniverseId(0xC0DE),
            symbols: vec!["thing".to_owned()],
            entities: vec![SeedEntity {
                key: EntityKey(0x01),
                generation: 0,
                symbol: "thing".to_owned(),
                content: serde_json::json!({ "kind": "demo", "name": "Root" }),
            }],
            relations: vec![],
        };
        let store = UniverseStore::open(temp.path()).unwrap();
        let snapshot = store.install_seed(&seed).unwrap();
        (temp, snapshot)
    }

    #[test]
    fn situation_snapshot_carries_the_stores_entities() {
        let (_temp, snapshot) = seeded_snapshot();
        let situation = situation_snapshot(&snapshot);
        assert_eq!(situation.entities.len(), 1);
        assert_eq!(situation.entities[0].key, EntityKey(0x01));
        assert_eq!(situation.universe, snapshot.universe);
    }

    #[test]
    fn an_authenticated_client_receives_the_boot_snapshot() {
        let (_temp, snapshot) = seeded_snapshot();
        let secret_bytes = b"a-sufficiently-long-shared-secret-value";
        let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = serve(
            &snapshot,
            address,
            AuthenticationSecret::new(secret_bytes).unwrap(),
            "test-stream",
        )
        .unwrap();

        let mut connection = ProtocolClient::connect(
            server.local_addr(),
            &AuthenticationSecret::new(secret_bytes).unwrap(),
            ProtocolHello {
                minimum_version: 0,
                maximum_version: 0,
                client_id: "test-client".to_owned(),
                resume_after: None,
            },
            transport_config(),
        )
        .unwrap();

        // Collect frames from the recovery bundle and the live stream: the boot
        // situation snapshot the server published must reach the client.
        let mut frames = match connection.recovery() {
            ResumeResult::Frames { frames, .. } => frames.clone(),
            _ => Vec::new(),
        };
        let is_snapshot = |frame: &universe_protocol::ServerFrame| {
            matches!(
                &frame.payload,
                ServerPayload::Snapshot(situation) if situation.entities.len() == 1
            )
        };
        while !frames.iter().any(is_snapshot) {
            match connection.read_next() {
                Ok(frame) => frames.push(frame),
                Err(_) => break,
            }
        }
        assert!(
            frames.iter().any(is_snapshot),
            "client never received the boot snapshot"
        );
    }

    #[test]
    fn client_acknowledges_and_receives_further_frames() {
        let (_temp, snapshot) = seeded_snapshot();
        let secret_bytes = b"a-sufficiently-long-shared-secret-value";
        let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = serve(
            &snapshot,
            address,
            AuthenticationSecret::new(secret_bytes).unwrap(),
            "test-stream",
        )
        .unwrap();

        let mut connection = ProtocolClient::connect(
            server.local_addr(),
            &AuthenticationSecret::new(secret_bytes).unwrap(),
            ProtocolHello {
                minimum_version: 0,
                maximum_version: 0,
                client_id: "test-client".to_owned(),
                resume_after: None,
            },
            transport_config(),
        )
        .unwrap();

        // Receive the boot snapshot, then acknowledge through its sequence — the
        // server confirms the acknowledgement (full client→server round-trip).
        let boot = read_until_snapshot(&mut connection);
        connection
            .acknowledge(boot.sequence, CorrelationId("ack-1".to_owned()))
            .unwrap();

        // A frame published AFTER the client connected is delivered live.
        server
            .publish(
                CorrelationId("tick-1".to_owned()),
                ServerPayload::Heartbeat(HeartbeatMessage {
                    revision: snapshot.revision,
                    tick: snapshot.tick,
                    readiness: OperationState::Measured,
                }),
            )
            .unwrap();
        let next = connection.read_next().unwrap();
        assert!(matches!(&next.payload, ServerPayload::Heartbeat(_)));

        // Requesting a resync is accepted by the server.
        connection
            .request_resynchronization(
                ResynchronizeRequest {
                    origin: EntityKey(0x01),
                    max_entities: 100,
                    max_relations: 100,
                    timeout_ticks: 32,
                },
                CorrelationId("resync-1".to_owned()),
            )
            .unwrap();
    }

    #[test]
    fn a_wrong_secret_is_refused() {
        let (_temp, snapshot) = seeded_snapshot();
        let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = serve(
            &snapshot,
            address,
            AuthenticationSecret::new(b"the-real-shared-secret-value-here").unwrap(),
            "test-stream",
        )
        .unwrap();

        let result = ProtocolClient::connect(
            server.local_addr(),
            &AuthenticationSecret::new(b"a-different-wrong-secret-value-x").unwrap(),
            ProtocolHello {
                minimum_version: 0,
                maximum_version: 0,
                client_id: "test-client".to_owned(),
                resume_after: None,
            },
            transport_config(),
        );
        assert!(result.is_err(), "a wrong secret must be refused");
    }
}
