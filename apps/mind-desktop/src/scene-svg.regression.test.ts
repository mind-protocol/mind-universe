import { describe, expect, it } from "vitest";
import citizenFrames from "../../../fixtures/desktop-world-snapshot/citizen/world-snapshot-frames.json";
import deltaFrames from "../../../fixtures/desktop-world-snapshot/citizen/delta-batch2-frames.json";
import { universeEventFromServerFrame } from "./protocol-adapter";
import { renderSceneSvg } from "./scene-svg";
import { applyUniverseEvent, emptyUniverseView, type UniverseView } from "./universe-state";

function fold(frames: readonly unknown[], from?: UniverseView): UniverseView {
  let view = from ?? emptyUniverseView();
  for (const frame of frames) {
    const event = universeEventFromServerFrame(frame);
    if (event) view = applyUniverseEvent(view, event);
  }
  return view;
}

// Deterministic visual fixtures: each scenario renders the view to an SVG image
// checked against a committed golden file. Any drift in the rendered scene — a
// moved entity, a changed material, a lost relation — fails the test.
describe("scene visual regression", () => {
  it("normal operation", async () => {
    const svg = renderSceneSvg(fold(citizenFrames));
    await expect(svg).toMatchFileSnapshot("./__snapshots__/scene-normal.svg");
  });

  it("partial data (only the first entity received)", async () => {
    const svg = renderSceneSvg(fold(citizenFrames.slice(0, 2)));
    await expect(svg).toMatchFileSnapshot("./__snapshots__/scene-partial.svg");
  });

  it("progressive import (batch-2 delta folded in)", async () => {
    const svg = renderSceneSvg(fold(deltaFrames, fold(citizenFrames)));
    await expect(svg).toMatchFileSnapshot("./__snapshots__/scene-progressive.svg");
  });

  it("stale / degraded health", async () => {
    const svg = renderSceneSvg(fold(citizenFrames), { health: "stale" });
    await expect(svg).toMatchFileSnapshot("./__snapshots__/scene-stale.svg");
  });

  it("epistemic honesty is visible: an unknown entity is dimmer than a measured one", () => {
    const view = fold(citizenFrames);
    // Aurora (measured) vs Nyx (unknown) — see the citizen world seed.
    const aurora = view.entities.get("0000000000000000000000000000b001")!;
    const nyx = view.entities.get("0000000000000000000000000000b002")!;
    expect(nyx.visual.material.emissiveIntensity).toBe(0);
    expect(aurora.visual.material.emissiveIntensity).toBeGreaterThan(0);
    expect(nyx.visual.material.opacity).toBeLessThan(aurora.visual.material.opacity);
  });
});
