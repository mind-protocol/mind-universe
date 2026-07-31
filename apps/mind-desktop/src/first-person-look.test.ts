import { describe, expect, it } from "vitest";
import { Euler, Vector3 } from "three";
import {
  applyLookDelta,
  groundBasis,
  orientationFromLookAt,
  PITCH_LIMIT
} from "./first-person-look";

// Reconstruct the world forward vector the renderer would get from an
// Euler(pitch, yaw, 0, "YXZ") applied to the camera's -Z, so the tests assert
// against the SAME convention the component uses.
function forwardOf(yaw: number, pitch: number): Vector3 {
  return new Vector3(0, 0, -1).applyEuler(new Euler(pitch, yaw, 0, "YXZ"));
}

describe("First-person look", () => {
  it("turns the head, not an orbit: dragging right yaws right, dragging down pitches down", () => {
    const right = applyLookDelta({ yaw: 0, pitch: 0 }, 100, 0, 0.01);
    const down = applyLookDelta({ yaw: 0, pitch: 0 }, 0, 100, 0.01);
    expect(right.yaw).toBeLessThan(0); // -Z forward yawing toward +X reads as negative yaw
    expect(right.pitch).toBe(0);
    expect(down.pitch).toBeLessThan(0);
    expect(down.yaw).toBe(0);
  });

  it("clamps pitch so the view never flips over the poles", () => {
    const wayUp = applyLookDelta({ yaw: 0, pitch: 0 }, 0, -100000, 0.01);
    const wayDown = applyLookDelta({ yaw: 0, pitch: 0 }, 0, 100000, 0.01);
    expect(wayUp.pitch).toBeCloseTo(PITCH_LIMIT);
    expect(wayDown.pitch).toBeCloseTo(-PITCH_LIMIT);
  });

  it("orientationFromLookAt round-trips: the derived head points at the target", () => {
    const eye: [number, number, number] = [0, 4, 13];
    const target: [number, number, number] = [0, 0, 0];
    const { yaw, pitch } = orientationFromLookAt(eye, target);
    const forward = forwardOf(yaw, pitch);
    const expected = new Vector3(
      target[0] - eye[0],
      target[1] - eye[1],
      target[2] - eye[2]
    ).normalize();
    expect(forward.x).toBeCloseTo(expected.x);
    expect(forward.y).toBeCloseTo(expected.y);
    expect(forward.z).toBeCloseTo(expected.z);
  });

  it("keeps walking level: the ground basis has no vertical term for any yaw", () => {
    for (const yaw of [0, 0.5, 1.7, -2.3, Math.PI]) {
      const { forward, right } = groundBasis(yaw);
      expect(forward[1]).toBe(0);
      expect(right[1]).toBe(0);
      // forward/right are unit and orthogonal on the ground plane
      expect(Math.hypot(forward[0], forward[2])).toBeCloseTo(1);
      expect(forward[0] * right[0] + forward[2] * right[2]).toBeCloseTo(0);
    }
  });

  it("ground forward matches the flattened look forward (looking up doesn't change where you walk)", () => {
    const yaw = 0.9;
    const level = groundBasis(yaw).forward;
    const lookingUp = forwardOf(yaw, 0.7);
    lookingUp.y = 0;
    lookingUp.normalize();
    expect(lookingUp.x).toBeCloseTo(level[0]);
    expect(lookingUp.z).toBeCloseTo(level[2]);
  });
});
