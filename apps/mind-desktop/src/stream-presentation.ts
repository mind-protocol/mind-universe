// Presentation extractor for the live stream (hover facet).
//
// The geometry decoder (protocol-adapter) turns a frame into a MaterializedEntity
// and drops everything else. The hover tooltip needs the presentation facet the
// frame ALSO carries — label / detail / epistemic / state per node, and the
// predicate per street. This module recovers exactly that, so a streamed city has
// the same tooltips and street classification as the baked fixture. It validates
// leniently: a missing/!string field degrades to an honest default rather than
// dropping the node from the presentation map.

import type { EpistemicState } from "./contracts";
import type {
  EntityPresentation,
  RelationPresentation
} from "./postgres-pilot-fixture";

const EPISTEMIC_STATES = new Set<EpistemicState>([
  "observed",
  "measured",
  "known_absent",
  "unknown",
  "not_measured",
  "measurement_failed"
]);

type JsonObject = Record<string, unknown>;

function object(value: unknown): JsonObject | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonObject)
    : null;
}

function stringOr(value: unknown, fallback: string): string {
  return typeof value === "string" && value.length > 0 ? value : fallback;
}

function epistemicOr(value: unknown): EpistemicState {
  return typeof value === "string" && EPISTEMIC_STATES.has(value as EpistemicState)
    ? (value as EpistemicState)
    : "unknown";
}

/** Recovers the entity presentation from an `entity_materialized` frame, or null. */
export function entityPresentationFromFrame(
  input: unknown
): { readonly id: string; readonly presentation: EntityPresentation } | null {
  const frame = object(input);
  const payload = frame && object(frame.payload);
  if (!payload || payload.message_type !== "entity_materialized") return null;
  const entity = object(payload.entity);
  const id = entity && typeof entity.id === "string" ? entity.id : null;
  if (!entity || !id) return null;
  return {
    id,
    presentation: {
      label: stringOr(entity.label, id),
      detail: stringOr(entity.detail, ""),
      epistemic: epistemicOr(entity.epistemic),
      state: stringOr(entity.state, "")
    }
  };
}

/** Recovers the relation presentation from a `relation_materialized` frame, or null. */
export function relationPresentationFromFrame(
  input: unknown
): { readonly id: string; readonly presentation: RelationPresentation } | null {
  const frame = object(input);
  const payload = frame && object(frame.payload);
  if (!payload || payload.message_type !== "relation_materialized") return null;
  const relation = object(payload.relation);
  const id = relation && typeof relation.id === "string" ? relation.id : null;
  if (!relation || !id) return null;
  return {
    id,
    // Streets stay unlabelled (hundreds would be noise); the predicate is kept for
    // provenance and infrastructure-family classification.
    presentation: {
      label: null,
      predicate: stringOr(relation.predicate, "RELATED_TO")
    }
  };
}
