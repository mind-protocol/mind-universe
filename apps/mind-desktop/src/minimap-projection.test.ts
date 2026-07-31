import { describe, expect, it } from "vitest";
import {
  clampSize,
  clampZoom,
  minimapScale,
  projectToMinimap
} from "./minimap-projection";

const view = { width: 200, height: 200, radius: 40, zoom: 1, pad: 10 };

describe("minimap projection", () => {
  it("maps the plaza centre to the panel centre", () => {
    expect(projectToMinimap(0, 0, view)).toEqual([100, 100]);
  });

  it("fits the world radius inside the padded panel at zoom 1", () => {
    // A node at the fit radius lands (half - pad) px from centre — on the padding.
    const [px] = projectToMinimap(view.radius, 0, view);
    expect(px).toBeCloseTo(view.width / 2 + (100 - view.pad), 6);
  });

  it("scales linearly with zoom", () => {
    const base = minimapScale(view);
    expect(minimapScale({ ...view, zoom: 2 })).toBeCloseTo(base * 2, 9);
  });

  it("+z projects downward, +x rightward (top-down screen convention)", () => {
    const [px, py] = projectToMinimap(10, 5, view);
    expect(px).toBeGreaterThan(100);
    expect(py).toBeGreaterThan(100);
  });

  it("clamps zoom and size to sane ranges", () => {
    expect(clampZoom(1000)).toBe(8);
    expect(clampZoom(0.001)).toBe(0.25);
    expect(clampZoom(Number.NaN)).toBe(1);
    expect(clampSize(10)).toBe(120);
    expect(clampSize(9999)).toBe(640);
  });

  it("never divides by zero for a degenerate (single-node) city", () => {
    const [px, py] = projectToMinimap(0, 0, { ...view, radius: 0 });
    expect(Number.isFinite(px) && Number.isFinite(py)).toBe(true);
  });
});
