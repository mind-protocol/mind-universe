import { describe, expect, it } from "vitest";
import citizenFrames from "../../../fixtures/desktop-world-snapshot/citizen/world-snapshot-frames.json";
import undergroundBinding from "../../../fixtures/assets/underground-visual-binding-v0.json";
import resolutionPolicy from "../../../fixtures/assets/visual-resolution-policy-v1.json";
import type { MaterializedEntity, UniverseEvent } from "./contracts";
import { universeEventFromServerFrame } from "./protocol-adapter";
import { renderSceneSvg } from "./scene-svg";
import { applyUniverseEvent, emptyUniverseView } from "./universe-state";

// Appearance is PROVENANCE-BASED and TOOLKIT-SCOPED: a node resolves its visual
// form through its producing toolkit's visual binding, keyed by role_axis /
// semantic_type. There is NO universal default. The actors (produced by the
// citizen-energy kit) resolve to the citizen mapping; the `thing` LEDGER — bound
// by no producing toolkit — resolves to the honest bare-presence fallback, NOT a
// defaulted citizen body.
const AURORA = "0000000000000000000000000000b001"; // actor, hot, measured
const NYX = "0000000000000000000000000000b002"; // actor, dormant, unknown
const LEDGER = "0000000000000000000000000000b003"; // thing, sleeping — unbound → fallback

function foldCitizenWorld() {
  let view = emptyUniverseView();
  for (const frame of citizenFrames) {
    const event = universeEventFromServerFrame(frame);
    expect(event).not.toBeNull();
    view = applyUniverseEvent(view, event!);
  }
  return view;
}

describe("provenance-based visual resolution in the renderer", () => {
  it("folds the resolved citizen world into the view", () => {
    const view = foldCitizenWorld();
    expect(view.synchronized).toBe(true);
    expect(view.entities.size).toBe(3);
    expect(view.relations.size).toBe(1);
  });

  it("carries each node's provenance so resolution can key on role / type", () => {
    const view = foldCitizenWorld();
    expect(view.entities.get(AURORA)!.provenance).toEqual({
      roleAxis: "actor",
      semanticType: "actor"
    });
    expect(view.entities.get(LEDGER)!.provenance).toEqual({
      roleAxis: "thing",
      semanticType: "thing"
    });
  });

  it("resolves the actors — and only the actors — to the citizen-energy binding", () => {
    const view = foldCitizenWorld();
    for (const id of [AURORA, NYX]) {
      const entity = view.entities.get(id);
      expect(entity?.embodiment).toBeDefined();
      expect(entity!.embodiment!.mapping.mapping_id).toBe(
        "citizen-energy-semi-humanoid-v1"
      );
    }
  });

  it("resolves the unbound `thing` to the bare-presence fallback, not the citizen form", () => {
    const view = foldCitizenWorld();
    const ledger = view.entities.get(LEDGER)!;
    // No producing-toolkit binding was inlined — the node is honestly unattributed.
    expect(ledger.embodiment).toBeUndefined();
    // And the scene draws it as the fallback presence point: the fallback particle
    // colour appears in the render, and it is NOT a citizen body (whose particle
    // colour is #d8f7ff).
    const svg = renderSceneSvg(view);
    const fallbackParticle = resolutionPolicy.fallback_presence.palette.particle;
    expect(fallbackParticle).toBe("#9db9c9");
    expect(svg).toContain(fallbackParticle);
  });

  it("lets residency drive the form for a bound actor: hot vs dormant", () => {
    const view = foldCitizenWorld();
    expect(view.entities.get(AURORA)!.embodiment!.residency).toBe("hot");
    expect(view.entities.get(NYX)!.embodiment!.residency).toBe("dormant");
  });

  it("keeps epistemic honesty: an unknown actor never emits as if confident", () => {
    const view = foldCitizenWorld();
    expect(
      view.entities.get(AURORA)!.visual.material.emissiveIntensity
    ).toBeGreaterThan(0);
    expect(view.entities.get(NYX)!.visual.material.emissiveIntensity).toBe(0);
    expect(view.entities.get(NYX)!.visual.material.opacity).toBeLessThan(
      view.entities.get(AURORA)!.visual.material.opacity
    );
  });
});

// A lightweight proof that the renderer consumes the ROLE-KEYED underground
// binding shape (archetypes[<name>].forms[<residency>]) — the same resolvedForm
// path a full underground snapshot would exercise. A node whose provenance is an
// underground role/type resolves to that toolkit's own infrastructure grammar
// (here the EnergyWell), NOT the citizen form and NOT the fallback.
describe("role-keyed toolkit binding (underground) resolves through provenance", () => {
  function undergroundView(semanticType: string, residency: string, roleAxis = "actor") {
    const entity: MaterializedEntity = {
      id: "u001",
      generation: 0,
      position: [0, 0, 0],
      visual: {
        primitive: "unknown",
        motion: "still",
        material: {
          color: "#c9b98a",
          emissive: "#f5a524",
          emissiveIntensity: 1.6,
          opacity: 0.9,
          scale: 1
        }
      },
      // The projector would inline the producing toolkit's binding as the mapping.
      embodiment: {
        source_mapping_id: undergroundBinding.authority_id,
        mapping: undergroundBinding as never,
        motion_profile: {} as never,
        residency: residency as never,
        sampled_at_ms: 0
      },
      provenance: { semanticType, roleAxis },
      dynamics: { embedding: [] }
    };
    const events: UniverseEvent[] = [
      { version: 0, sequence: 0, kind: "snapshot_started", revision: 1 },
      { version: 0, sequence: 1, kind: "entity_materialized", entity }
    ];
    return events.reduce(applyUniverseEvent, emptyUniverseView());
  }

  it("dresses an EnergyWell node in the underground core palette, not the fallback", () => {
    const svg = renderSceneSvg(undergroundView("EnergyWell", "hot"));
    // The underground core colour is drawn (the well's emitting head + shaft) ...
    expect(svg).toContain(undergroundBinding.palette.core);
    // ... and it is NOT the bare-presence fallback.
    expect(svg).not.toContain(resolutionPolicy.fallback_presence.palette.particle);
  });

  it("falls back to the bare presence for an underground role it does not declare", () => {
    // No archetype is declared_for this type and no role_axis matches → fallback.
    const svg = renderSceneSvg(undergroundView("NotAnUndergroundType", "hot", "unbound"));
    expect(svg).toContain(resolutionPolicy.fallback_presence.palette.particle);
  });
});
