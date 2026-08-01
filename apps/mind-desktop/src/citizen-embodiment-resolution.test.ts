import { describe, expect, it } from "vitest";
import citizenFrames from "../../../fixtures/desktop-world-snapshot/citizen/world-snapshot-frames.json";
import undergroundToolkit from "../../../fixtures/ontology/underground-toolkit-v0.json";
import resolutionPolicy from "../../../fixtures/assets/visual-resolution-policy-v1.json";
import type { MaterializedEntity, UniverseEvent } from "./contracts";
import { universeEventFromServerFrame } from "./protocol-adapter";
import { renderSceneSvg } from "./scene-svg";
import { applyUniverseEvent, emptyUniverseView } from "./universe-state";
import { citizenEnergyMapping } from "./embodiment";

// Appearance lives IN the toolkit: the underground binding is carried AS a member
// of the construct (subtype physicalization_binding), reached via the construct's
// `space PROJECTS_AS visual_binding` edge, not a standalone file. We read that
// member's content the way the projector would resolve it through provenance.
const undergroundMember = (
  undergroundToolkit.members as Array<{ id: string; content: Record<string, unknown> }>
).find((m) => m.id === "visual_binding:l2:mind-universe:underground-toolkit-v0")!;
const undergroundBinding = {
  id: undergroundMember.id,
  ...undergroundMember.content
} as { id: string; palette: { core: string }; [key: string]: unknown };

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
      expect(citizenEnergyMapping(entity!.embodiment!).mapping_id).toBe(
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
        source_mapping_id: undergroundBinding.id,
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

// A toolkit's OWN binding, arriving on the wire the way the projector now emits
// it: role-keyed mapping, no motion profile, and the provenance to key on. Every
// one of these was a reason the binding never reached the renderer — the adapter
// required the LOD-keyed schema and a motion profile, so it dropped the frame.
const UNDERGROUND_MEMBER = "0000000000000000000000000000c001";

function undergroundFrame(residency: string, roleAxis: string) {
  return {
    protocol_version: 0,
    sequence: 2,
    payload: {
      message_type: "entity_materialized",
      entity: {
        id: UNDERGROUND_MEMBER,
        generation: 0,
        symbol: "narrative",
        content_kind: "justification",
        residency,
        position_micro: [0, 0, 0],
        placement: { provenance: "scaffold" },
        visual: {
          primitive: "unknown",
          motion: "still",
          material: {
            color: "#8a97a8",
            emissive: "#2b3440",
            emissive_intensity_micro: 0,
            opacity_micro: 700_000,
            scale_micro: 1_000_000
          }
        },
        label: "narrative",
        detail: "justification",
        state: residency,
        epistemic: "measured",
        dynamics: { embedding_micro: [0, 0, 0, 0] },
        provenance: {
          canonical_id: "justification:l2:mind-universe:underground-toolkit-v0",
          role_axis: roleAxis,
          semantic_type: "justification",
          producing_toolkit: "space:l2:mind-universe:underground-toolkit-v0"
        },
        embodiment: {
          source_mapping_id: undergroundMember.id,
          mapping: undergroundMember.content,
          residency,
          sampled_at_ms: 0,
          resolved_form: null,
          confident: true
        }
      }
    }
  };
}

function foldUnderground(residency: string, roleAxis: string) {
  let view = emptyUniverseView();
  const frames = [
    {
      protocol_version: 0,
      sequence: 1,
      payload: { message_type: "snapshot", revision: 1 }
    },
    undergroundFrame(residency, roleAxis)
  ];
  for (const frame of frames) {
    const event = universeEventFromServerFrame(frame);
    expect(event).not.toBeNull();
    view = applyUniverseEvent(view, event as UniverseEvent);
  }
  return view;
}

describe("a toolkit dresses what it produced", () => {
  it("keeps a role-keyed binding that declares no motion profile", () => {
    const entity = foldUnderground("sleeping", "narrative").entities.get(
      UNDERGROUND_MEMBER
    ) as MaterializedEntity;
    expect(entity.embodiment).toBeDefined();
    expect(entity.embodiment!.mapping.schema_version).toBe(
      "visual-embodiment/1-role-keyed"
    );
    // Nothing was invented to fill the gap the authority left.
    expect(entity.embodiment!.motion_profile).toBeUndefined();
  });

  it("draws the underground archetype for the node's role, not the bare fallback", () => {
    const svg = renderSceneSvg(foldUnderground("sleeping", "narrative"));
    // The reservoir archetype is the one declared for role_axis `narrative`; its
    // form is drawn in the underground toolkit's own palette.
    expect(svg).toContain("#c9b98a");
    expect(svg).not.toContain(resolutionPolicy.fallback_presence.palette.core);
  });

  it("leaves a role its toolkit never declared honestly unbound", () => {
    // The underground binding declares no archetype for `metric`, a node_type off
    // the closed role axis. No archetype ⇒ bare presence, never a borrowed dress.
    const svg = renderSceneSvg(foldUnderground("sleeping", "metric"));
    // The bare presence is a single particle point, drawn in the fallback
    // palette's particle colour — a recognizably unbound node.
    expect(svg).toContain(resolutionPolicy.fallback_presence.palette.particle);
    expect(svg).not.toContain("#c9b98a");
  });
});
