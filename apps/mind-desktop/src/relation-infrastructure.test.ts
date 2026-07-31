import { describe, expect, it } from "vitest";
import {
  drapedRoute,
  infrastructureStyle,
  projectPhysicalProfile,
  relationFamily,
  type PhysicalProfile
} from "./relation-infrastructure";
import { terrainHeight } from "./terrain";

describe("projectPhysicalProfile (canonical single table)", () => {
  const base: PhysicalProfile = {
    family: "enablement",
    polarity: [0.9, 0.1],
    hierarchy: 0.6,
    permanence: 0.8,
    mode: "composite",
    calibrated: false
  };

  it("maps polarity sign to light colour, not a per-family palette", () => {
    expect(projectPhysicalProfile({ ...base, polarity: [0.9, 0.5] }).lightColor).toBe(
      "#57c8ff"
    ); // excitation
    expect(projectPhysicalProfile({ ...base, polarity: [-0.8, -0.4] }).lightColor).toBe(
      "#e0655f"
    ); // inhibition
    expect(projectPhysicalProfile({ ...base, polarity: [0, 0] }).lightColor).toBe(
      "#9aa6c0"
    ); // neutral
  });

  it("carries hierarchy through as slope (no flattening)", () => {
    expect(projectPhysicalProfile({ ...base, hierarchy: 0.6 }).slope).toBe(0.6);
    expect(projectPhysicalProfile({ ...base, hierarchy: -1 }).slope).toBe(-1);
  });

  it("grows radius with permanence and flags asymmetric links one-way", () => {
    const thin = projectPhysicalProfile({ ...base, permanence: 0 }).radius;
    const thick = projectPhysicalProfile({ ...base, permanence: 1 }).radius;
    expect(thick).toBeGreaterThan(thin);
    expect(projectPhysicalProfile({ ...base, polarity: [0.9, 0.1] }).oneWay).toBe(true);
    expect(projectPhysicalProfile({ ...base, polarity: [0.5, 0.5] }).oneWay).toBe(false);
  });
});

describe("relationFamily", () => {
  it("classifies every predicate the fixture actually uses", () => {
    expect(relationFamily("MAPS_TO")).toBe("lifecycle");
    expect(relationFamily("IN_BATCH")).toBe("containment");
    expect(relationFamily("IMPORTS_FROM")).toBe("flow");
    expect(relationFamily("GOVERNED_BY")).toBe("foundation");
    expect(relationFamily("USES_ONTOLOGY_MAPPING")).toBe("decision_work");
    expect(relationFamily("USES_CODE_STRATEGY")).toBe("decision_work");
    expect(relationFamily("HAS_RECEIPT")).toBe("evidence");
  });

  it("resolves canonical examples from the relation-families mapping", () => {
    expect(relationFamily("CAUSES")).toBe("causality");
    expect(relationFamily("PART_OF")).toBe("containment");
    expect(relationFamily("BLOCKS")).toBe("constraint");
    expect(relationFamily("MEASURED_BY")).toBe("validation");
  });

  it("keeps unknown predicates unknown instead of guessing a family", () => {
    expect(relationFamily("WHATEVER_UNSEEN")).toBe("unknown");
    expect(relationFamily(undefined)).toBe("unknown");
    expect(infrastructureStyle("WHATEVER_UNSEEN").form).toContain("inconnue");
  });

  it("is case-insensitive on the predicate", () => {
    expect(relationFamily("maps_to")).toBe("lifecycle");
  });
});

describe("drapedRoute", () => {
  it("follows the terrain: every point sits just above the land beneath it", () => {
    const route = drapedRoute([12, 3, -4], [-9, 1, 8]);
    for (const [x, y, z] of route) {
      // y should be terrain height plus a small clearance/crown, never below land.
      expect(y).toBeGreaterThanOrEqual(terrainHeight(x, z));
      expect(y).toBeLessThan(terrainHeight(x, z) + 0.5);
    }
  });

  it("starts and ends over the two footprints", () => {
    const route = drapedRoute([12, 3, -4], [-9, 1, 8]);
    expect(route[0][0]).toBeCloseTo(12);
    expect(route[0][2]).toBeCloseTo(-4);
    expect(route[route.length - 1][0]).toBeCloseTo(-9);
    expect(route[route.length - 1][2]).toBeCloseTo(8);
  });
});
