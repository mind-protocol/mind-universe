import {
  DESKTOP_PROTOCOL_VERSION,
  type EnergyTransfer,
  type EnergyTransferPrimitive,
  type EpistemicState,
  type UniverseEvent
} from "./contracts";

const VISUAL_MICROUNITS = 1_000_000;
const epistemicStates = new Set<EpistemicState>([
  "observed",
  "measured",
  "known_absent",
  "unknown",
  "not_measured",
  "measurement_failed"
]);
const primitives = new Set<EnergyTransferPrimitive>([
  "energy_packet",
  "inhibitory_wave",
  "rupture"
]);

type JsonObject = Record<string, unknown>;

function object(value: unknown): JsonObject | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonObject)
    : null;
}

function string(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function safeInteger(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : null;
}

function oneOf<T extends string>(value: unknown, allowed: Set<T>): T | null {
  return typeof value === "string" && allowed.has(value as T)
    ? (value as T)
    : null;
}

export function universeEventFromServerFrame(
  input: unknown
): UniverseEvent | null {
  const frame = object(input);
  if (!frame || frame.protocol_version !== DESKTOP_PROTOCOL_VERSION) return null;
  const sequence = safeInteger(frame.sequence);
  const payload = object(frame.payload);
  if (sequence === null || sequence === 0 || !payload) return null;

  if (payload.message_type === "snapshot") {
    const revision = safeInteger(payload.revision);
    if (revision === null) return null;
    return {
      version: DESKTOP_PROTOCOL_VERSION,
      sequence,
      kind: "snapshot_started",
      revision
    };
  }

  if (payload.message_type !== "energy_transfer") return null;
  const visual = object(payload.visual);
  if (!visual) return null;
  const primitive = oneOf(visual.primitive, primitives);
  const epistemic = oneOf(payload.epistemic, epistemicStates);
  const revision = safeInteger(payload.revision);
  const tick = safeInteger(payload.tick);
  const energy = safeInteger(payload.energy);
  const gate = safeInteger(payload.gate_microunits);
  const emissiveIntensity = safeInteger(
    visual.emissive_intensity_microunits
  );
  const radius = safeInteger(visual.radius_microunits);
  const opacity = safeInteger(visual.opacity_microunits);
  const durationMs = safeInteger(visual.duration_ms);
  const transferId = string(payload.transfer_id);
  const executionId = string(payload.execution_id);
  const intentionId = string(payload.intention_id);
  const relationId = string(payload.relation_id);
  const source = string(payload.source);
  const target = string(payload.target);
  const color = string(visual.color);
  const emissive = string(visual.emissive);
  const direction =
    payload.direction === "source_to_target" ||
    payload.direction === "target_to_source"
      ? payload.direction
      : null;
  const polarity =
    payload.polarity === "support" ||
    payload.polarity === "inhibit" ||
    payload.polarity === "neutral"
      ? payload.polarity
      : null;
  const outcome =
    payload.outcome === "measured" || payload.outcome === "rejected"
      ? payload.outcome
      : null;

  if (
    revision === null ||
    tick === null ||
    energy === null ||
    gate === null ||
    emissiveIntensity === null ||
    radius === null ||
    opacity === null ||
    durationMs === null ||
    !transferId ||
    !executionId ||
    !intentionId ||
    !relationId ||
    !source ||
    !target ||
    !color ||
    !emissive ||
    !direction ||
    !polarity ||
    !outcome ||
    !epistemic ||
    !primitive
  ) {
    return null;
  }

  const transfer: EnergyTransfer = {
    transferId,
    executionId,
    intentionId,
    revision,
    tick,
    relationId,
    source,
    target,
    direction,
    polarity,
    energy,
    gate: gate / VISUAL_MICROUNITS,
    outcome,
    epistemic,
    visual: {
      primitive,
      color,
      emissive,
      emissiveIntensity: emissiveIntensity / VISUAL_MICROUNITS,
      radius: radius / VISUAL_MICROUNITS,
      opacity: opacity / VISUAL_MICROUNITS,
      durationMs
    }
  };
  return {
    version: DESKTOP_PROTOCOL_VERSION,
    sequence,
    kind: "energy_transferred",
    transfer
  };
}
