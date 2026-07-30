// Stream-side observability (G3 item 7). These are the metrics the desktop can
// honestly measure from the event stream itself — counts and bandwidth. The
// renderer-side metrics (frame time, event-to-photon latency, draw calls, GPU
// memory) require the GPU pipeline and are deliberately NOT claimed here; the
// `gpuMetricsMeasured: false` flag makes that explicit rather than reporting a
// fabricated zero.

export interface StreamMetrics {
  readonly framesReceived: number;
  readonly framesApplied: number;
  /** Frames that failed to parse or were an unsupported message type. */
  readonly framesRejected: number;
  /** Frames dropped under backpressure (never silently). */
  readonly framesDropped: number;
  readonly bytesReceived: number;
  readonly gapsDetected: number;
  readonly resyncsRequested: number;
  readonly lastAppliedAtMs: number | null;
  readonly gpuMetricsMeasured: false;
}

export const emptyMetrics = (): StreamMetrics => ({
  framesReceived: 0,
  framesApplied: 0,
  framesRejected: 0,
  framesDropped: 0,
  bytesReceived: 0,
  gapsDetected: 0,
  resyncsRequested: 0,
  lastAppliedAtMs: null,
  gpuMetricsMeasured: false
});

/** Approximate on-wire size of a frame (bytes of its JSON encoding). */
export function frameBytes(frame: unknown): number {
  try {
    return JSON.stringify(frame)?.length ?? 0;
  } catch {
    return 0;
  }
}

export function recordReceived(m: StreamMetrics, bytes: number): StreamMetrics {
  return { ...m, framesReceived: m.framesReceived + 1, bytesReceived: m.bytesReceived + bytes };
}

export function recordApplied(m: StreamMetrics, atMs: number): StreamMetrics {
  return { ...m, framesApplied: m.framesApplied + 1, lastAppliedAtMs: atMs };
}

export function recordRejected(m: StreamMetrics): StreamMetrics {
  return { ...m, framesRejected: m.framesRejected + 1 };
}

export function recordDropped(m: StreamMetrics, count: number): StreamMetrics {
  return { ...m, framesDropped: m.framesDropped + count };
}

export function recordGap(m: StreamMetrics): StreamMetrics {
  return { ...m, gapsDetected: m.gapsDetected + 1 };
}

export function recordResync(m: StreamMetrics): StreamMetrics {
  return { ...m, resyncsRequested: m.resyncsRequested + 1 };
}
