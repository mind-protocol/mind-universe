// ALIGN.md §2/§4 — the two carriers of the canonical link channels must converge.
//
// A bond can learn its physical_profile from EITHER a fixture projection
// (presentation.physics, a full profile) or the live wire (relation.visual
// .polarity/.hierarchy, emitted by desktop_world_snapshot and parsed by the
// protocol adapter). The World Bond now feeds both through the SAME
// projectPhysicalProfile table, so the same predicate looks identical — same
// light colour, same slope — regardless of which carrier delivered it. The wire
// omits permanence and any calibration signal, so it renders a neutral thickness
// and stays honestly uncalibrated (fainter) rather than asserting calibration.
//
// This test locks that convergence (and the honest divergence on calibration).
// It reconstructs exactly the profile the Bond synthesises from wire fields.

import { describe, expect, it } from "vitest";
import {
  projectPhysicalProfile,
  type PhysicalProfile
} from "./relation-infrastructure";

// The synthesis the Bond applies to wire-carried fields (World.tsx): only
// polarity + hierarchy are known; the rest are honest neutrals.
function fromWire(
  polarity: readonly [number, number],
  hierarchy: number
): ReturnType<typeof projectPhysicalProfile> {
  return projectPhysicalProfile({
    family: "unknown",
    polarity,
    hierarchy,
    permanence: 0.5,
    mode: "axis",
    calibrated: false
  });
}

describe("canonical link channels converge across carriers", () => {
  it("wire and fixture agree on colour + slope for an excitation predicate", () => {
    const polarity: readonly [number, number] = [0.9, 0.5];
    const hierarchy = 0.6;
    const fixture: PhysicalProfile = {
      family: "normative",
      polarity,
      hierarchy,
      permanence: 0.8,
      mode: "composite",
      calibrated: true
    };
    const fromFixture = projectPhysicalProfile(fixture);
    const wire = fromWire(polarity, hierarchy);

    // The visible identity of the link — colour and slope — is carrier-independent.
    expect(wire.lightColor).toBe(fromFixture.lightColor);
    expect(wire.slope).toBe(fromFixture.slope);
    // …and it is the excitation colour, not a per-family palette.
    expect(wire.lightColor).toBe("#57c8ff");

    // The honest divergence: a wire bond cannot claim calibration.
    expect(fromFixture.calibrated).toBe(true);
    expect(wire.calibrated).toBe(false);
  });

  it("carries inhibition colour from a negative-polarity wire bond", () => {
    expect(fromWire([-0.8, -0.4], 0).lightColor).toBe("#e0655f");
  });

  it("stays neutral colour when the wire polarity is ~0 (honest, not faked)", () => {
    expect(fromWire([0, 0], 0.5).lightColor).toBe("#9aa6c0");
  });

  it("takes the slope straight from wire hierarchy, clamped to [-1,1]", () => {
    expect(fromWire([0.3, 0.3], 0.6).slope).toBe(0.6);
    expect(fromWire([0.3, 0.3], -2).slope).toBe(-1);
  });
});
