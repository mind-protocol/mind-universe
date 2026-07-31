import { describe, expect, it } from "vitest";
import {
  deriveAvailableActions,
  isConfidentEpistemic,
  isFogEpistemic,
  isValidAffordanceInstance,
  type AffordanceInstance,
  type AffordanceTarget,
  type AffordanceTemplate
} from "./affordance";
import type { EpistemicState } from "./contracts";

const inspectTemplate: AffordanceTemplate = {
  kind: "inspect",
  preconditions: ["target-resolves"],
  expectedSemanticEffect: { intent: "inspect_node", writes: [] },
  expectedPhysicalFeedback: "the core brightens under focus",
  capability: "observe",
  bounds: { radius: 1 },
  justification:
    "inspecting reads the node's own manifest; a read gesture commits no write"
};

const connectTemplate: AffordanceTemplate = {
  kind: "connect",
  port: "north",
  preconditions: ["both-endpoints-measured"],
  expectedSemanticEffect: {
    intent: "put_relation",
    port: "north",
    writes: ["ENERGIZES"]
  },
  capability: "construct",
  bounds: { fuel: 4, radius: 2 },
  justification:
    "dragging a bond between two vouched beacons compiles to one PutRelation"
};

function targetWith(
  epistemic: EpistemicState,
  overrides: Partial<AffordanceTarget> = {}
): AffordanceTarget {
  return {
    id: "node:alpha",
    epistemic,
    grantedCapabilities: ["observe", "construct"],
    ...overrides
  };
}

describe("deriveAvailableActions — honesty gate", () => {
  it("(a) a measured target yields an actionable affordance with a non-empty justification", () => {
    const result = deriveAvailableActions(targetWith("measured"), [
      inspectTemplate
    ]);

    expect(result).toHaveLength(1);
    const [action] = result;
    expect(action.availability).toBe("actionable");
    expect(action.reason).toBeUndefined();
    expect(action.justification.trim().length).toBeGreaterThan(0);
    // linked to the exact target, and the expected effect names it too
    expect(action.target).toBe("node:alpha");
    expect(action.expectedSemanticEffect.target).toBe("node:alpha");
    expect(isValidAffordanceInstance(action)).toBe(true);
  });

  it("an observed target is also actionable (both vouched states)", () => {
    const [action] = deriveAvailableActions(targetWith("observed"), [
      inspectTemplate
    ]);
    expect(action.availability).toBe("actionable");
  });

  it("(b) an unknown target yields a fogged affordance with a reason and NEVER actionable", () => {
    const [action] = deriveAvailableActions(targetWith("unknown"), [
      inspectTemplate
    ]);
    expect(action.availability).toBe("fogged");
    expect(action.availability).not.toBe("actionable");
    expect(action.reason && action.reason.length).toBeGreaterThan(0);
    expect(isValidAffordanceInstance(action)).toBe(true);
  });

  it("(b) a not_measured target is fogged, never actionable", () => {
    const [action] = deriveAvailableActions(targetWith("not_measured"), [
      inspectTemplate
    ]);
    expect(action.availability).toBe("fogged");
    expect(action.reason).toContain("not_measured");
  });

  it("(b) a measurement_failed target is fogged, never actionable", () => {
    const [action] = deriveAvailableActions(targetWith("measurement_failed"), [
      inspectTemplate
    ]);
    expect(action.availability).toBe("fogged");
  });

  it("no fog state ever produces an actionable affordance", () => {
    const fogStates: EpistemicState[] = [
      "unknown",
      "not_measured",
      "measurement_failed"
    ];
    for (const state of fogStates) {
      const [action] = deriveAvailableActions(targetWith(state), [
        inspectTemplate,
        connectTemplate
      ]);
      expect(action.availability).not.toBe("actionable");
      expect(isFogEpistemic(state)).toBe(true);
      expect(isConfidentEpistemic(state)).toBe(false);
    }
  });

  it("an unresolved precondition fogs even a measured target (fog is not only about the node)", () => {
    const target = targetWith("measured", {
      unresolvedPreconditions: ["both-endpoints-measured"]
    });
    const [action] = deriveAvailableActions(target, [connectTemplate]);
    expect(action.availability).toBe("fogged");
    expect(action.reason).toContain("both-endpoints-measured");
  });

  it("a missing capability makes a measured target forbidden, with a reason", () => {
    const target = targetWith("measured", { grantedCapabilities: ["observe"] });
    const [action] = deriveAvailableActions(target, [connectTemplate]);
    expect(action.availability).toBe("forbidden");
    expect(action.reason).toContain("construct");
    expect(isValidAffordanceInstance(action)).toBe(true);
  });

  it("(c) a config with no legitimate action yields a DEFINED empty array (empty templates)", () => {
    const result = deriveAvailableActions(targetWith("measured"), []);
    expect(result).toEqual([]);
    expect(result).not.toBeUndefined();
  });

  it("(c) a known_absent target yields a defined empty array — nothing to do on an absent node", () => {
    const result = deriveAvailableActions(targetWith("known_absent"), [
      inspectTemplate,
      connectTemplate
    ]);
    expect(result).toEqual([]);
  });

  it("a template with an empty justification is dropped, never fabricated into a button", () => {
    const noJustification: AffordanceTemplate = {
      ...inspectTemplate,
      justification: "   "
    };
    const result = deriveAvailableActions(targetWith("measured"), [
      noJustification,
      connectTemplate
    ]);
    // only the well-formed template survives
    expect(result).toHaveLength(1);
    expect(result[0].kind).toBe("connect");
  });

  it("emits exactly one instance per surviving template, in template order", () => {
    const result = deriveAvailableActions(targetWith("measured"), [
      connectTemplate,
      inspectTemplate
    ]);
    expect(result.map((a) => a.kind)).toEqual(["connect", "inspect"]);
  });

  it("(d) determinism — same input yields byte-identical output", () => {
    const target = targetWith("measured", {
      unresolvedPreconditions: ["both-endpoints-measured"]
    });
    const templates = [inspectTemplate, connectTemplate];
    const first = deriveAvailableActions(target, templates);
    const second = deriveAvailableActions(target, templates);
    expect(JSON.stringify(first)).toBe(JSON.stringify(second));
  });
});

describe("isValidAffordanceInstance", () => {
  it("rejects an instance whose justification is blank", () => {
    const instance: AffordanceInstance = {
      kind: "inspect",
      target: "node:alpha",
      preconditions: [],
      expectedSemanticEffect: { intent: "inspect_node", target: "node:alpha", writes: [] },
      capability: "observe",
      bounds: {},
      justification: "  ",
      availability: "actionable"
    };
    expect(isValidAffordanceInstance(instance)).toBe(false);
  });

  it("rejects a non-actionable instance that carries no reason", () => {
    const instance: AffordanceInstance = {
      kind: "inspect",
      target: "node:alpha",
      preconditions: [],
      expectedSemanticEffect: { intent: "inspect_node", target: "node:alpha", writes: [] },
      capability: "observe",
      bounds: {},
      justification: "valid reason to inspect",
      availability: "fogged"
    };
    expect(isValidAffordanceInstance(instance)).toBe(false);
  });

  it("rejects an actionable instance that pretends to carry a fog reason", () => {
    const instance: AffordanceInstance = {
      kind: "inspect",
      target: "node:alpha",
      preconditions: [],
      expectedSemanticEffect: { intent: "inspect_node", target: "node:alpha", writes: [] },
      capability: "observe",
      bounds: {},
      justification: "valid reason to inspect",
      availability: "actionable",
      reason: "should not be here"
    };
    expect(isValidAffordanceInstance(instance)).toBe(false);
  });

  it("rejects an instance whose expected effect targets a different node", () => {
    const instance: AffordanceInstance = {
      kind: "inspect",
      target: "node:alpha",
      preconditions: [],
      expectedSemanticEffect: { intent: "inspect_node", target: "node:beta", writes: [] },
      capability: "observe",
      bounds: {},
      justification: "valid reason to inspect",
      availability: "actionable"
    };
    expect(isValidAffordanceInstance(instance)).toBe(false);
  });
});
