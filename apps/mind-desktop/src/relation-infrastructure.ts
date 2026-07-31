// Bonds are traversable infrastructure, never generic lines
// (space:mind-universe:ontology3d:v1 · contract visual-truth forbids generic
// lines; mapping relation-families assigns each predicate an infrastructure form).
//
// This module is the honest predicate -> family resolver plus the ground-draped
// routing used to lay streets between building footprints. A predicate the graph
// has not taught us maps to `unknown` and is drawn as a foggy, unclassified route —
// we never assert a family we cannot justify.

import { terrainHeight } from "./terrain";
import type { Vector3 } from "./contracts";

export type RelationFamily =
  | "attention"
  | "causality"
  | "constraint"
  | "containment"
  | "decision_work"
  | "evidence"
  | "flow"
  | "foundation"
  | "lifecycle"
  | "validation"
  | "unknown";

// Predicate -> family. Seeded from the canonical examples in
// mapping:mind-universe:ontology3d:v1:relation-families, extended with the
// fixture's own predicates classified against the nearest canonical intent.
// Fixture predicates are annotated with the canonical member they mirror.
const PREDICATE_FAMILY: Readonly<Record<string, RelationFamily>> = {
  // Canonical examples from the relation-families mapping.
  CAUSES: "causality",
  LEADS_TO: "causality",
  PRODUCES: "causality",
  UNLOCKS: "causality",
  BLOCKS: "constraint",
  INHIBITS: "constraint",
  LIMITS: "constraint",
  MITIGATES: "constraint",
  PART_OF: "containment",
  SUBCASE_OF: "containment",
  APPLIES_IN: "containment",
  INSTANCE_OF: "containment",
  MOTIVATES: "decision_work",
  ADDRESSES: "decision_work",
  OPTION_FOR: "decision_work",
  DEPENDS_ON: "decision_work",
  DERIVED_FROM: "evidence",
  OBSERVES: "evidence",
  SUPPORTS_ESTIMATE: "evidence",
  CONTRADICTS: "evidence",
  FEEDS: "flow",
  COMMUNICATES: "flow",
  GROUNDS: "foundation",
  DEFINES: "foundation",
  SAFEGUARDS: "foundation",
  IMPLEMENTS: "foundation",
  MATCHES: "lifecycle",
  REINFORCES: "lifecycle",
  WEAKENS: "lifecycle",
  SUPERSEDES: "lifecycle",
  COMPILES_TO: "lifecycle",
  MAKES_SALIENT: "attention",
  RECRUITS: "attention",
  EXPLAINS: "attention",
  QUESTIONS: "attention",
  COMMENTED_ON: "attention",
  TESTS: "validation",
  AUDITS: "validation",
  MEASURES: "validation",
  MEASURED_BY: "validation",
  // Fixture predicates, each aligned to the canonical member it mirrors.
  IN_BATCH: "containment", // mirrors PART_OF: an asset belongs to a batch
  HAS_RECEIPT: "evidence", // a receipt is provenance
  IMPORTS_FROM: "flow", // data fed in from the source
  GOVERNED_BY: "foundation", // governance defines/safeguards
  USES_ONTOLOGY_MAPPING: "decision_work", // mirrors DEPENDS_ON
  USES_CODE_STRATEGY: "decision_work", // mirrors DEPENDS_ON
  MAPS_TO: "lifecycle" // a correspondence, mirrors MATCHES
};

export function relationFamily(predicate: string | undefined): RelationFamily {
  if (!predicate) return "unknown";
  return PREDICATE_FAMILY[predicate.toUpperCase()] ?? "unknown";
}

export interface InfrastructureStyle {
  readonly family: RelationFamily;
  readonly color: string;
  readonly width: number;
  readonly dashed: boolean;
  // The city form this family takes, per the relation-families mapping.
  readonly form: string;
}

const FAMILY_STYLE: Readonly<Record<RelationFamily, Omit<InfrastructureStyle, "family">>> = {
  causality: { color: "#e0a24a", width: 3, dashed: false, form: "chaussée directionnelle" },
  flow: { color: "#3fb7c9", width: 2.4, dashed: true, form: "canal / conduit" },
  evidence: { color: "#8fd6ff", width: 1.7, dashed: false, form: "faisceau de provenance" },
  containment: { color: "#6f7fae", width: 4.5, dashed: false, form: "zone / imbrication" },
  foundation: { color: "#c2a86a", width: 3.2, dashed: false, form: "fondation / pilier" },
  decision_work: { color: "#c68adf", width: 2.6, dashed: true, form: "route branchée" },
  lifecycle: { color: "#66c07a", width: 2.6, dashed: false, form: "cycle de vie" },
  constraint: { color: "#d86a5a", width: 3, dashed: false, form: "barrière / porte" },
  attention: { color: "#f0e68c", width: 1.8, dashed: true, form: "halo / projecteur" },
  validation: { color: "#7fe0b0", width: 2, dashed: false, form: "circuit de validation" },
  unknown: { color: "#5a616e", width: 1.6, dashed: true, form: "route (famille inconnue)" }
};

export function infrastructureStyle(predicate: string | undefined): InfrastructureStyle {
  const family = relationFamily(predicate);
  return { family, ...FAMILY_STYLE[family] };
}

/**
 * A street that follows the land between two building footprints. Points are
 * sampled along the horizontal line from `a` to `b` and lifted to sit just above
 * the terrain beneath them, with a shallow crown at mid-span for legibility. The
 * result reads as a route across the city rather than a wire between floating nodes.
 */
export function drapedRoute(
  a: Vector3,
  b: Vector3,
  samples = 14,
  clearance = 0.06
): Vector3[] {
  const points: Vector3[] = [];
  const count = Math.max(2, samples);
  for (let index = 0; index <= count; index += 1) {
    const t = index / count;
    const x = a[0] + (b[0] - a[0]) * t;
    const z = a[2] + (b[2] - a[2]) * t;
    const crown = Math.sin(t * Math.PI) * 0.15;
    points.push([x, terrainHeight(x, z) + clearance + crown, z]);
  }
  return points;
}

// ---------------------------------------------------------------------------
// Canonical projection (ALIGN.md §2 — the single table).
//
// A bond is NOT styled by an invented family taxonomy. Each canonical attribute
// of the predicate's physical_profile drives one orthogonal perceptual channel.
// This is the debt-free path; `infrastructureStyle` above remains only as the
// fallback for sources that do not yet carry a profile.

// The canonical physical_profile of a predicate (subset the renderer consumes).
export interface PhysicalProfile {
  readonly family: string;
  readonly polarity: readonly [number, number]; // [p_ab, p_ba] in [-1,1]
  readonly hierarchy: number; // [-1,1] : +1 source below target, -1 above
  readonly permanence: number; // [0,1] : wire → cable → beam → arch
  readonly mode: string;
  readonly calibrated: boolean;
}

// Orthogonal channels the bond renderer sweeps into geometry/material.
export interface BondChannels {
  readonly family: string;
  readonly lightColor: string; // polarity sign → excitation / inhibition
  readonly slope: number; // hierarchy → conduit pitch (do not flatten)
  readonly radius: number; // permanence → material thickness
  readonly oneWay: boolean; // polarity asymmetry → direction
  readonly calibrated: boolean; // uncalibrated ⇒ honest fog downstream
}

// polarity mean sign → light colour: + excitation (cyan), − inhibition (red),
// ~0 neutral (slate). This is the canonical §2 rule, not a per-family palette.
export function projectPhysicalProfile(profile: PhysicalProfile): BondChannels {
  const mean = (profile.polarity[0] + profile.polarity[1]) / 2;
  const lightColor =
    mean >= 0.08 ? "#57c8ff" : mean <= -0.08 ? "#e0655f" : "#9aa6c0";
  const asymmetry = Math.abs(
    Math.abs(profile.polarity[0]) - Math.abs(profile.polarity[1])
  );
  return {
    family: profile.family,
    lightColor,
    slope: Math.max(-1, Math.min(1, profile.hierarchy)),
    radius: 0.022 + Math.max(0, Math.min(1, profile.permanence)) * 0.05,
    oneWay: asymmetry >= 0.2,
    calibrated: profile.calibrated
  };
}
