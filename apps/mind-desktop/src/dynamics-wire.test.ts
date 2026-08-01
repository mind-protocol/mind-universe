import { describe, expect, it } from "vitest";
// REAL bin output: `desktop_world_snapshot` run over a store whose nodes declare
// energy / weight / embedding (fixtures/desktop-world-snapshot/dynamics-world-seed.json),
// with the visual authority. This closes the wire gap end-to-end — the per-node
// dynamic signals travel from graph content, through the Rust projector, across
// the wire, through the adapter, into a rendered scene.
import worldFrames from "../../../fixtures/desktop-world-snapshot/dynamics/world-snapshot-frames.json";
import { universeEventFromServerFrame } from "./protocol-adapter";
import { renderSceneSvg } from "./scene-svg";
import { applyUniverseEvent, emptyUniverseView } from "./universe-state";
import { citizenEnergyMapping } from "./embodiment";

const EMISSIVE = "#1b9fff"; // palette.emissive — only a glowing node paints it.

function foldWorld() {
  let view = emptyUniverseView();
  for (const frame of worldFrames) {
    const event = universeEventFromServerFrame(frame);
    expect(event).not.toBeNull();
    view = applyUniverseEvent(view, event!);
  }
  return view;
}

describe("per-node dynamics survive the wire (bin → adapter → renderer)", () => {
  it("the projector attached energy, weight, and embedding to every node", () => {
    const view = foldWorld();
    expect(view.entities.size).toBe(3);
    for (const entity of view.entities.values()) {
      expect(entity.dynamics).toBeDefined();
      expect(typeof entity.dynamics!.energy).toBe("number");
      expect(typeof entity.dynamics!.weight).toBe("number");
      expect(Array.isArray(entity.dynamics!.embedding)).toBe(true);
      // The mapping carries the modulation envelope the renderer derives within.
      expect(citizenEnergyMapping(entity.embodiment!).dynamics).toBeDefined();
    }
  });

  it("the embedding micro-encoding round-trips to real numbers", () => {
    const ember = [...foldWorld().entities.values()].find(
      (entity) => entity.dynamics!.weight === 15
    )!;
    // seed embedding [900,100,-200] / scale 1000 → [0.9, 0.1, -0.2].
    expect(ember.dynamics!.embedding).toEqual([0.9, 0.1, -0.2]);
  });

  it("renders varied, epistemically-honest beings from the real frames", () => {
    const svg = renderSceneSvg(foldWorld());
    // Two measured nodes (Ember, Vega) glow; the unknown Nyx does not, despite
    // carrying energy 150 — the gate held all the way from graph to pixel.
    const halos = (svg.match(new RegExp(EMISSIVE, "g")) ?? []).length;
    expect(halos).toBe(2);
    // Distinct aura widths ⇒ the nodes are visibly different individuals.
    const auraWidths = new Set(
      [...svg.matchAll(/<ellipse[^>]*rx="([\d.]+)"/g)].map((m) => m[1])
    );
    expect(auraWidths.size).toBeGreaterThan(1);
  });
});
