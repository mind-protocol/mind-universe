// The SSE bridge's frames must decode through the REAL production reducer. This
// pins stream-frames.mjs to protocol-adapter + universe-state so the dev bridge
// and the native path can never drift apart in wire shape.

import { describe, expect, it } from "vitest";
import { cityToFrames, PROTOCOL_VERSION } from "./stream-frames.mjs";
import { universeEventFromServerFrame } from "../src/protocol-adapter.ts";
import { applyUniverseEvent, emptyUniverseView } from "../src/universe-state.ts";
import {
  entityPresentationFromFrame,
  relationPresentationFromFrame
} from "../src/stream-presentation.ts";

// A synthetic 2-node, 1-street city (no store read — hermetic).
const city = {
  revision: 11,
  entities: [
    {
      id: "00000000000000000000000000001000",
      district: "canon",
      x: 12.5,
      z: -4.25,
      primitive: "open_polyhedral_attractor",
      motion: "still",
      color: "#9b8fb0",
      emissive: "#9b8fb0",
      emissiveIntensity: 0.35,
      opacity: 0.92,
      scale: 0.5,
      epistemic: "unknown",
      label: "actor",
      state: "active",
      detail: "ontology_definition"
    },
    {
      id: "00000000000000000000000000002000",
      district: "physics",
      x: -8,
      z: 3,
      primitive: "torus_knot",
      motion: "still",
      color: "#d86a5a",
      emissive: "#d86a5a",
      emissiveIntensity: 0.35,
      opacity: 0.92,
      scale: 0.55,
      epistemic: "not_measured",
      label: "GROUNDS",
      state: "prototype_not_calibrated",
      detail: "physical_profile"
    }
  ],
  relations: [
    {
      id: "00000000000000000000000000001000--GROUNDS-->00000000000000000000000000002000",
      source: "00000000000000000000000000001000",
      target: "00000000000000000000000000002000",
      predicate: "GROUNDS",
      physics: {
        family: "normative",
        polarity: [0.85, 0.55],
        hierarchy: 0.45,
        permanence: 0.9,
        mode: "composite",
        calibrated: false
      }
    }
  ]
};

describe("cityToFrames", () => {
  it("emits contiguous, versioned frames starting at the given sequence", () => {
    const { frames, nextSeq } = cityToFrames(city, 1);
    expect(frames).toHaveLength(1 + city.entities.length + city.relations.length);
    expect(frames[0].payload.message_type).toBe("snapshot");
    frames.forEach((frame, index) => {
      expect(frame.protocol_version).toBe(PROTOCOL_VERSION);
      expect(frame.sequence).toBe(1 + index);
    });
    expect(nextSeq).toBe(1 + frames.length);
  });

  it("decodes through the real adapter into a synchronized view", () => {
    const { frames } = cityToFrames(city, 1);
    let view = emptyUniverseView();
    for (const frame of frames) {
      const event = universeEventFromServerFrame(frame);
      expect(event).not.toBeNull();
      view = applyUniverseEvent(view, event);
    }
    expect(view.synchronized).toBe(true);
    expect(view.revision).toBe(11);
    expect(view.entities.size).toBe(2);
    expect(view.relations.size).toBe(1);

    // The layout position survives the micro round-trip (x/z; y seeds at 0).
    const node = view.entities.get("00000000000000000000000000001000");
    expect(node?.position[0]).toBeCloseTo(12.5, 6);
    expect(node?.position[2]).toBeCloseTo(-4.25, 6);

    // The physical_profile channels reach the Bond renderer's input.
    const street = view.relations.get(
      "00000000000000000000000000001000--GROUNDS-->00000000000000000000000000002000"
    );
    expect(street?.visual.hierarchy).toBeCloseTo(0.45, 6);
    expect(street?.visual.polarity?.[0]).toBeCloseTo(0.85, 6);
    expect(street?.visual.polarity?.[1]).toBeCloseTo(0.55, 6);
  });

  it("carries the hover presentation facet the client recovers", () => {
    const { frames } = cityToFrames(city, 1);
    const entityFrame = frames.find(
      (f) => f.payload.message_type === "entity_materialized"
    );
    const recovered = entityPresentationFromFrame(entityFrame);
    expect(recovered?.id).toBe("00000000000000000000000000001000");
    expect(recovered?.presentation.label).toBe("actor");
    expect(recovered?.presentation.detail).toBe("ontology_definition");
    expect(recovered?.presentation.state).toBe("active");
    expect(recovered?.presentation.epistemic).toBe("unknown");

    const relationFrame = frames.find(
      (f) => f.payload.message_type === "relation_materialized"
    );
    const street = relationPresentationFromFrame(relationFrame);
    expect(street?.presentation.predicate).toBe("GROUNDS");
    expect(street?.presentation.label).toBeNull();

    // The geometry decoder ignores the presentation facet entirely.
    expect(entityPresentationFromFrame(relationFrame)).toBeNull();
    expect(relationPresentationFromFrame(entityFrame)).toBeNull();
  });

  it("keeps sequence monotonic across successive store-change batches", () => {
    const first = cityToFrames(city, 1);
    const second = cityToFrames(city, first.nextSeq);
    // A fresh snapshot frame resets the client view regardless of the gap, so the
    // second batch simply continues the counter.
    expect(second.frames[0].sequence).toBe(first.nextSeq);
    expect(second.frames[0].payload.message_type).toBe("snapshot");
  });
});
