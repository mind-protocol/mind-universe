import { describe, expect, it } from "vitest";
import { observerMotion } from "./observer-controls";

describe("Observer controls", () => {
  it("supports both WASD and French ZQSD layouts", () => {
    expect(observerMotion(new Set(["KeyW", "KeyA"]))).toEqual(
      observerMotion(new Set(["KeyZ", "KeyQ"]))
    );
  });

  it("normalizes diagonal motion and applies deliberate speed modes", () => {
    const normal = observerMotion(new Set(["KeyW", "KeyD"]));
    const fast = observerMotion(new Set(["KeyW", "ShiftLeft"]));
    const precise = observerMotion(new Set(["KeyW", "AltLeft"]));

    expect(Math.hypot(normal.forward, normal.right, normal.up)).toBeCloseTo(1);
    expect(fast.speedMultiplier).toBe(3);
    expect(precise.speedMultiplier).toBe(0.25);
  });
});
