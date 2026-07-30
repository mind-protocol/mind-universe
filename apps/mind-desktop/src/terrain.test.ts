import { describe, expect, it } from "vitest";
import {
  TERRAIN_BASE_Y,
  terrainHeight
} from "./terrain";

describe("terrainHeight", () => {
  it("is deterministic: same coordinates give the same elevation", () => {
    expect(terrainHeight(3.2, -1.7)).toBe(terrainHeight(3.2, -1.7));
  });

  it("stays below the lowest fixture entity so foundations have positive length", () => {
    // The relief amplitude sums to at most 0.7 + 0.55 + 0.4 = 1.65, so the ground
    // top can never exceed base + 1.65 = -4.35, which is below the lowest entity
    // (world y ~ -3.84). Buildings therefore always rise out of the land.
    const maxAmplitude = 0.7 + 0.55 + 0.4;
    const lowestFixtureEntityY = -3.84;
    let peak = Number.NEGATIVE_INFINITY;
    for (let x = -60; x <= 60; x += 1.5) {
      for (let z = -60; z <= 60; z += 1.5) {
        peak = Math.max(peak, terrainHeight(x, z));
      }
    }
    expect(peak).toBeLessThanOrEqual(TERRAIN_BASE_Y + maxAmplitude);
    expect(peak).toBeLessThan(lowestFixtureEntityY);
  });

  it("actually rises and falls across the map (it is not flat)", () => {
    const samples = [
      terrainHeight(0, 0),
      terrainHeight(10, 4),
      terrainHeight(-8, 12),
      terrainHeight(20, -15)
    ];
    const spread = Math.max(...samples) - Math.min(...samples);
    expect(spread).toBeGreaterThan(0.5);
  });
});
