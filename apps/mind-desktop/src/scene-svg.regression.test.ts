import { describe, expect, it } from "vitest";
import citizenFrames from "../../../fixtures/desktop-world-snapshot/citizen/world-snapshot-frames.json";
import deltaFrames from "../../../fixtures/desktop-world-snapshot/citizen/delta-batch2-frames.json";
import type {
  MaterializedRelation,
  RelationVisualDescriptor,
  UniverseEvent,
  Vector3
} from "./contracts";
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

  it("lantern mode fogs the un-measured and reveals the measured", async () => {
    const svg = renderSceneSvg(fold(citizenFrames), { lantern: true });
    await expect(svg).toMatchFileSnapshot("./__snapshots__/scene-lantern.svg");
    // Nyx (unknown) is shrouded — the Lantern marks it "?" rather than faking it.
    expect(svg).toContain(">?</text>");
    expect(svg).toContain("lantern: revealing epistemic status");
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

// ALIGN §2/§6 — the link "pente + couleur" slice: a bond derives its colour from
// polarity sign and its slope from hierarchy, each strictly opt-in. These tests
// prove the two channels are legible AND orthogonal, and that an absent channel
// leaves the bond exactly neutral (the honesty contract: no faked slope/polarity).
describe("bond channels: polarity → colour, hierarchy → slope", () => {
  const NEUTRAL = "#5a6675";
  const EXCITATION = "#46e0d0";
  const INHIBITION = "#d8607a";

  function bondVisual(extra: Partial<RelationVisualDescriptor>): RelationVisualDescriptor {
    return {
      primitive: "dual_lane_bond",
      material: {
        color: NEUTRAL,
        emissive: "#1f2731",
        emissiveIntensity: 0,
        opacity: 0.5,
        scale: 1
      },
      width: 0.7,
      laneSeparation: 0,
      ...extra
    };
  }

  function twoNodeView(visual: RelationVisualDescriptor): UniverseView {
    const entity = (id: string, position: Vector3): UniverseEvent => ({
      version: 0,
      sequence: 0,
      kind: "entity_materialized",
      entity: {
        id,
        generation: 0,
        position,
        visual: {
          primitive: "unknown",
          motion: "still",
          material: {
            color: "#8a97a8",
            emissive: "#2b3440",
            emissiveIntensity: 0,
            opacity: 0.7,
            scale: 1
          }
        },
        dynamics: { embedding: [] }
      }
    });
    const relation: MaterializedRelation = { id: "r1", source: "a", target: "b", visual };
    const events: UniverseEvent[] = [
      { version: 0, sequence: 0, kind: "snapshot_started", revision: 1 },
      entity("a", [-2, 0, 0]),
      entity("b", [2, 1.5, 0]),
      { version: 0, sequence: 0, kind: "relation_materialized", relation }
    ].map((event, index) => ({ ...event, sequence: index }) as UniverseEvent);
    return events.reduce(applyUniverseEvent, emptyUniverseView());
  }

  const bondPath = (svg: string): string => svg.match(/<path d="M[^/]*?\/>/)![0];

  it("positive polarity paints the bond as excitation, negative as inhibition", () => {
    const excite = bondPath(renderSceneSvg(twoNodeView(bondVisual({ polarity: [0.85, 0.35] }))));
    const inhibit = bondPath(renderSceneSvg(twoNodeView(bondVisual({ polarity: [-0.6, -0.2] }))));
    expect(excite).toContain(`stroke="${EXCITATION}"`);
    expect(inhibit).toContain(`stroke="${INHIBITION}"`);
  });

  it("an absent polarity keeps the neutral material colour — never a faked polarity", () => {
    const neutral = bondPath(renderSceneSvg(twoNodeView(bondVisual({}))));
    expect(neutral).toContain(`stroke="${NEUTRAL}"`);
    expect(neutral).not.toContain(EXCITATION);
    expect(neutral).not.toContain(INHIBITION);
  });

  it("hierarchy tilts the bond's crown, and does so independently of colour", () => {
    const flat = bondPath(renderSceneSvg(twoNodeView(bondVisual({}))));
    const hierarchical = bondPath(renderSceneSvg(twoNodeView(bondVisual({ hierarchy: 0.75 }))));
    // Same colour (polarity absent on both) but a different arc: the slope channel
    // moved the quadratic control point without touching the stroke.
    expect(hierarchical).toContain(`stroke="${NEUTRAL}"`);
    expect(hierarchical).not.toEqual(flat);
  });

  it("a bond with neither channel is byte-identical to the plain neutral bond", () => {
    const bare = bondPath(renderSceneSvg(twoNodeView(bondVisual({}))));
    // The control point is the plain midpoint lifted by 26 — no slope term applied.
    expect(bare).toMatch(/Q 240\.00 /);
  });
});
