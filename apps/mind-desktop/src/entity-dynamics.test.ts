import { describe, expect, it } from "vitest";
import { avatarMappingFixture } from "./avatar-fixture";
import type { EmbodimentDynamics } from "./contracts";
import {
  deriveEntityDynamics,
  IDENTITY_DYNAMICS,
  primitiveJitter
} from "./entity-dynamics";

// The graph-declared envelope the renderer derives within (fixtures/assets/
// visual-embodiment-catalog.json → dynamics). If the authority ever drops it,
// this suite fails loudly rather than silently testing a made-up envelope.
const bounds = avatarMappingFixture.dynamics as EmbodimentDynamics;

describe("per-node dynamics derivation", () => {
  it("the shipped authority declares a dynamics envelope", () => {
    expect(bounds).toBeDefined();
    expect(bounds.weight_to_scale).toHaveLength(4);
    expect(bounds.energy_to_emissive).toHaveLength(4);
  });

  it("no declared envelope ⇒ strict identity (every node at its base form)", () => {
    expect(
      deriveEntityDynamics(undefined, { confident: true, energy: 999, weight: 999 })
    ).toEqual(IDENTITY_DYNAMICS);
  });

  it("weight (poids) enlarges the node, monotonically and within bounds", () => {
    const [, inMax, outMin, outMax] = bounds.weight_to_scale;
    const light = deriveEntityDynamics(bounds, { confident: true, weight: 0 }).scale;
    const heavy = deriveEntityDynamics(bounds, { confident: true, weight: inMax }).scale;
    expect(light).toBeCloseTo(outMin);
    expect(heavy).toBeCloseTo(outMax);
    expect(heavy).toBeGreaterThan(light);
    // Above the input span it clamps — never runs away past the envelope.
    expect(deriveEntityDynamics(bounds, { confident: true, weight: inMax * 10 }).scale).toBeCloseTo(
      outMax
    );
    // An unknown weight is never invented into a size.
    expect(deriveEntityDynamics(bounds, { confident: true }).scale).toBe(1);
  });

  it("energy brightens ONLY a confident node (ALIGN §3 honesty gate)", () => {
    const [, energyMax, , emitMax] = bounds.energy_to_emissive;
    const measured = deriveEntityDynamics(bounds, { confident: true, energy: energyMax });
    expect(measured.emissiveMultiplier).toBeCloseTo(emitMax);
    // Same energy, but not epistemically confident ⇒ no brightening at all.
    const unmeasured = deriveEntityDynamics(bounds, { confident: false, energy: energyMax });
    expect(unmeasured.emissiveMultiplier).toBe(1);
    // Confident but no energy ⇒ no boost invented.
    expect(deriveEntityDynamics(bounds, { confident: true }).emissiveMultiplier).toBe(1);
  });

  it("embedding orients the node within the declared max, and distinct embeddings differ", () => {
    const max = bounds.embedding_orientation_max_rad;
    const a = deriveEntityDynamics(bounds, { confident: true, embedding: [1, 0, 0] });
    const b = deriveEntityDynamics(bounds, { confident: true, embedding: [-1, 0.5, 0] });
    expect(Math.abs(a.yaw)).toBeLessThanOrEqual(max + 1e-9);
    expect(Math.abs(b.yaw)).toBeLessThanOrEqual(max + 1e-9);
    // Two different embeddings ⇒ different orientation: the procedural variant.
    expect(a.yaw).not.toBeCloseTo(b.yaw);
    // Micro-variation amplitude stays inside its bound.
    expect(a.microvariation).toBeGreaterThanOrEqual(0);
    expect(a.microvariation).toBeLessThanOrEqual(bounds.embedding_microvariation + 1e-9);
  });

  it("is deterministic — same signals always yield the same modulation", () => {
    const signals = { confident: true, energy: 120, weight: 40, embedding: [0.3, -0.7, 0.1] };
    expect(deriveEntityDynamics(bounds, signals)).toEqual(deriveEntityDynamics(bounds, signals));
  });
});

describe("procedural per-primitive jitter", () => {
  const embedding = [0.4, -0.2, 0.9, 0.1];
  const amplitude = 0.14;

  it("is identity with no embedding or zero amplitude (preserves the base form)", () => {
    expect(primitiveJitter(undefined, 3, amplitude)).toEqual({
      offset: [0, 0, 0],
      scale: [1, 1, 1]
    });
    expect(primitiveJitter(embedding, 3, 0)).toEqual({ offset: [0, 0, 0], scale: [1, 1, 1] });
  });

  it("is deterministic and bounded by the amplitude", () => {
    const first = primitiveJitter(embedding, 2, amplitude);
    expect(primitiveJitter(embedding, 2, amplitude)).toEqual(first);
    for (const axis of first.offset) expect(Math.abs(axis)).toBeLessThanOrEqual(amplitude + 1e-9);
    for (const axis of first.scale) expect(Math.abs(axis - 1)).toBeLessThanOrEqual(amplitude + 1e-9);
  });

  it("varies per primitive index — different parts deform differently", () => {
    expect(primitiveJitter(embedding, 0, amplitude)).not.toEqual(
      primitiveJitter(embedding, 1, amplitude)
    );
  });
});
