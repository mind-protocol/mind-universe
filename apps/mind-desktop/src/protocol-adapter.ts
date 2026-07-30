import {
  DESKTOP_PROTOCOL_VERSION,
  type EmbodimentMotionProfile,
  type EnergyTransfer,
  type EnergyTransferPrimitive,
  type EntityEmbodiment,
  type EntityMotionPrimitive,
  type EntityVisualPrimitive,
  type EpistemicState,
  type MaterializedEntity,
  type MaterializedRelation,
  type PhysicalResidency,
  type UniverseEvent,
  type Vector3,
  type VisualEmbodimentMapping,
  type VisualMaterial
} from "./contracts";
import { validateEmbodimentMapping } from "./embodiment";

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

const entityPrimitives = new Set<EntityVisualPrimitive>([
  "pulsing_core",
  "open_polyhedral_attractor",
  "oriented_ring",
  "bounded_volume",
  "faceted_router",
  "slab",
  "torus_knot",
  "cylinder",
  "tetrahedron",
  "unknown"
]);
const entityMotions = new Set<EntityMotionPrimitive>([
  "outward_pulse",
  "inward_orbit",
  "through_flow",
  "boundary_breath",
  "port_transform",
  "still"
]);
const relationPrimitives = new Set<
  MaterializedRelation["visual"]["primitive"]
>(["dual_lane_bond", "luminous_chain", "navigable_path", "unknown"]);
const physicalResidencies = new Set<PhysicalResidency>([
  "hot",
  "sleeping",
  "aggregated",
  "dormant"
]);

// Resolves the optional embodiment a projector attaches when it read the
// entity's visual mapping from graph authority. The mapping must satisfy the
// renderer's own validator, or the embodiment is dropped (the entity still
// renders with its base visual) — an invalid authority is never trusted.
function embodimentFromFrame(value: unknown): EntityEmbodiment | undefined {
  const raw = object(value);
  if (!raw) return undefined;
  const sourceMappingId = string(raw.source_mapping_id);
  const residency = oneOf(raw.residency, physicalResidencies);
  const sampledAtMs = safeInteger(raw.sampled_at_ms);
  const motionProfile = object(raw.motion_profile);
  const mapping = object(raw.mapping);
  if (!sourceMappingId || !residency || sampledAtMs === null || !motionProfile || !mapping) {
    return undefined;
  }
  const typedMapping = mapping as unknown as VisualEmbodimentMapping;
  if (!validateEmbodimentMapping(typedMapping)) return undefined;
  return {
    source_mapping_id: sourceMappingId,
    mapping: typedMapping,
    motion_profile: motionProfile as unknown as EmbodimentMotionProfile,
    residency,
    sampled_at_ms: sampledAtMs
  };
}

// Position components are a layout, so they may be negative — unlike the
// non-negative visual microunit fields parsed with safeInteger.
function signedInteger(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) ? value : null;
}

function vector3FromMicro(value: unknown): Vector3 | null {
  if (!Array.isArray(value) || value.length !== 3) return null;
  const components = value.map(signedInteger);
  if (components.some((component) => component === null)) return null;
  return [
    (components[0] as number) / VISUAL_MICROUNITS,
    (components[1] as number) / VISUAL_MICROUNITS,
    (components[2] as number) / VISUAL_MICROUNITS
  ];
}

function materialFromMicro(value: unknown): VisualMaterial | null {
  const material = object(value);
  if (!material) return null;
  const color = string(material.color);
  const emissive = string(material.emissive);
  const emissiveIntensity = safeInteger(material.emissive_intensity_micro);
  const opacity = safeInteger(material.opacity_micro);
  const scale =
    material.scale_micro === undefined
      ? VISUAL_MICROUNITS
      : safeInteger(material.scale_micro);
  if (!color || !emissive || emissiveIntensity === null || opacity === null || scale === null) {
    return null;
  }
  return {
    color,
    emissive,
    emissiveIntensity: emissiveIntensity / VISUAL_MICROUNITS,
    opacity: opacity / VISUAL_MICROUNITS,
    scale: scale / VISUAL_MICROUNITS
  };
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

  if (payload.message_type === "entity_materialized") {
    const raw = object(payload.entity);
    if (!raw) return null;
    const id = string(raw.id);
    const generation = safeInteger(raw.generation);
    const position = vector3FromMicro(raw.position_micro);
    const visualRaw = object(raw.visual);
    if (id === null || generation === null || !position || !visualRaw) return null;
    const material = materialFromMicro(visualRaw.material);
    if (!material) return null;
    // A projection that declares no canonical visual falls back to the honest
    // `unknown` primitive / `still` motion rather than being rejected.
    const primitive = oneOf(visualRaw.primitive, entityPrimitives) ?? "unknown";
    const motion = oneOf(visualRaw.motion, entityMotions) ?? "still";
    const embodiment = embodimentFromFrame(raw.embodiment);
    const entity: MaterializedEntity = {
      id,
      generation,
      position,
      visual: { primitive, motion, material },
      ...(embodiment ? { embodiment } : {})
    };
    return {
      version: DESKTOP_PROTOCOL_VERSION,
      sequence,
      kind: "entity_materialized",
      entity
    };
  }

  if (payload.message_type === "relation_materialized") {
    const raw = object(payload.relation);
    if (!raw) return null;
    const id = string(raw.id);
    const source = string(raw.source);
    const target = string(raw.target);
    const visualRaw = object(raw.visual);
    if (id === null || source === null || target === null || !visualRaw) return null;
    const color = string(visualRaw.color);
    const emissive = string(visualRaw.emissive);
    const emissiveIntensity = safeInteger(visualRaw.emissive_intensity_micro);
    const opacity = safeInteger(visualRaw.opacity_micro);
    const width = safeInteger(visualRaw.width_micro);
    const laneSeparation = safeInteger(visualRaw.lane_separation_micro);
    if (
      !color ||
      !emissive ||
      emissiveIntensity === null ||
      opacity === null ||
      width === null ||
      laneSeparation === null
    ) {
      return null;
    }
    const primitive = oneOf(visualRaw.primitive, relationPrimitives) ?? "unknown";
    const relation: MaterializedRelation = {
      id,
      source,
      target,
      visual: {
        primitive,
        material: {
          color,
          emissive,
          emissiveIntensity: emissiveIntensity / VISUAL_MICROUNITS,
          opacity: opacity / VISUAL_MICROUNITS,
          scale: 1
        },
        width: width / VISUAL_MICROUNITS,
        laneSeparation: laneSeparation / VISUAL_MICROUNITS
      }
    };
    return {
      version: DESKTOP_PROTOCOL_VERSION,
      sequence,
      kind: "relation_materialized",
      relation
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
