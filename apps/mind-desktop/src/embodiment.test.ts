import { describe, expect, it } from "vitest";
import {
  advanceCriticalSpring,
  deterministicPointCloud,
  measureMotion,
  resolveEmbodimentForm,
  validateEmbodimentMapping
} from "./embodiment";
import {
  avatarMappingFixture,
  avatarMotionFixture
} from "./avatar-fixture";

describe("graph-driven embodiment", () => {
  it("accepts the graph-projected v1 mapping and resolves its LOD forms", () => {
    expect(validateEmbodimentMapping(avatarMappingFixture)).toBe(true);
    expect(resolveEmbodimentForm(avatarMappingFixture, "hot")).toHaveLength(8);
    expect(resolveEmbodimentForm(avatarMappingFixture, "sleeping")).toHaveLength(
      3
    );
  });

  it("fails closed when a mapping exceeds the native primitive ceiling", () => {
    const invalid = { ...avatarMappingFixture, primitive_budget: 13 };
    expect(validateEmbodimentMapping(invalid)).toBe(false);
    expect(resolveEmbodimentForm(invalid, "hot")).toBeNull();
  });

  it("derives movement only from two timestamped authoritative samples", () => {
    expect(measureMotion(undefined, [1, 0, 0], undefined, 1000)).toEqual({
      velocity: [0, 0, 0],
      speed: 0
    });
    expect(measureMotion([0, 0, 0], [1, 0, 0], 500, 1000)).toEqual({
      velocity: [2, 0, 0],
      speed: 2
    });
  });

  it("advances toward authority without overshooting a normal frame", () => {
    const next = advanceCriticalSpring(
      { position: [0, 0, 0], velocity: [0, 0, 0] },
      [1, 0, 0],
      avatarMotionFixture.interpolation.settle_seconds,
      1 / 60
    );
    expect(next.position[0]).toBeGreaterThan(0);
    expect(next.position[0]).toBeLessThan(1);
  });

  it("builds deterministic bounded particle positions", () => {
    const first = deterministicPointCloud(96, 0.78);
    const second = deterministicPointCloud(96, 0.78);
    expect(first).toEqual(second);
    expect(first).toHaveLength(96 * 3);
    expect(deterministicPointCloud(999, 1)).toHaveLength(160 * 3);
  });
});
