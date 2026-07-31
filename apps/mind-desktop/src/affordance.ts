// S3 — the affordance face, perception half.
//
// CLAUDE.md ("Observation and affordance contract") requires a runtime
// observation to expose `available_actions: Vec<AffordanceInstance>` that are
// "local, typed, permission-aware, bounded, linked to exact targets and ports,
// explicit about preconditions, explicit about expected semantic and physical
// consequences" and that "must include or resolve to a justification."
//
// This module is the PERCEPTION half: the typed descriptor an observation
// carries, plus a pure, deterministic derivation that decides — honestly —
// which affordances a given target may offer. It emits gestures; it never
// mutates the Universe. Committing an intent is the action half (out of scope).
//
// Honesty is the core. A node the world cannot vouch for is NEVER a confident
// button. Fog stays fog: an `unknown` / `not_measured` / `measurement_failed`
// target (or an unresolved precondition) yields a `fogged` affordance with a
// stated reason — never `"actionable"`. A missing justification is not a valid
// instance (we never fabricate one). "Nothing to do here" is a valid, total
// answer: a DEFINED empty array, never `undefined`, never a throw.

import type { EntityId, EpistemicState } from "./contracts";

// A CLOSED union of typed verbs. Adding a verb is a deliberate ontology change,
// not an accident of a free-text string.
export type AffordanceKind =
  | "inspect"
  | "place"
  | "connect"
  | "open"
  | "build"
  | "test";

// The honesty state of an offered affordance.
// - `actionable`: the world vouches for the target and the actor is permitted.
// - `fogged`:     the world cannot vouch for the target/preconditions yet.
// - `forbidden`:  the actor lacks the required capability.
export type AffordanceAvailability = "actionable" | "fogged" | "forbidden";

// The structured SemanticIntent this gesture will produce — the "expected
// observation" a caller later compares against real readback. A typed object,
// deliberately NOT free text, so the expectation can be checked field by field.
export interface ExpectedSemanticEffect {
  // The SemanticIntent verb/kind this gesture compiles to (transport-level name).
  readonly intent: string;
  // The exact node the intent targets.
  readonly target: EntityId;
  // The exact port the intent touches, when the gesture is port-scoped.
  readonly port?: string;
  // The predicates/fields the committed intent is expected to write. Empty for
  // a read-only gesture such as `inspect`.
  readonly writes: readonly string[];
}

// A bounded budget for the gesture. Every field is optional but the struct is
// required — an affordance is always explicit about being bounded, even if the
// bound is "no extra fuel / radius declared".
export interface AffordanceBounds {
  readonly fuel?: number;
  readonly radius?: number;
}

export interface AffordanceInstance {
  readonly kind: AffordanceKind;
  // A stable node handle. Not a position, not a renderer id.
  readonly target: EntityId;
  readonly port?: string;
  // Explicit preconditions, named. Local and inspectable.
  readonly preconditions: readonly string[];
  readonly expectedSemanticEffect: ExpectedSemanticEffect;
  readonly expectedPhysicalFeedback?: string;
  // The permission/capability required to legitimately perform the gesture.
  readonly capability: string;
  readonly bounds: AffordanceBounds;
  // REQUIRED, non-empty. Why this physical gesture is a legitimate
  // interpretation of the semantic operation. An instance without it is invalid.
  readonly justification: string;
  readonly availability: AffordanceAvailability;
  // Why the affordance is fogged/forbidden. REQUIRED whenever availability is
  // not "actionable"; absent when actionable.
  readonly reason?: string;
}

// What an author declares BEFORE the honesty gate runs. It carries everything
// except the honesty verdict (`availability`/`reason`), which the derivation
// decides from the target's epistemic context. `target` is supplied by the
// context, so a template stays reusable across nodes.
export interface AffordanceTemplate {
  readonly kind: AffordanceKind;
  readonly port?: string;
  readonly preconditions: readonly string[];
  // Same as ExpectedSemanticEffect but without `target` (bound at derivation).
  readonly expectedSemanticEffect: Omit<ExpectedSemanticEffect, "target">;
  readonly expectedPhysicalFeedback?: string;
  readonly capability: string;
  readonly bounds: AffordanceBounds;
  readonly justification: string;
}

// The epistemic context of one target, as the observation knows it.
export interface AffordanceTarget {
  readonly id: EntityId;
  // What the world can vouch for about this target.
  readonly epistemic: EpistemicState;
  // Capabilities the acting Observer/Actor currently holds. A template whose
  // capability is absent here is `forbidden`.
  readonly grantedCapabilities: readonly string[];
  // Preconditions the world could NOT resolve (unmet or unmeasured). If a
  // template lists any of these, it is fogged even when the target itself is
  // measured — a precondition we cannot vouch for is fog, not a green button.
  readonly unresolvedPreconditions?: readonly string[];
}

// The two epistemic states the world can positively vouch for. Everything else
// is either fog (uncertainty) or known-absence.
const CONFIDENT_EPISTEMIC: ReadonlySet<EpistemicState> = new Set<EpistemicState>([
  "observed",
  "measured"
]);

// The fog states: the world does not (yet) know enough to vouch for the target.
const FOG_EPISTEMIC: ReadonlySet<EpistemicState> = new Set<EpistemicState>([
  "unknown",
  "not_measured",
  "measurement_failed"
]);

export function isConfidentEpistemic(state: EpistemicState): boolean {
  return CONFIDENT_EPISTEMIC.has(state);
}

export function isFogEpistemic(state: EpistemicState): boolean {
  return FOG_EPISTEMIC.has(state);
}

// A defensive validator, mirroring `isRenderableEnergyTransfer`: proves an
// instance is well-formed before a consumer trusts it. Justification must be
// non-empty; a non-actionable instance must carry a reason; an actionable one
// must not pretend to have a fog/forbid reason.
export function isValidAffordanceInstance(
  instance: AffordanceInstance
): boolean {
  if (instance.target.length === 0) return false;
  if (instance.capability.length === 0) return false;
  if (instance.justification.trim().length === 0) return false;
  if (instance.expectedSemanticEffect.target !== instance.target) return false;
  if (instance.availability === "actionable") {
    return instance.reason === undefined;
  }
  return typeof instance.reason === "string" && instance.reason.length > 0;
}

// Decides the honesty verdict for one template against a target. Precedence:
//   1. capability not granted        → forbidden   (permission is the outer gate)
//   2. target is fog OR a listed precondition is unresolved → fogged
//   3. target is confident + permitted → actionable
// This ordering keeps the load-bearing invariant: a fog target can never become
// `actionable`, because step 2 is reached only when step 1 did not forbid, and
// step 3 requires a confident epistemic state.
function verdict(
  template: AffordanceTemplate,
  target: AffordanceTarget
): { availability: AffordanceAvailability; reason?: string } {
  if (!target.grantedCapabilities.includes(template.capability)) {
    return {
      availability: "forbidden",
      reason: `capability not granted: ${template.capability}`
    };
  }

  const unresolved = target.unresolvedPreconditions ?? [];
  const blockingPrecondition = template.preconditions.find((p) =>
    unresolved.includes(p)
  );
  if (blockingPrecondition !== undefined) {
    return {
      availability: "fogged",
      reason: `precondition not vouched: ${blockingPrecondition}`
    };
  }

  if (isFogEpistemic(target.epistemic)) {
    return {
      availability: "fogged",
      reason: `target epistemic is ${target.epistemic}: the world cannot vouch for it`
    };
  }

  if (isConfidentEpistemic(target.epistemic)) {
    return { availability: "actionable" };
  }

  // `known_absent` reaches here — handled by the caller as a drop, so this is a
  // defensive last resort, kept honest rather than defaulting to actionable.
  return {
    availability: "fogged",
    reason: `target epistemic is ${target.epistemic}: not an actionable ground`
  };
}

// Pure, deterministic derivation. Given a target's epistemic context and a set
// of candidate templates, returns the offered affordances — exactly one defined
// array per call, never `undefined`, never a throw.
//
// Drop rules (a dropped template is honest silence, never a fabricated button):
//   - a template with an empty/whitespace justification is not a valid instance;
//   - when the target is `known_absent`, no affordance applies (the node is
//     positively not there) — every template drops, yielding a defined `[]`.
// Every surviving template yields exactly one instance, in template order
// (stable — no clock, no randomness).
export function deriveAvailableActions(
  target: AffordanceTarget,
  templates: readonly AffordanceTemplate[]
): readonly AffordanceInstance[] {
  // A target the world positively knows is absent offers nothing to do. This is
  // a defined, total "nothing here" — distinct from fog (uncertainty).
  if (target.epistemic === "known_absent") return [];

  const offered: AffordanceInstance[] = [];
  for (const template of templates) {
    // Never fabricate a justification: a template without one cannot become a
    // valid instance, so it is dropped rather than emitted with a hole.
    if (template.justification.trim().length === 0) continue;

    const { availability, reason } = verdict(template, target);
    const instance: AffordanceInstance = {
      kind: template.kind,
      target: target.id,
      ...(template.port !== undefined ? { port: template.port } : {}),
      preconditions: template.preconditions,
      expectedSemanticEffect: {
        ...template.expectedSemanticEffect,
        target: target.id
      },
      ...(template.expectedPhysicalFeedback !== undefined
        ? { expectedPhysicalFeedback: template.expectedPhysicalFeedback }
        : {}),
      capability: template.capability,
      bounds: template.bounds,
      justification: template.justification,
      availability,
      ...(reason !== undefined ? { reason } : {})
    };
    offered.push(instance);
  }
  return offered;
}
