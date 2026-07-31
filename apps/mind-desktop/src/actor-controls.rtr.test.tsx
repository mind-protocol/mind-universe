// @vitest-environment happy-dom
//
// Measures the runtime glue that unit tests over the pure core cannot reach:
// the REAL ActorControls useFrame hook, driven deterministically by
// @react-three/test-renderer (no RAF, no visible browser). We hold a key on
// window, advance frames by hand, and read the displacement the hook actually
// emits through onMove — closing the previously not_measured link.

import ReactThreeTestRenderer from "@react-three/test-renderer";
import { afterEach, describe, expect, it } from "vitest";
import { ActorControls } from "./ActorControls";
import { motionBounds, type MotionBounds } from "./actor-control";
import type { Vector3 as Vec3 } from "./contracts";

const buoyant: MotionBounds = {
  ...motionBounds,
  axes: { forward: true, right: true, up: true }
};

function press(code: string) {
  window.dispatchEvent(new KeyboardEvent("keydown", { code }));
}
function release(code: string) {
  window.dispatchEvent(new KeyboardEvent("keyup", { code }));
}

afterEach(() => {
  // Clear any key still held so state never leaks between measurements.
  for (const code of ["KeyE", "KeyZ", "KeyC", "KeyD"]) release(code);
});

async function driveOnce(
  bounds: MotionBounds,
  piloting: boolean,
  code: string
): Promise<Vec3[]> {
  const emitted: Vec3[] = [];
  const renderer = await ReactThreeTestRenderer.create(
    <ActorControls
      bounds={bounds}
      piloting={piloting}
      onMove={(d) => emitted.push(d)}
    />
  );
  press(code);
  await renderer.advanceFrames(4, 1 / 60);
  release(code);
  await renderer.unmount();
  return emitted;
}

describe("ActorControls runtime (measured via test-renderer)", () => {
  it("grounds the avatar: the vertical key (E) emits NO motion, even while piloting", async () => {
    const emitted = await driveOnce(motionBounds, true, "KeyE");
    expect(emitted).toEqual([]); // the shipped contract denies `up` — no flying
  });

  it("emits horizontal-only motion on forward input (Z), respecting the clamp", async () => {
    const emitted = await driveOnce(motionBounds, true, "KeyZ");
    expect(emitted.length).toBeGreaterThan(0);
    for (const d of emitted) {
      expect(Math.abs(d[1])).toBeLessThan(1e-6); // no vertical component
      expect(Math.hypot(d[0], d[1], d[2])).toBeLessThanOrEqual(
        motionBounds.maxTickDisplacement + 1e-9
      );
    }
  });

  it("emits NOTHING on a permitted axis when not granted — forward (Z), refused", async () => {
    const emitted = await driveOnce(motionBounds, false, "KeyZ");
    expect(emitted).toEqual([]);
  });

  it("still rises when a contract explicitly permits the vertical axis (buoyant)", async () => {
    // The gate mechanism is axis-agnostic: a fixture that DID permit `up` would
    // lift the body. Proves the grounding above comes from the contract, not a
    // hard-coded block in the runtime.
    const emitted = await driveOnce(buoyant, true, "KeyE");
    expect(emitted.length).toBeGreaterThan(0);
    expect(emitted.reduce((sum, d) => sum + d[1], 0)).toBeGreaterThan(0);
  });
});
