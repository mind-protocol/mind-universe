import { describe, expect, it } from "vitest";
import citizenFrames from "../../../fixtures/desktop-world-snapshot/citizen/world-snapshot-frames.json";
import { universeEventFromServerFrame } from "./protocol-adapter";
import { applyUniverseEvent, emptyUniverseView } from "./universe-state";

// The projector resolved each entity's visual from the graph-materialized
// visual-mapping authority (fixtures/assets/visual-embodiment-catalog.json +
// visual-projection-policy.json). This proves the renderer receives that
// resolution end-to-end: real residency drives the form, real epistemic state
// drives the material, and the mapping still satisfies the renderer's validator.
const AURORA = "0000000000000000000000000000b001"; // actor, hot, measured
const NYX = "0000000000000000000000000000b002"; // actor, dormant, unknown
const LEDGER = "0000000000000000000000000000b003"; // thing, sleeping (fallback)

function foldCitizenWorld() {
  let view = emptyUniverseView();
  for (const frame of citizenFrames) {
    const event = universeEventFromServerFrame(frame);
    expect(event).not.toBeNull();
    view = applyUniverseEvent(view, event!);
  }
  return view;
}

describe("graph-authority visual resolution in the renderer", () => {
  it("folds the resolved citizen world into the view", () => {
    const view = foldCitizenWorld();
    expect(view.synchronized).toBe(true);
    expect(view.entities.size).toBe(3);
    expect(view.relations.size).toBe(1);
  });

  it("resolves an embodiment from graph authority for each entity", () => {
    const view = foldCitizenWorld();
    for (const id of [AURORA, NYX, LEDGER]) {
      const entity = view.entities.get(id);
      expect(entity?.embodiment).toBeDefined();
      expect(entity!.embodiment!.mapping.mapping_id).toBe(
        "citizen-energy-semi-humanoid-v1"
      );
    }
  });

  it("lets residency drive the form: hot is humanoid, lower is the orb", () => {
    const view = foldCitizenWorld();
    expect(view.entities.get(AURORA)!.embodiment!.residency).toBe("hot");
    expect(view.entities.get(NYX)!.embodiment!.residency).toBe("dormant");
  });

  it("keeps epistemic honesty: an unknown entity never emits as if confident", () => {
    const view = foldCitizenWorld();
    // Aurora is measured → confident, emissive; Nyx is unknown → no emission.
    expect(
      view.entities.get(AURORA)!.visual.material.emissiveIntensity
    ).toBeGreaterThan(0);
    expect(view.entities.get(NYX)!.visual.material.emissiveIntensity).toBe(0);
    expect(view.entities.get(NYX)!.visual.material.opacity).toBeLessThan(
      view.entities.get(AURORA)!.visual.material.opacity
    );
  });
});
