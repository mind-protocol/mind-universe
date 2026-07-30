import { describe, expect, it } from "vitest";
import citizenFrames from "../../../fixtures/desktop-world-snapshot/citizen/world-snapshot-frames.json";
import {
  checkStale,
  defaultStreamConfig,
  ingestBatch,
  ingestFrame,
  initialStream,
  onDisconnect,
  onReconnect
} from "./stream-client";

const T0 = 1_000;

describe("production transport client", () => {
  it("synchronises on a clean, in-order stream and measures it", () => {
    const state = ingestBatch(initialStream(), citizenFrames, T0);
    expect(state.health).toBe("synchronized");
    expect(state.view.entities.size).toBe(3);
    expect(state.pendingResync).toBeNull();
    expect(state.metrics.framesReceived).toBe(citizenFrames.length);
    expect(state.metrics.framesApplied).toBe(citizenFrames.length);
    expect(state.metrics.bytesReceived).toBeGreaterThan(0);
    // The GPU-side metrics are honestly not measured by the stream layer.
    expect(state.metrics.gpuMetricsMeasured).toBe(false);
  });

  it("detects a sequence gap and requests resync without claiming sync", () => {
    let state = ingestFrame(initialStream(), citizenFrames[0], T0); // snapshot, seq 1
    expect(state.health).toBe("synchronized");
    // Skip seq 2, apply seq 3 → gap.
    state = ingestFrame(state, citizenFrames[2], T0);
    expect(state.health).toBe("degraded");
    expect(state.view.synchronized).toBe(false);
    expect(state.pendingResync).toEqual({ reason: "sequence_gap", fromSequence: 1 });
    expect(state.metrics.gapsDetected).toBe(1);
    expect(state.metrics.resyncsRequested).toBe(1);
  });

  it("counts an unparseable frame as rejected and degrades", () => {
    const state = ingestFrame(initialStream(), { not: "a frame" }, T0);
    expect(state.metrics.framesRejected).toBe(1);
    expect(state.metrics.framesApplied).toBe(0);
    expect(state.health).toBe("degraded");
  });

  it("drops the oldest frames under backpressure, never silently", () => {
    const config = { ...defaultStreamConfig(), maxBatch: 2 };
    const state = ingestBatch(initialStream(), citizenFrames, T0, config);
    // 5 frames, capacity 2 → 3 dropped.
    expect(state.metrics.framesDropped).toBe(citizenFrames.length - 2);
    expect(state.health).toBe("degraded");
  });

  it("goes stale when frames stop arriving", () => {
    let state = ingestBatch(initialStream(), citizenFrames, T0);
    expect(state.health).toBe("synchronized");
    const config = defaultStreamConfig();
    state = checkStale(state, T0 + config.staleAfterMs + 1, config);
    expect(state.health).toBe("stale");
    expect(state.pendingResync?.reason).toBe("stale");
  });

  it("tracks disconnect and reconnect", () => {
    let state = onDisconnect(initialStream());
    expect(state.health).toBe("disconnected");
    expect(state.reconnectAttempts).toBe(1);
    state = onReconnect(state);
    expect(state.health).toBe("connecting");
    expect(state.reconnectAttempts).toBe(1);
  });
});
