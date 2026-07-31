import { describe, expect, it } from "vitest";
import {
  AVATAR_MAPPING_AUTHORITY,
  avatarMappingFixture,
  avatarMotionFixture
} from "./avatar-fixture";
import type { EntityDynamicSignals, MaterializedEntity } from "./contracts";
import { renderSceneSvg } from "./scene-svg";
import { emptyUniverseView, type UniverseView } from "./universe-state";

// Proves the per-node dynamics slice ON SCREEN: the SAME graph-resolved form
// (citizen semi_humanoid, hot) varies per node by energy → emission, weight
// (poids) → size, and embedding → orientation + micro-variation. This is the
// "plusieurs variantes" the north-star city-cathedral shows: one silhouette,
// every node an individual. Epistemic honesty holds — an unmeasured node with
// energy still does not glow.
const EMISSIVE = "#1b9fff"; // palette.emissive — only a glowing node paints it.

function actor(
  id: string,
  x: number,
  confident: boolean,
  dynamics: EntityDynamicSignals
): MaterializedEntity {
  return {
    id,
    generation: 0,
    position: [x, 0, 0],
    visual: {
      primitive: "unknown",
      motion: "still",
      material: {
        color: "#77d9ff",
        emissive: EMISSIVE,
        emissiveIntensity: confident ? 2.4 : 0,
        opacity: confident ? 0.82 : 0.3,
        scale: 1
      }
    },
    embodiment: {
      source_mapping_id: AVATAR_MAPPING_AUTHORITY,
      mapping: avatarMappingFixture,
      motion_profile: avatarMotionFixture,
      residency: "hot",
      sampled_at_ms: 1000
    },
    dynamics
  };
}

function viewOf(entities: readonly MaterializedEntity[]): UniverseView {
  return {
    ...emptyUniverseView(),
    revision: 1,
    sequence: 0,
    synchronized: true,
    entities: new Map(entities.map((entity) => [entity.id, entity]))
  };
}

// A row of citizens sharing ONE form, differing only by their live signals.
const CITY: readonly MaterializedEntity[] = [
  actor("faint-light", -5, true, { energy: 20, weight: 15, embedding: [0.9, 0.1, -0.2] }),
  actor("bright-heavy", -1.6, true, { energy: 155, weight: 95, embedding: [-0.4, 0.8, 0.3] }),
  actor("turned-mid", 1.8, true, { energy: 80, weight: 45, embedding: [0.2, -0.9, 0.5] }),
  // Unmeasured: carries energy, but must NOT glow (ALIGN §3 honesty gate).
  actor("unmeasured", 5, false, { energy: 150, weight: 60, embedding: [0.6, 0.6, 0.1] })
];

describe("scene dynamics regression: energy → emit, weight → size, embedding → variants", () => {
  it("renders the modulated city to a deterministic golden", async () => {
    const svg = renderSceneSvg(viewOf(CITY));
    await expect(svg).toMatchFileSnapshot("./__snapshots__/scene-dynamics.svg");
  });

  it("two nodes with the same form but different embeddings render differently", () => {
    const base = { energy: 80, weight: 45 };
    const one = renderSceneSvg(viewOf([actor("solo", 0, true, { ...base, embedding: [1, 0, 0] })]));
    const two = renderSceneSvg(
      viewOf([actor("solo", 0, true, { ...base, embedding: [-0.3, 0.9, 0.2] })])
    );
    // Same silhouette, same energy/weight — only the embedding differs, and it
    // is enough to make the two beings distinct (orientation + micro-variation).
    expect(one).not.toEqual(two);
  });

  it("energy brightens a measured node but NEVER an unmeasured one", () => {
    const measured = renderSceneSvg(
      viewOf([actor("m", 0, true, { energy: 155, weight: 40, embedding: [0.1, 0.2, 0.3] })])
    );
    const unmeasured = renderSceneSvg(
      viewOf([actor("u", 0, false, { energy: 155, weight: 40, embedding: [0.1, 0.2, 0.3] })])
    );
    // The measured node paints an emissive halo; the unmeasured one, despite the
    // same energy, paints none — its glow is gated to zero.
    expect(measured).toContain(EMISSIVE);
    expect(unmeasured).not.toContain(EMISSIVE);
  });

  it("weight (poids) enlarges the body: a heavier node draws a wider aura ellipse", () => {
    const rx = (svg: string): number =>
      Math.max(...[...svg.matchAll(/<ellipse[^>]*rx="([\d.]+)"/g)].map((m) => Number(m[1])));
    const light = rx(renderSceneSvg(viewOf([actor("l", 0, true, { weight: 0, embedding: [1, 0, 0] })])));
    const heavy = rx(renderSceneSvg(viewOf([actor("h", 0, true, { weight: 95, embedding: [1, 0, 0] })])));
    expect(heavy).toBeGreaterThan(light);
  });
});
