// Live transport connector (G3 item 1 — the socket wiring). This drives the
// authenticated protocol over an injected `Socket`: it performs the
// hello/welcome handshake, routes incoming server frames into the pure
// `stream-client` reducer, emits acknowledgements and resync requests, and
// tracks disconnects. The `Socket` is injected so the connector is testable
// without a network; the real implementation is a thin WebSocket/Tauri adapter.
//
// AUTHENTICATION BOUNDARY: this connector NEVER handles credentials. The caller
// opens an already-authenticated socket (token in the connection layer) and
// passes it in. No secret ever enters a protocol message here.
//
// SERVER CONTRACT (what the live endpoint must speak, mirroring
// `crates/universe-protocol`): after the socket opens the client sends a
// `hello` (ProtocolHello); the server replies `welcome` (ProtocolWelcome);
// thereafter the server streams `ServerFrame`s and the client sends
// `client_frame`s carrying `acknowledge` / `resynchronize` payloads.

import {
  checkStale,
  ingestFrame,
  initialStream,
  onDisconnect,
  onReconnect,
  type StreamConfig,
  type StreamState
} from "./stream-client";

export interface Socket {
  send(data: string): void;
  close(): void;
  onOpen?: () => void;
  onMessage?: (data: string) => void;
  onClose?: () => void;
}

export interface ResyncTarget {
  readonly origin: string;
  readonly maxEntities: number;
  readonly maxRelations: number;
  readonly timeoutTicks: number;
}

export interface TransportConfig {
  readonly clientId: string;
  readonly minVersion: number;
  readonly maxVersion: number;
  readonly resync: ResyncTarget;
  /** Send an acknowledgement after this many applied frames. */
  readonly ackEveryFrames: number;
  /** Injected clock (real app: () => performance.now()). */
  readonly nowMs: () => number;
  readonly streamConfig?: StreamConfig;
}

export type TransportPhase = "idle" | "handshaking" | "streaming" | "closed";

export class UniverseTransport {
  private socket: Socket | null = null;
  private phase: TransportPhase = "idle";
  private stream: StreamState = initialStream();
  private streamId: string | null = null;
  private resumeAfter: number | null = null;
  private appliedSinceAck = 0;
  private correlationSeq = 0;

  constructor(private readonly config: TransportConfig) {}

  /** Attach an already-authenticated socket and begin the handshake on open. */
  attach(socket: Socket): void {
    this.socket = socket;
    socket.onOpen = () => this.handleOpen();
    socket.onMessage = (data) => this.handleMessage(data);
    socket.onClose = () => this.handleClose();
  }

  getPhase(): TransportPhase {
    return this.phase;
  }

  getStream(): StreamState {
    return this.stream;
  }

  /** Drive stale detection from the app's animation/heartbeat loop. */
  tick(): void {
    this.stream = checkStale(this.stream, this.config.nowMs(), this.config.streamConfig);
    if (this.stream.pendingResync) this.emitResync();
  }

  private handleOpen(): void {
    this.phase = "handshaking";
    this.stream = onReconnect(this.stream);
    this.send({
      kind: "hello",
      minimum_version: this.config.minVersion,
      maximum_version: this.config.maxVersion,
      client_id: this.config.clientId,
      resume_after: this.resumeAfter
    });
  }

  private handleMessage(data: string): void {
    let message: Record<string, unknown>;
    try {
      message = JSON.parse(data) as Record<string, unknown>;
    } catch {
      return; // A malformed line is ignored; the frame layer counts real frames.
    }

    if (this.phase === "handshaking" && message.kind === "welcome") {
      this.streamId = typeof message.stream_id === "string" ? message.stream_id : null;
      this.phase = "streaming";
      if (message.resynchronization_required === true) {
        this.stream = { ...this.stream, pendingResync: { reason: "sequence_gap", fromSequence: this.stream.view.sequence } };
        this.emitResync();
      }
      return;
    }

    if (this.phase !== "streaming") return;

    // A server frame carries protocol_version + payload.
    if (typeof message.protocol_version === "number" && message.payload) {
      const before = this.stream.metrics.framesApplied;
      this.stream = ingestFrame(this.stream, message, this.config.nowMs());
      if (this.stream.metrics.framesApplied > before) {
        this.resumeAfter = this.stream.view.sequence;
        this.appliedSinceAck += 1;
        if (this.appliedSinceAck >= this.config.ackEveryFrames) this.emitAck();
      }
      if (this.stream.pendingResync) this.emitResync();
    }
  }

  private handleClose(): void {
    this.phase = "closed";
    this.stream = onDisconnect(this.stream);
  }

  private emitAck(): void {
    this.appliedSinceAck = 0;
    this.sendClientFrame({ message_type: "acknowledge", through: this.stream.view.sequence });
  }

  private emitResync(): void {
    const target = this.config.resync;
    this.sendClientFrame({
      message_type: "resynchronize",
      origin: target.origin,
      max_entities: target.maxEntities,
      max_relations: target.maxRelations,
      timeout_ticks: target.timeoutTicks
    });
  }

  private sendClientFrame(payload: Record<string, unknown>): void {
    this.send({
      kind: "client_frame",
      protocol_version: this.config.maxVersion,
      stream_id: this.streamId,
      correlation: `${this.config.clientId}:${this.correlationSeq++}`,
      payload
    });
  }

  private send(message: unknown): void {
    this.socket?.send(JSON.stringify(message));
  }
}
