import { describe, expect, it } from "vitest";
import type { EntityAudio, MaterializedEntity } from "./contracts";
import {
  desiredAudioLoops,
  reconcileAudioLoops,
  type AudioReconciliation
} from "./audio-loops";

function thing(id: string, audio?: EntityAudio): MaterializedEntity {
  return {
    id,
    generation: 0,
    position: [0, 0, 0],
    visual: {
      primitive: "unknown",
      motion: "still",
      material: {
        color: "#fff",
        emissive: "#000",
        emissiveIntensity: 0,
        opacity: 1,
        scale: 1
      }
    },
    ...(audio ? { audio } : {})
  };
}

const CHIME: EntityAudio = { src: "asset://chime.wav", loop: true, gain: 0.8 };
const DRONE: EntityAudio = { src: "asset://drone.wav", loop: true, gain: 0.5 };

// Apply a reconciliation the way the React layer would, returning the active map.
function apply(
  active: ReadonlyMap<string, string>,
  reconciliation: AudioReconciliation
): Map<string, string> {
  return new Map(reconciliation.active);
}

describe("desiredAudioLoops", () => {
  it("selects only entities that carry an audio pointer", () => {
    const desired = desiredAudioLoops(
      [thing("silent"), thing("a", CHIME), thing("b", DRONE)],
      false
    );
    expect([...desired.keys()].sort()).toEqual(["a", "b"]);
    expect(desired.get("a")?.src).toBe("asset://chime.wav");
  });

  it("is empty when muted — muting is an honest silence", () => {
    expect(desiredAudioLoops([thing("a", CHIME)], true).size).toBe(0);
  });

  it("clamps gain into [0, 1]", () => {
    const desired = desiredAudioLoops(
      [thing("loud", { src: "s", loop: true, gain: 4 })],
      false
    );
    expect(desired.get("loud")?.gain).toBe(1);
  });

  it("ignores an audio facet with an empty source", () => {
    const desired = desiredAudioLoops(
      [thing("blank", { src: "", loop: true, gain: 1 })],
      false
    );
    expect(desired.size).toBe(0);
  });
});

describe("reconcileAudioLoops", () => {
  it("starts a loop when an audio thing materializes", () => {
    const r = reconcileAudioLoops(new Map(), [thing("a", CHIME)], false);
    expect(r.start.map((c) => c.id)).toEqual(["a"]);
    expect(r.stop).toEqual([]);
    expect(r.active.get("a")).toBe("asset://chime.wav");
  });

  it("does not restart a loop that is already sounding the same source", () => {
    const active = new Map([["a", "asset://chime.wav"]]);
    const r = reconcileAudioLoops(active, [thing("a", CHIME)], false);
    expect(r.start).toEqual([]);
    expect(r.stop).toEqual([]);
    expect(r.active.get("a")).toBe("asset://chime.wav");
  });

  it("stops a loop when its entity is released from the view", () => {
    const active = new Map([["a", "asset://chime.wav"]]);
    const r = reconcileAudioLoops(active, [], false);
    expect(r.start).toEqual([]);
    expect(r.stop).toEqual(["a"]);
    expect(r.active.size).toBe(0);
  });

  it("stops a loop when the entity loses its audio facet", () => {
    const active = new Map([["a", "asset://chime.wav"]]);
    const r = reconcileAudioLoops(active, [thing("a")], false);
    expect(r.stop).toEqual(["a"]);
    expect(r.active.size).toBe(0);
  });

  it("restarts (stop + start) when the source changes", () => {
    const active = new Map([["a", "asset://chime.wav"]]);
    const r = reconcileAudioLoops(active, [thing("a", DRONE)], false);
    expect(r.stop).toEqual(["a"]);
    expect(r.start.map((c) => c.src)).toEqual(["asset://drone.wav"]);
    expect(r.active.get("a")).toBe("asset://drone.wav");
  });

  it("stops every loop when muted, and restores them when unmuted", () => {
    const view = [thing("a", CHIME), thing("b", DRONE)];
    // Reach steady state.
    let active = apply(new Map(), reconcileAudioLoops(new Map(), view, false));
    expect(active.size).toBe(2);

    // Mute: everything stops, nothing starts.
    const muted = reconcileAudioLoops(active, view, true);
    expect(muted.stop.sort()).toEqual(["a", "b"]);
    expect(muted.start).toEqual([]);
    active = apply(active, muted);
    expect(active.size).toBe(0);

    // Unmute: everything starts again.
    const unmuted = reconcileAudioLoops(active, view, false);
    expect(unmuted.start.map((c) => c.id).sort()).toEqual(["a", "b"]);
    expect(unmuted.stop).toEqual([]);
  });

  it("is idempotent at steady state across repeated reconciliations", () => {
    const view = [thing("a", CHIME), thing("b", DRONE)];
    let active = new Map<string, string>();
    for (let i = 0; i < 3; i += 1) {
      const r = reconcileAudioLoops(active, view, false);
      active = apply(active, r);
      if (i > 0) {
        expect(r.start).toEqual([]);
        expect(r.stop).toEqual([]);
      }
    }
    expect(active.size).toBe(2);
  });
});
