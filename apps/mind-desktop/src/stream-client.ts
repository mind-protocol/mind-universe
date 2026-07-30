// Production transport client (G3 item 1). A pure reducer over the desktop
// event stream that keeps HONEST connection health: it detects sequence gaps
// and requests resync, bounds its intake with backpressure (dropping the oldest,
// never silently), goes stale when frames stop arriving, and tracks reconnects.
// It never reports `synchronized` while a gap is unresolved.
//
// Time is injected (`nowMs`) so the reducer stays deterministic and testable —
// no `Date.now()` inside.

import { universeEventFromServerFrame } from "./protocol-adapter";
import { applyUniverseEvent, emptyUniverseView, type UniverseView } from "./universe-state";
import {
  emptyMetrics,
  frameBytes,
  recordApplied,
  recordDropped,
  recordGap,
  recordReceived,
  recordRejected,
  recordResync,
  type StreamMetrics
} from "./stream-metrics";

export type StreamHealth =
  | "connecting"
  | "synchronized"
  | "degraded"
  | "stale"
  | "disconnected";

export interface ResyncRequest {
  readonly reason: "sequence_gap" | "stale";
  readonly fromSequence: number;
}

export interface StreamConfig {
  /** Max frames accepted in one batch before backpressure drops the oldest. */
  readonly maxBatch: number;
  /** No applied frame within this many ms → the view is stale. */
  readonly staleAfterMs: number;
}

export const defaultStreamConfig = (): StreamConfig => ({
  maxBatch: 256,
  staleAfterMs: 5_000
});

export interface StreamState {
  readonly view: UniverseView;
  readonly health: StreamHealth;
  readonly metrics: StreamMetrics;
  readonly pendingResync: ResyncRequest | null;
  readonly reconnectAttempts: number;
}

export const initialStream = (): StreamState => ({
  view: emptyUniverseView(),
  health: "connecting",
  metrics: emptyMetrics(),
  pendingResync: null,
  reconnectAttempts: 0
});

// Ingests a single server frame at time `nowMs`.
export function ingestFrame(
  state: StreamState,
  frame: unknown,
  nowMs: number
): StreamState {
  const metrics = recordReceived(state.metrics, frameBytes(frame));
  const event = universeEventFromServerFrame(frame);

  if (!event) {
    // Unparseable or unsupported frame: counted, health degraded, nothing applied.
    return {
      ...state,
      metrics: recordRejected(metrics),
      health: state.health === "disconnected" ? state.health : "degraded"
    };
  }

  const previous = state.view;
  const view = applyUniverseEvent(previous, event);

  if (!view.synchronized && event.kind !== "snapshot_started") {
    // A sequence gap: applyUniverseEvent refused to advance. Request resync from
    // the last good sequence and stay degraded — never claim synchronized.
    return {
      ...state,
      view,
      metrics: recordResync(recordGap(metrics)),
      health: "degraded",
      pendingResync: { reason: "sequence_gap", fromSequence: previous.sequence }
    };
  }

  return {
    ...state,
    view,
    metrics: recordApplied(metrics, nowMs),
    health: "synchronized",
    // A fresh snapshot clears any outstanding resync request.
    pendingResync: event.kind === "snapshot_started" ? null : state.pendingResync
  };
}

// Ingests a batch with backpressure: if it exceeds `maxBatch`, the OLDEST
// overflow frames are dropped (counted), which itself produces a gap the client
// then resyncs from — bounded intake, never a silent stall.
export function ingestBatch(
  state: StreamState,
  frames: readonly unknown[],
  nowMs: number,
  config: StreamConfig = defaultStreamConfig()
): StreamState {
  let next = state;
  let accepted = frames;
  if (frames.length > config.maxBatch) {
    const overflow = frames.length - config.maxBatch;
    next = {
      ...next,
      metrics: recordDropped(next.metrics, overflow),
      health: next.health === "disconnected" ? next.health : "degraded"
    };
    accepted = frames.slice(overflow);
  }
  for (const frame of accepted) {
    next = ingestFrame(next, frame, nowMs);
  }
  return next;
}

// Marks the view stale if no frame has applied within the configured window.
export function checkStale(
  state: StreamState,
  nowMs: number,
  config: StreamConfig = defaultStreamConfig()
): StreamState {
  if (
    state.health !== "synchronized" ||
    state.metrics.lastAppliedAtMs === null ||
    nowMs - state.metrics.lastAppliedAtMs <= config.staleAfterMs
  ) {
    return state;
  }
  return {
    ...state,
    health: "stale",
    metrics: recordResync(state.metrics),
    pendingResync: { reason: "stale", fromSequence: state.view.sequence }
  };
}

export function onDisconnect(state: StreamState): StreamState {
  return {
    ...state,
    health: "disconnected",
    reconnectAttempts: state.reconnectAttempts + 1
  };
}

// A reconnect keeps the reconnect count but returns to connecting; the next
// snapshot resynchronises the view.
export function onReconnect(state: StreamState): StreamState {
  return { ...state, health: "connecting" };
}
