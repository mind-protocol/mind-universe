// City projection -> desktop protocol frames (the wire the SSE dev-bridge speaks).
//
// The frame shape here is EXACTLY what `src/protocol-adapter.ts`
// (`universeEventFromServerFrame`) decodes and what the native universe-server /
// Tauri path emits — micro-encoded integers, `message_type` payloads, contiguous
// sequence numbers. Keeping one shape means graduating from this dev bridge to the
// production stream is a transport swap, not a reformat.

export const PROTOCOL_VERSION = 0;
const MICRO = 1_000_000;
const micro = (value) => Math.round(value * MICRO);

function entityFrame(entity, sequence) {
  return {
    protocol_version: PROTOCOL_VERSION,
    sequence,
    payload: {
      message_type: "entity_materialized",
      entity: {
        id: entity.id,
        generation: 0,
        // x/z from the layout; y seeds at 0 because the renderer re-grounds every
        // node onto the terrain relief (settleOnGround) regardless of seed height.
        position_micro: [micro(entity.x), 0, micro(entity.z)],
        visual: {
          primitive: entity.primitive,
          motion: entity.motion,
          material: {
            color: entity.color,
            emissive: entity.emissive,
            emissive_intensity_micro: micro(entity.emissiveIntensity),
            opacity_micro: micro(entity.opacity),
            scale_micro: micro(entity.scale)
          }
        },
        // Presentation facet (hover): label + detail + epistemic + state. The
        // geometry decoder (protocol-adapter) ignores these; the client's
        // presentation extractor (stream-presentation) reads them so the streamed
        // city has the same tooltips as the baked fixture.
        label: entity.label,
        detail: entity.detail,
        state: entity.state,
        epistemic: entity.epistemic
      }
    }
  };
}

function relationFrame(relation, sequence) {
  // A benign structural default; the Bond renderer recolours each street from the
  // physical_profile channels below, so this material is mostly a fallback.
  const visual = {
    primitive: "dual_lane_bond",
    color: "#7890b5",
    emissive: "#32496a",
    emissive_intensity_micro: micro(0.3),
    opacity_micro: micro(0.45),
    width_micro: micro(0.7),
    lane_separation_micro: micro(0.04)
  };
  // Carry the physical_profile channels (ALIGN §2) when the predicate resolves one.
  // A predicate with no profile stays a neutral, uncalibrated bond (honest fog).
  const physics = relation.physics;
  if (physics) {
    visual.hierarchy_micro = micro(physics.hierarchy);
    visual.polarity_micro = [micro(physics.polarity[0]), micro(physics.polarity[1])];
  }
  return {
    protocol_version: PROTOCOL_VERSION,
    sequence,
    payload: {
      message_type: "relation_materialized",
      relation: {
        id: relation.id,
        source: relation.source,
        target: relation.target,
        // The exact graph predicate — kept for provenance and street classification
        // (the geometry decoder ignores it; the presentation extractor reads it).
        predicate: relation.predicate,
        visual
      }
    }
  };
}

/**
 * A full frame batch for one city snapshot, starting at `startSeq` (>0). The
 * first frame is `snapshot` (resets the client view); entities then relations
 * follow with CONTIGUOUS sequence numbers, satisfying applyUniverseEvent's
 * gap rule. Returns { frames, nextSeq } so a live bridge can keep the counter
 * monotonic across successive store-change batches.
 */
export function cityToFrames(city, startSeq = 1) {
  let seq = startSeq;
  const frames = [
    {
      protocol_version: PROTOCOL_VERSION,
      sequence: seq++,
      payload: { message_type: "snapshot", revision: city.revision ?? 0 }
    }
  ];
  for (const entity of city.entities) frames.push(entityFrame(entity, seq++));
  for (const relation of city.relations) frames.push(relationFrame(relation, seq++));
  return { frames, nextSeq: seq };
}
