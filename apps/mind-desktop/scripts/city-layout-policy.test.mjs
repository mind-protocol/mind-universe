// Proves the data-sourced district assignment reproduces the former hardcoded
// districtOf() substring logic exactly. If they ever diverge, this fails.
//
// Part of the vitest suite (npm test). Run alone: npx vitest run scripts/city-layout-policy.test.mjs

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const policy = JSON.parse(
  readFileSync(resolve(here, "city-layout-policy.json"), "utf8")
);

// The data interpreter, mirrored from materialize-city.mjs (kept in sync by this
// test's whole purpose).
function districtForKind(kind = "") {
  const k = String(kind).toLowerCase();
  for (const rule of policy.rules ?? []) {
    if (
      (rule.includes ?? []).some((s) => k.includes(s)) ||
      (rule.startsWith ?? []).some((s) => k.startsWith(s)) ||
      (rule.equals ?? []).some((s) => k === s)
    )
      return rule.district;
  }
  return policy.default_district ?? "outskirts";
}

// The ORIGINAL hardcoded logic, verbatim from the pre-refactor materialize-city.mjs.
function districtOfReference(kind = "") {
  const k = kind.toLowerCase();
  if (k.includes("audio") || k.includes("acoustic")) return "acoustic";
  if (k.includes("physical_profile")) return "physics";
  if (k.startsWith("ontology_")) return "canon";
  if (k.includes("code")) return "code";
  if (k.includes("data_contract") || k.includes("schema")) return "schema";
  if (k.includes("visual_mapping") || k === "mapping" || k.includes("_mapping"))
    return "mapping";
  if (k.includes("contract")) return "contract";
  if (
    k.includes("validation") || k.includes("metric") || k.includes("health") ||
    k.includes("gap") || k.includes("task") || k.includes("receipt") ||
    k.includes("changeset") || k.includes("outcome") || k.includes("problem")
  )
    return "governance";
  if (
    k.includes("loop") || k.includes("objective") || k.includes("pattern") ||
    k.includes("policy") || k === "space"
  )
    return "civic";
  return "outskirts";
}

// Every kind observed in the live store, plus edge cases exercising each branch
// and its precedence (contract-before-governance, schema-before-mapping, etc.).
const cases = [
  "ontology_relation", "ontology_definition", "physical_profile",
  "ontology_contract", "built_position", "primitive", "placement_justification",
  "placement_construction", "ontology_source", "ontology_gap",
  "ground_change_receipt", "dynamic_state", "ontology_manifest",
  "ground_change_rejection", "derived_state", "derived_cache",
  "behaviour_interface",
  // edge cases
  "audio_loop", "acoustic_beacon", "code", "data_contract", "schema",
  "visual_mapping", "mapping", "predicate_mapping", "contract", "validation",
  "metric", "health", "gap", "task", "receipt", "changeset", "outcome",
  "problem", "loop", "objective", "pattern", "policy", "space", "",
  "totally_unknown_kind"
];

describe("city-layout-policy", () => {
  it("reproduces the former hardcoded districtOf() for every kind", () => {
    for (const kind of cases) {
      expect(districtForKind(kind), `district mismatch for kind "${kind}"`).toBe(
        districtOfReference(kind)
      );
    }
  });

  it("has a style for every district and no orphans", () => {
    for (const d of policy.district_order) {
      expect(policy.styles[d], `missing style for district "${d}"`).toBeTruthy();
    }
    expect(Object.keys(policy.styles).sort()).toEqual(
      [...policy.district_order].sort()
    );
  });
});
