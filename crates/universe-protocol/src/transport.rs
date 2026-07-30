//! Bounded authenticated loopback transport for the versioned protocol.
//!
//! The wire is length-prefixed JSON. Authentication uses a fresh random
//! challenge and HMAC-SHA-256; the configured secret itself is neither
//! serializable nor printable.

use crate::{
    AcknowledgeMessage, ClientFrame, ClientPayload, CorrelationId, HeartbeatMessage, ProtocolHello,
    ProtocolStream, ProtocolStreamError, ProtocolWelcome, ResumeResult, ResynchronizeRequest,
    ServerFrame, ServerPayload, SituationSnapshotMessage, StreamBudget, StreamSequence,
    PROTOCOL_VERSION,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const AUTH_DOMAIN: &[u8] = b"mind-universe-protocol-v0-auth";
const AUTH_CHALLENGE_BYTES: usize = 32;
const AUTH_PROOF_BYTES: usize = 32;

/// Bootstrap-only transport credential.
///
/// It is intentionally not serializable and its debug representation never
/// exposes credential bytes.
#[derive(Clone)]
pub struct AuthenticationSecret(Arc<[u8]>);

impl AuthenticationSecret {
    pub fn new(secret: impl AsRef<[u8]>) -> Result<Self, ProtocolTransportError> {
        let secret = secret.as_ref();
        if secret.len() < 32 {
            return Err(ProtocolTransportError::InvalidSecret);
        }
        Ok(Self(Arc::from(secret)))
    }

    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for AuthenticationSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthenticationSecret([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolTransportConfig {
    pub wire_max_frame_bytes: usize,
    pub max_connections: usize,
    pub io_timeout: Duration,
    pub heartbeat_interval: Duration,
}

impl ProtocolTransportConfig {
    fn validate(self) -> Result<Self, ProtocolTransportError> {
        if self.wire_max_frame_bytes == 0
            || self.wire_max_frame_bytes > u32::MAX as usize
            || self.max_connections == 0
            || self.io_timeout.is_zero()
            || self.heartbeat_interval.is_zero()
        {
            return Err(ProtocolTransportError::InvalidConfiguration);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransportErrorCode {
    AuthenticationFailed,
    InvalidHandshake,
    ProtocolRejected,
    UnsupportedRequest,
    SnapshotUnavailable,
    Backpressured,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct AuthChallenge {
    nonce: [u8; AUTH_CHALLENGE_BYTES],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct AuthProof {
    proof: [u8; AUTH_PROOF_BYTES],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport_type", rename_all = "snake_case")]
enum ClientTransportMessage {
    Hello(ProtocolHello),
    Authenticate(AuthProof),
    Frame(ClientFrame),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport_type", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "wire envelopes are serialized immediately; boxing would add an avoidable wire-path allocation"
)]
enum ServerTransportMessage {
    Challenge(AuthChallenge),
    Ready {
        welcome: ProtocolWelcome,
        recovery: ResumeResult,
    },
    Acknowledged {
        through: StreamSequence,
    },
    Frame(ServerFrame),
    SnapshotRequired {
        requested_after: StreamSequence,
        earliest_available: StreamSequence,
        latest_published: StreamSequence,
    },
    Error {
        code: TransportErrorCode,
    },
}

#[derive(Debug)]
pub enum ProtocolTransportError {
    InvalidSecret,
    InvalidConfiguration,
    LoopbackRequired,
    Io(io::Error),
    Serialization(String),
    FrameTooLarge { bytes: usize, limit: usize },
    PeerClosed,
    AuthenticationFailed,
    InvalidHandshake,
    ProtocolRejected,
    UnsupportedRequest,
    SnapshotUnavailable,
    Backpressured,
    Stream(ProtocolStreamError),
    ServerThreadPanicked,
}

impl fmt::Display for ProtocolTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSecret => formatter.write_str("invalid authentication secret"),
            Self::InvalidConfiguration => formatter.write_str("invalid transport configuration"),
            Self::LoopbackRequired => formatter.write_str("transport must bind to loopback"),
            Self::Io(error) => write!(formatter, "transport I/O failed: {error}"),
            Self::Serialization(error) => write!(formatter, "transport encoding failed: {error}"),
            Self::FrameTooLarge { bytes, limit } => {
                write!(formatter, "wire frame size {bytes} exceeds limit {limit}")
            }
            Self::PeerClosed => formatter.write_str("transport peer closed"),
            Self::AuthenticationFailed => formatter.write_str("authentication failed"),
            Self::InvalidHandshake => formatter.write_str("invalid transport handshake"),
            Self::ProtocolRejected => formatter.write_str("protocol negotiation rejected"),
            Self::UnsupportedRequest => formatter.write_str("transport request is unsupported"),
            Self::SnapshotUnavailable => formatter.write_str("recovery snapshot is unavailable"),
            Self::Backpressured => formatter.write_str("transport stream is backpressured"),
            Self::Stream(error) => {
                write!(formatter, "protocol stream rejected operation: {error:?}")
            }
            Self::ServerThreadPanicked => formatter.write_str("protocol server thread panicked"),
        }
    }
}

impl std::error::Error for ProtocolTransportError {}

impl From<io::Error> for ProtocolTransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ProtocolStreamError> for ProtocolTransportError {
    fn from(error: ProtocolStreamError) -> Self {
        match error {
            ProtocolStreamError::Backpressured { .. } => Self::Backpressured,
            other => Self::Stream(other),
        }
    }
}

struct SharedServer {
    stream: Mutex<ProtocolStream>,
    secret: AuthenticationSecret,
    heartbeat: Mutex<HeartbeatMessage>,
    recovery_snapshot: Mutex<Option<SituationSnapshotMessage>>,
    connection_failures: Mutex<VecDeque<String>>,
    active_connections: AtomicUsize,
    config: ProtocolTransportConfig,
}

/// A running bounded protocol server backed by a real loopback `TcpListener`.
pub struct RunningProtocolServer {
    address: SocketAddr,
    shared: Arc<SharedServer>,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl RunningProtocolServer {
    pub fn bind_loopback(
        address: SocketAddr,
        secret: AuthenticationSecret,
        stream_id: impl Into<String>,
        stream_budget: StreamBudget,
        transport_config: ProtocolTransportConfig,
        heartbeat: HeartbeatMessage,
        recovery_snapshot: Option<SituationSnapshotMessage>,
    ) -> Result<Self, ProtocolTransportError> {
        if !address.ip().is_loopback() {
            return Err(ProtocolTransportError::LoopbackRequired);
        }
        let config = transport_config.validate()?;
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let shared = Arc::new(SharedServer {
            stream: Mutex::new(ProtocolStream::new(stream_id, stream_budget)?),
            secret,
            heartbeat: Mutex::new(heartbeat),
            recovery_snapshot: Mutex::new(recovery_snapshot),
            connection_failures: Mutex::new(VecDeque::new()),
            active_connections: AtomicUsize::new(0),
            config,
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shared = Arc::clone(&shared);
        let thread_shutdown = Arc::clone(&shutdown);
        let accept_thread = thread::Builder::new()
            .name("universe-protocol-loopback".into())
            .spawn(move || accept_loop(listener, thread_shared, thread_shutdown))?;
        Ok(Self {
            address,
            shared,
            shutdown,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn bind_ephemeral_loopback(
        secret: AuthenticationSecret,
        stream_id: impl Into<String>,
        stream_budget: StreamBudget,
        transport_config: ProtocolTransportConfig,
        heartbeat: HeartbeatMessage,
        recovery_snapshot: Option<SituationSnapshotMessage>,
    ) -> Result<Self, ProtocolTransportError> {
        Self::bind_loopback(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            secret,
            stream_id,
            stream_budget,
            transport_config,
            heartbeat,
            recovery_snapshot,
        )
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    pub fn publish(
        &self,
        correlation: CorrelationId,
        payload: ServerPayload,
    ) -> Result<ServerFrame, ProtocolTransportError> {
        Ok(lock(&self.shared.stream)?.publish(correlation, payload)?)
    }

    pub fn set_heartbeat(&self, heartbeat: HeartbeatMessage) -> Result<(), ProtocolTransportError> {
        *lock(&self.shared.heartbeat)? = heartbeat;
        Ok(())
    }

    pub fn set_recovery_snapshot(
        &self,
        snapshot: SituationSnapshotMessage,
    ) -> Result<(), ProtocolTransportError> {
        *lock(&self.shared.recovery_snapshot)? = Some(snapshot);
        Ok(())
    }

    /// Bounded, redacted transport failures useful for readiness diagnostics.
    pub fn connection_failures(&self) -> Result<Vec<String>, ProtocolTransportError> {
        Ok(lock(&self.shared.connection_failures)?
            .iter()
            .cloned()
            .collect())
    }

    pub fn shutdown(mut self) -> Result<(), ProtocolTransportError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<(), ProtocolTransportError> {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.accept_thread.take() {
            thread
                .join()
                .map_err(|_| ProtocolTransportError::ServerThreadPanicked)?;
        }
        Ok(())
    }
}

impl Drop for RunningProtocolServer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// An authenticated client connection over a real loopback `TcpStream`.
#[derive(Debug)]
pub struct ProtocolConnection {
    stream: TcpStream,
    config: ProtocolTransportConfig,
    welcome: ProtocolWelcome,
    recovery: ResumeResult,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the public read API returns an owned frame and preserves the existing unboxed frame contract"
)]
pub enum ProtocolReadEvent {
    Frame(ServerFrame),
    SnapshotRequired {
        requested_after: StreamSequence,
        earliest_available: StreamSequence,
        latest_published: StreamSequence,
    },
}

impl ProtocolConnection {
    pub fn welcome(&self) -> &ProtocolWelcome {
        &self.welcome
    }

    pub fn recovery(&self) -> &ResumeResult {
        &self.recovery
    }

    pub fn acknowledge(
        &mut self,
        through: StreamSequence,
        correlation: CorrelationId,
    ) -> Result<(), ProtocolTransportError> {
        self.send_payload(
            correlation,
            ClientPayload::Acknowledge(AcknowledgeMessage { through }),
        )?;
        match read_message(&mut self.stream, self.config.wire_max_frame_bytes)? {
            ServerTransportMessage::Acknowledged {
                through: acknowledged,
            } if acknowledged == through => Ok(()),
            ServerTransportMessage::Error { code } => Err(code.into()),
            _ => Err(ProtocolTransportError::InvalidHandshake),
        }
    }

    pub fn request_resynchronization(
        &mut self,
        request: ResynchronizeRequest,
        correlation: CorrelationId,
    ) -> Result<(), ProtocolTransportError> {
        self.send_payload(correlation, ClientPayload::Resynchronize(request))
    }

    pub fn read_next(&mut self) -> Result<ServerFrame, ProtocolTransportError> {
        match self.read_event()? {
            ProtocolReadEvent::Frame(frame) => Ok(frame),
            ProtocolReadEvent::SnapshotRequired { .. } => {
                Err(ProtocolTransportError::SnapshotUnavailable)
            }
        }
    }

    pub fn read_event(&mut self) -> Result<ProtocolReadEvent, ProtocolTransportError> {
        match read_message(&mut self.stream, self.config.wire_max_frame_bytes)? {
            ServerTransportMessage::Frame(frame) => Ok(ProtocolReadEvent::Frame(frame)),
            ServerTransportMessage::SnapshotRequired {
                requested_after,
                earliest_available,
                latest_published,
            } => Ok(ProtocolReadEvent::SnapshotRequired {
                requested_after,
                earliest_available,
                latest_published,
            }),
            ServerTransportMessage::Error { code } => Err(code.into()),
            _ => Err(ProtocolTransportError::InvalidHandshake),
        }
    }

    fn send_payload(
        &mut self,
        correlation: CorrelationId,
        payload: ClientPayload,
    ) -> Result<(), ProtocolTransportError> {
        let frame = ClientFrame {
            protocol_version: PROTOCOL_VERSION,
            stream_id: self.welcome.stream_id.clone(),
            correlation,
            payload,
        };
        write_message(
            &mut self.stream,
            &ClientTransportMessage::Frame(frame),
            self.config.wire_max_frame_bytes,
        )
    }
}

pub struct ProtocolClient;

impl ProtocolClient {
    pub fn connect(
        address: SocketAddr,
        secret: &AuthenticationSecret,
        hello: ProtocolHello,
        config: ProtocolTransportConfig,
    ) -> Result<ProtocolConnection, ProtocolTransportError> {
        if !address.ip().is_loopback() {
            return Err(ProtocolTransportError::LoopbackRequired);
        }
        let config = config.validate()?;
        let mut stream = TcpStream::connect_timeout(&address, config.io_timeout)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(config.io_timeout))?;
        stream.set_write_timeout(Some(config.io_timeout))?;
        write_message(
            &mut stream,
            &ClientTransportMessage::Hello(hello.clone()),
            config.wire_max_frame_bytes,
        )?;
        let challenge = match read_message(&mut stream, config.wire_max_frame_bytes)? {
            ServerTransportMessage::Challenge(challenge) => challenge,
            ServerTransportMessage::Error { code } => return Err(code.into()),
            _ => return Err(ProtocolTransportError::InvalidHandshake),
        };
        let proof = authentication_proof(secret.bytes(), &challenge.nonce, &hello)?;
        write_message(
            &mut stream,
            &ClientTransportMessage::Authenticate(AuthProof { proof }),
            config.wire_max_frame_bytes,
        )?;
        let (welcome, recovery) = match read_message(&mut stream, config.wire_max_frame_bytes)? {
            ServerTransportMessage::Ready { welcome, recovery } => (welcome, recovery),
            ServerTransportMessage::Error { code } => return Err(code.into()),
            _ => return Err(ProtocolTransportError::InvalidHandshake),
        };
        Ok(ProtocolConnection {
            stream,
            config,
            welcome,
            recovery,
        })
    }
}

impl From<TransportErrorCode> for ProtocolTransportError {
    fn from(code: TransportErrorCode) -> Self {
        match code {
            TransportErrorCode::AuthenticationFailed => Self::AuthenticationFailed,
            TransportErrorCode::InvalidHandshake => Self::InvalidHandshake,
            TransportErrorCode::ProtocolRejected => Self::ProtocolRejected,
            TransportErrorCode::UnsupportedRequest => Self::UnsupportedRequest,
            TransportErrorCode::SnapshotUnavailable => Self::SnapshotUnavailable,
            TransportErrorCode::Backpressured => Self::Backpressured,
        }
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<SharedServer>, shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, address)) if address.ip().is_loopback() => {
                if shared
                    .active_connections
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                        (active < shared.config.max_connections).then_some(active + 1)
                    })
                    .is_err()
                {
                    let mut stream = stream;
                    let _ = stream.set_nonblocking(false);
                    send_error(
                        &mut stream,
                        TransportErrorCode::Backpressured,
                        shared.config.wire_max_frame_bytes,
                    );
                    continue;
                }
                let connection_shared = Arc::clone(&shared);
                let connection_shutdown = Arc::clone(&shutdown);
                if thread::Builder::new()
                    .name("universe-protocol-client".into())
                    .spawn(move || {
                        if let Err(error) =
                            serve_connection(stream, &connection_shared, &connection_shutdown)
                        {
                            if let Ok(mut failures) = connection_shared.connection_failures.lock() {
                                if failures.len() == 32 {
                                    failures.pop_front();
                                }
                                failures.push_back(error.to_string());
                            }
                        }
                        connection_shared
                            .active_connections
                            .fetch_sub(1, Ordering::AcqRel);
                    })
                    .is_err()
                {
                    shared.active_connections.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

fn serve_connection(
    mut socket: TcpStream,
    shared: &SharedServer,
    shutdown: &AtomicBool,
) -> Result<(), ProtocolTransportError> {
    socket.set_nonblocking(false)?;
    socket.set_nodelay(true)?;
    socket.set_read_timeout(Some(shared.config.io_timeout))?;
    socket.set_write_timeout(Some(shared.config.io_timeout))?;
    let hello = match read_message(&mut socket, shared.config.wire_max_frame_bytes)? {
        ClientTransportMessage::Hello(hello) => hello,
        _ => {
            send_error(
                &mut socket,
                TransportErrorCode::InvalidHandshake,
                shared.config.wire_max_frame_bytes,
            );
            return Err(ProtocolTransportError::InvalidHandshake);
        }
    };
    let mut nonce = [0_u8; AUTH_CHALLENGE_BYTES];
    getrandom::fill(&mut nonce)
        .map_err(|_| ProtocolTransportError::Io(io::Error::other("random source unavailable")))?;
    write_message(
        &mut socket,
        &ServerTransportMessage::Challenge(AuthChallenge { nonce }),
        shared.config.wire_max_frame_bytes,
    )?;
    let proof = match read_message(&mut socket, shared.config.wire_max_frame_bytes)? {
        ClientTransportMessage::Authenticate(proof) => proof,
        _ => {
            send_error(
                &mut socket,
                TransportErrorCode::InvalidHandshake,
                shared.config.wire_max_frame_bytes,
            );
            return Err(ProtocolTransportError::InvalidHandshake);
        }
    };
    let expected = authentication_proof(shared.secret.bytes(), &nonce, &hello)?;
    if !constant_time_equal(&expected, &proof.proof) {
        send_error(
            &mut socket,
            TransportErrorCode::AuthenticationFailed,
            shared.config.wire_max_frame_bytes,
        );
        return Err(ProtocolTransportError::AuthenticationFailed);
    }
    let (welcome, recovery) = {
        let stream = lock(&shared.stream)?;
        let welcome = match stream.negotiate(&hello) {
            Ok(welcome) => welcome,
            Err(_) => {
                drop(stream);
                send_error(
                    &mut socket,
                    TransportErrorCode::ProtocolRejected,
                    shared.config.wire_max_frame_bytes,
                );
                return Err(ProtocolTransportError::ProtocolRejected);
            }
        };
        let after = hello.resume_after.unwrap_or(stream.latest_published());
        let recovery = stream.resume(after)?;
        (welcome, recovery)
    };
    write_message(
        &mut socket,
        &ServerTransportMessage::Ready {
            welcome,
            recovery: recovery.clone(),
        },
        shared.config.wire_max_frame_bytes,
    )?;
    let mut last_sent = recovery_latest_sent(&recovery, hello.resume_after);
    socket.set_read_timeout(Some(shared.config.heartbeat_interval))?;
    while !shutdown.load(Ordering::Acquire) {
        match read_message::<ClientTransportMessage>(
            &mut socket,
            shared.config.wire_max_frame_bytes,
        ) {
            Ok(ClientTransportMessage::Frame(frame)) => {
                validate_client_frame(&frame, &shared.stream)?;
                match frame.payload {
                    ClientPayload::Acknowledge(acknowledgement) => {
                        lock(&shared.stream)?.acknowledge(acknowledgement.through)?;
                        write_message(
                            &mut socket,
                            &ServerTransportMessage::Acknowledged {
                                through: acknowledgement.through,
                            },
                            shared.config.wire_max_frame_bytes,
                        )?;
                    }
                    ClientPayload::Resynchronize(request) => {
                        let snapshot = lock(&shared.recovery_snapshot)?.clone();
                        let Some(snapshot) = snapshot else {
                            send_error(
                                &mut socket,
                                TransportErrorCode::SnapshotUnavailable,
                                shared.config.wire_max_frame_bytes,
                            );
                            continue;
                        };
                        if request.max_entities == 0
                            || request.max_relations == 0
                            || request.timeout_ticks == 0
                            || snapshot.origin != request.origin
                            || snapshot.entities.len() > request.max_entities as usize
                            || snapshot.relations.len() > request.max_relations as usize
                        {
                            send_error(
                                &mut socket,
                                TransportErrorCode::ProtocolRejected,
                                shared.config.wire_max_frame_bytes,
                            );
                            continue;
                        }
                        let frame =
                            lock(&shared.stream)?.resynchronize(frame.correlation, snapshot)?;
                        write_message(
                            &mut socket,
                            &ServerTransportMessage::Frame(frame.clone()),
                            shared.config.wire_max_frame_bytes,
                        )?;
                        last_sent = frame.sequence;
                    }
                    _ => send_error(
                        &mut socket,
                        TransportErrorCode::UnsupportedRequest,
                        shared.config.wire_max_frame_bytes,
                    ),
                }
            }
            Ok(_) => {
                send_error(
                    &mut socket,
                    TransportErrorCode::InvalidHandshake,
                    shared.config.wire_max_frame_bytes,
                );
                return Err(ProtocolTransportError::InvalidHandshake);
            }
            Err(ProtocolTransportError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                last_sent = flush_since(&mut socket, shared, last_sent)?;
            }
            Err(ProtocolTransportError::PeerClosed) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn flush_since(
    socket: &mut TcpStream,
    shared: &SharedServer,
    after: StreamSequence,
) -> Result<StreamSequence, ProtocolTransportError> {
    let recovery = lock(&shared.stream)?.resume(after)?;
    match recovery {
        ResumeResult::Frames { frames, .. } if !frames.is_empty() => {
            let mut last = after;
            for frame in frames {
                write_message(
                    socket,
                    &ServerTransportMessage::Frame(frame.clone()),
                    shared.config.wire_max_frame_bytes,
                )?;
                last = frame.sequence;
            }
            Ok(last)
        }
        ResumeResult::Frames { .. } => {
            let heartbeat = lock(&shared.heartbeat)?.clone();
            let frame = lock(&shared.stream)?.publish(
                CorrelationId("heartbeat".into()),
                ServerPayload::Heartbeat(heartbeat),
            )?;
            write_message(
                socket,
                &ServerTransportMessage::Frame(frame.clone()),
                shared.config.wire_max_frame_bytes,
            )?;
            Ok(frame.sequence)
        }
        ResumeResult::SnapshotRequired {
            requested_after,
            earliest_available,
            latest_published,
        } => {
            write_message(
                socket,
                &ServerTransportMessage::SnapshotRequired {
                    requested_after,
                    earliest_available,
                    latest_published,
                },
                shared.config.wire_max_frame_bytes,
            )?;
            Ok(after)
        }
    }
}

fn validate_client_frame(
    frame: &ClientFrame,
    stream: &Mutex<ProtocolStream>,
) -> Result<(), ProtocolTransportError> {
    if frame.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolTransportError::ProtocolRejected);
    }
    let stream = lock(stream)?;
    let expected = stream.negotiate(&ProtocolHello {
        minimum_version: PROTOCOL_VERSION,
        maximum_version: PROTOCOL_VERSION,
        client_id: "authenticated".into(),
        resume_after: None,
    })?;
    if frame.stream_id != expected.stream_id {
        return Err(ProtocolTransportError::ProtocolRejected);
    }
    Ok(())
}

fn recovery_latest_sent(
    recovery: &ResumeResult,
    requested: Option<StreamSequence>,
) -> StreamSequence {
    match recovery {
        ResumeResult::Frames { frames, .. } => frames
            .last()
            .map(|frame| frame.sequence)
            .or(requested)
            .unwrap_or(StreamSequence(0)),
        ResumeResult::SnapshotRequired {
            requested_after, ..
        } => *requested_after,
    }
}

fn authentication_proof(
    secret: &[u8],
    nonce: &[u8; AUTH_CHALLENGE_BYTES],
    hello: &ProtocolHello,
) -> Result<[u8; AUTH_PROOF_BYTES], ProtocolTransportError> {
    let hello = serde_json::to_vec(hello)
        .map_err(|error| ProtocolTransportError::Serialization(error.to_string()))?;
    let mut message = Vec::with_capacity(AUTH_DOMAIN.len() + nonce.len() + hello.len());
    message.extend_from_slice(AUTH_DOMAIN);
    message.extend_from_slice(nonce);
    message.extend_from_slice(&hello);
    Ok(hmac_sha256(secret, &message))
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK];
    let mut outer_pad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner);
    outer.finalize().into()
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn write_message<T: Serialize>(
    stream: &mut TcpStream,
    message: &T,
    limit: usize,
) -> Result<(), ProtocolTransportError> {
    let encoded = serde_json::to_vec(message)
        .map_err(|error| ProtocolTransportError::Serialization(error.to_string()))?;
    if encoded.len() > limit {
        return Err(ProtocolTransportError::FrameTooLarge {
            bytes: encoded.len(),
            limit,
        });
    }
    let length =
        u32::try_from(encoded.len()).map_err(|_| ProtocolTransportError::FrameTooLarge {
            bytes: encoded.len(),
            limit,
        })?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&encoded)?;
    stream.flush()?;
    Ok(())
}

fn read_message<T: DeserializeOwned>(
    stream: &mut TcpStream,
    limit: usize,
) -> Result<T, ProtocolTransportError> {
    let mut prefix = [0_u8; 4];
    read_exact_or_closed(stream, &mut prefix)?;
    let bytes = u32::from_be_bytes(prefix) as usize;
    if bytes > limit {
        return Err(ProtocolTransportError::FrameTooLarge { bytes, limit });
    }
    let mut encoded = vec![0_u8; bytes];
    read_exact_or_closed(stream, &mut encoded)?;
    serde_json::from_slice(&encoded)
        .map_err(|error| ProtocolTransportError::Serialization(error.to_string()))
}

fn read_exact_or_closed(
    stream: &mut TcpStream,
    target: &mut [u8],
) -> Result<(), ProtocolTransportError> {
    match stream.read_exact(target) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
            ) =>
        {
            Err(ProtocolTransportError::PeerClosed)
        }
        Err(error) => Err(ProtocolTransportError::Io(error)),
    }
}

fn send_error(stream: &mut TcpStream, code: TransportErrorCode, limit: usize) {
    let _ = write_message(stream, &ServerTransportMessage::Error { code }, limit);
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ProtocolTransportError> {
    mutex.lock().map_err(|_| {
        ProtocolTransportError::Io(io::Error::other("protocol server state unavailable"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperationState, QueryCompletion, SituationDeltaMessage};
    use universe_core::{EntityKey, Revision, Tick, UniverseId};

    fn secret(value: u8) -> AuthenticationSecret {
        AuthenticationSecret::new([value; 32]).unwrap()
    }

    fn stream_budget(frames: usize) -> StreamBudget {
        StreamBudget {
            max_pending_frames: frames,
            max_pending_bytes: 64 * 1024,
            max_frame_bytes: 16 * 1024,
        }
    }

    fn config() -> ProtocolTransportConfig {
        ProtocolTransportConfig {
            wire_max_frame_bytes: 32 * 1024,
            max_connections: 4,
            io_timeout: Duration::from_secs(2),
            heartbeat_interval: Duration::from_millis(20),
        }
    }

    fn heartbeat(tick: u64) -> HeartbeatMessage {
        HeartbeatMessage {
            revision: Revision(tick),
            tick: Tick(tick),
            readiness: OperationState::Measured,
        }
    }

    fn snapshot(revision: u64) -> SituationSnapshotMessage {
        SituationSnapshotMessage {
            universe: UniverseId(1),
            revision: Revision(revision),
            tick: Tick(revision),
            origin: EntityKey(1),
            completion: QueryCompletion::Complete,
            entities: Vec::new(),
            relations: Vec::new(),
        }
    }

    fn hello(resume_after: Option<StreamSequence>) -> ProtocolHello {
        ProtocolHello {
            minimum_version: PROTOCOL_VERSION,
            maximum_version: PROTOCOL_VERSION,
            client_id: "desktop-test".into(),
            resume_after,
        }
    }

    fn delta(from: u64, to: u64) -> ServerPayload {
        ServerPayload::Delta(SituationDeltaMessage {
            universe: UniverseId(1),
            from_revision: Revision(from),
            to_revision: Revision(to),
            tick: Tick(to),
            entity_upserts: Vec::new(),
            relation_upserts: Vec::new(),
            relation_tombstones: Vec::new(),
        })
    }

    #[test]
    fn real_loopback_auth_delta_disconnect_and_exact_resume() {
        let server = RunningProtocolServer::bind_ephemeral_loopback(
            secret(7),
            "desktop-stream",
            stream_budget(8),
            config(),
            heartbeat(0),
            Some(snapshot(2)),
        )
        .unwrap();
        let client_result =
            ProtocolClient::connect(server.local_addr(), &secret(7), hello(None), config());
        if client_result.is_err() {
            thread::sleep(Duration::from_millis(20));
            panic!(
                "client={client_result:?}, server={:?}",
                server.connection_failures().unwrap()
            );
        }
        let mut client = client_result.unwrap();
        assert!(matches!(
            client.recovery(),
            ResumeResult::Frames { frames, .. } if frames.is_empty()
        ));
        let first = server
            .publish(CorrelationId("delta-1".into()), delta(0, 1))
            .unwrap();
        assert_eq!(client.read_next().unwrap(), first);
        drop(client);

        let second = server
            .publish(CorrelationId("delta-2".into()), delta(1, 2))
            .unwrap();
        let mut resumed = ProtocolClient::connect(
            server.local_addr(),
            &secret(7),
            hello(Some(first.sequence)),
            config(),
        )
        .unwrap();
        assert!(matches!(
            resumed.recovery(),
            ResumeResult::Frames { frames, .. } if frames == &vec![second.clone()]
        ));
        resumed
            .acknowledge(second.sequence, CorrelationId("ack-2".into()))
            .unwrap();
        server.shutdown().unwrap();
    }

    #[test]
    fn real_loopback_rejects_wrong_secret_without_leaking_either_secret() {
        let accepted = secret(11);
        let rejected = secret(19);
        let server = RunningProtocolServer::bind_ephemeral_loopback(
            accepted.clone(),
            "auth-stream",
            stream_budget(4),
            config(),
            heartbeat(0),
            None,
        )
        .unwrap();
        let error = ProtocolClient::connect(server.local_addr(), &rejected, hello(None), config())
            .unwrap_err();
        assert!(matches!(
            error,
            ProtocolTransportError::AuthenticationFailed
        ));
        let debug = format!("{accepted:?} {rejected:?} {error:?} {error}");
        assert!(!debug.contains(&format!("{:?}", [11_u8; 32])));
        assert!(!debug.contains(&format!("{:?}", [19_u8; 32])));
        assert!(debug.contains("[REDACTED]"));
        server.shutdown().unwrap();
    }

    #[test]
    fn real_loopback_lost_retention_requires_and_serves_snapshot() {
        let server = RunningProtocolServer::bind_ephemeral_loopback(
            secret(23),
            "snapshot-stream",
            stream_budget(4),
            config(),
            heartbeat(2),
            Some(snapshot(2)),
        )
        .unwrap();
        let first = server
            .publish(CorrelationId("delta-1".into()), delta(0, 1))
            .unwrap();
        let second = server
            .publish(CorrelationId("delta-2".into()), delta(1, 2))
            .unwrap();
        let mut current = ProtocolClient::connect(
            server.local_addr(),
            &secret(23),
            hello(Some(second.sequence)),
            config(),
        )
        .unwrap();
        current
            .acknowledge(second.sequence, CorrelationId("retire".into()))
            .unwrap();
        drop(current);
        thread::sleep(Duration::from_millis(10));

        let mut stale = ProtocolClient::connect(
            server.local_addr(),
            &secret(23),
            hello(Some(StreamSequence(0))),
            config(),
        )
        .unwrap();
        assert!(stale.welcome().resynchronization_required);
        assert!(matches!(
            stale.recovery(),
            ResumeResult::SnapshotRequired {
                requested_after: StreamSequence(0),
                latest_published,
                ..
            } if *latest_published == second.sequence
        ));
        stale
            .request_resynchronization(
                ResynchronizeRequest {
                    origin: EntityKey(1),
                    max_entities: 8,
                    max_relations: 8,
                    timeout_ticks: 2,
                },
                CorrelationId("snapshot-recovery".into()),
            )
            .unwrap();
        let recovered = stale.read_next().unwrap();
        assert!(matches!(
            recovered.payload,
            ServerPayload::Snapshot(SituationSnapshotMessage {
                revision: Revision(2),
                ..
            })
        ));
        assert!(recovered.sequence > first.sequence);
        server.shutdown().unwrap();
    }

    #[test]
    fn wire_size_limit_rejects_before_allocation_and_server_queue_backpressures() {
        let server = RunningProtocolServer::bind_ephemeral_loopback(
            secret(31),
            "bounded-stream",
            stream_budget(1),
            config(),
            heartbeat(0),
            None,
        )
        .unwrap();
        server
            .publish(CorrelationId("one".into()), delta(0, 1))
            .unwrap();
        assert!(matches!(
            server.publish(CorrelationId("two".into()), delta(1, 2)),
            Err(ProtocolTransportError::Backpressured)
        ));

        let address = server.local_addr();
        let mut raw = TcpStream::connect(address).unwrap();
        raw.write_all(&u32::MAX.to_be_bytes()).unwrap();
        raw.flush().unwrap();
        raw.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut response = [0_u8; 4];
        let result = raw.read_exact(&mut response);
        assert!(result.is_err());
        server.shutdown().unwrap();
    }
}
