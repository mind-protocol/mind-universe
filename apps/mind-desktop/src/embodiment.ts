import type {
  CitizenEnergyEmbodiment,
  EmbodimentMotionProfile,
  EmbodimentPrimitiveTuple,
  EntityEmbodiment,
  PhysicalResidency,
  RoleKeyedEmbodimentMapping,
  Vector3,
  VisualEmbodimentMapping
} from "./contracts";

const NATIVE_MAX_PRIMITIVES = 12;
const NATIVE_MAX_PARTICLES = 160;
const ALLOWED_PRIMITIVES = new Set([
  "icosphere",
  "sphere",
  "capsule",
  "points",
  "fresnel_shell",
  "box",
  "cylinder",
  "cone",
  "torus",
  "plane",
  "tube"
]);

export interface MotionSample {
  readonly velocity: Vector3;
  readonly speed: number;
}

export interface SpringState {
  readonly position: Vector3;
  readonly velocity: Vector3;
}

export function validateEmbodimentMapping(
  mapping: VisualEmbodimentMapping
): boolean {
  if (
    mapping.schema_version !== "visual-embodiment/1" ||
    mapping.primitive_budget < 1 ||
    mapping.primitive_budget > NATIVE_MAX_PRIMITIVES ||
    mapping.particle_budget < 0 ||
    mapping.particle_budget > NATIVE_MAX_PARTICLES ||
    !mapping.forms[mapping.fallback_form]
  ) {
    return false;
  }

  return Object.values(mapping.forms).every((form) => {
    if (form.length > mapping.primitive_budget) return false;
    let particles = 0;
    for (const primitive of form) {
      if (!ALLOWED_PRIMITIVES.has(primitive[0])) return false;
      if (
        !validVector(primitive[3]) ||
        !validVector(primitive[4]) ||
        !validScale(primitive[5])
      ) {
        return false;
      }
      if (primitive[0] === "points") particles += primitive[6];
    }
    return particles <= mapping.particle_budget;
  });
}

/**
 * Validates a toolkit's OWN role-keyed binding under the same native budgets. A
 * role-keyed mapping declares forms per archetype instead of per LOD, so the
 * LOD-keyed validator above rejects it — correctly, since that validator is also
 * the gate for the citizen-energy renderer. Without this second validator the
 * wire dropped every toolkit binding before it reached the renderer that knows
 * how to draw it, which is why the one real binding in the store dressed nothing.
 */
export function validateRoleKeyedMapping(
  mapping: RoleKeyedEmbodimentMapping
): boolean {
  if (
    mapping.schema_version !== "visual-embodiment/1-role-keyed" ||
    mapping.primitive_budget < 1 ||
    mapping.primitive_budget > NATIVE_MAX_PRIMITIVES ||
    mapping.particle_budget < 0 ||
    mapping.particle_budget > NATIVE_MAX_PARTICLES ||
    !mapping.archetypes ||
    Object.keys(mapping.archetypes).length === 0
  ) {
    return false;
  }
  const forms = [
    ...Object.values(mapping.archetypes).flatMap((archetype) =>
      Object.values(archetype.forms ?? {})
    ),
    ...Object.values(mapping.dormant_form ?? {})
  ];
  if (forms.length === 0) return false;
  return forms.every((form) =>
    formWithinBudgets(form, mapping.primitive_budget, mapping.particle_budget)
  );
}

/**
 * Either authored shape, for callers that only need to know the binding is
 * trustworthy — never for deciding WHICH renderer draws it.
 */
export function validateAnyEmbodimentMapping(
  mapping: VisualEmbodimentMapping | RoleKeyedEmbodimentMapping
): boolean {
  return mapping.schema_version === "visual-embodiment/1-role-keyed"
    ? validateRoleKeyedMapping(mapping)
    : validateEmbodimentMapping(mapping as VisualEmbodimentMapping);
}

/**
 * Whether the citizen-energy renderer may draw this embodiment: it needs the
 * LOD-keyed mapping AND the motion profile that drives it. A toolkit's own
 * role-keyed binding fails here and is drawn by the role-keyed path instead —
 * unbound is a correct outcome, a borrowed humanoid is not.
 */
export function isCitizenEnergyEmbodiment(
  embodiment: EntityEmbodiment
): embodiment is CitizenEnergyEmbodiment {
  return (
    embodiment.motion_profile !== undefined &&
    embodiment.mapping.schema_version === "visual-embodiment/1" &&
    validateEmbodimentMapping(embodiment.mapping as VisualEmbodimentMapping)
  );
}

/**
 * Narrows an embodiment to the LOD-keyed citizen-energy mapping, throwing when it
 * is not one. A caller that reads `mapping_id` or the `dynamics` envelope is
 * asserting a form family, and must establish it rather than assume it.
 */
export function citizenEnergyMapping(
  embodiment: EntityEmbodiment
): VisualEmbodimentMapping {
  if (embodiment.mapping.schema_version !== "visual-embodiment/1") {
    throw new Error(
      `not a citizen-energy mapping: ${embodiment.mapping.schema_version}`
    );
  }
  return embodiment.mapping as VisualEmbodimentMapping;
}

/**
 * The per-node modulation envelope, which only the LOD-keyed catalog declares.
 * A role-keyed toolkit binding has none — `undefined` here means "this authority
 * declared no envelope", so the renderer draws at identity instead of deriving
 * within bounds nobody authored.
 */
export function embodimentDynamicsEnvelope(
  embodiment: EntityEmbodiment | undefined
): VisualEmbodimentMapping["dynamics"] {
  const mapping = embodiment?.mapping;
  return mapping?.schema_version === "visual-embodiment/1"
    ? (mapping as VisualEmbodimentMapping).dynamics
    : undefined;
}

function formWithinBudgets(
  form: readonly EmbodimentPrimitiveTuple[],
  primitiveBudget: number,
  particleBudget: number
): boolean {
  if (form.length > primitiveBudget) return false;
  let particles = 0;
  for (const primitive of form) {
    if (!ALLOWED_PRIMITIVES.has(primitive[0])) return false;
    if (
      !validVector(primitive[3]) ||
      !validVector(primitive[4]) ||
      !validScale(primitive[5])
    ) {
      return false;
    }
    if (primitive[0] === "points") particles += primitive[6];
  }
  return particles <= particleBudget;
}

export function resolveEmbodimentForm(
  mapping: VisualEmbodimentMapping,
  residency: PhysicalResidency
): readonly EmbodimentPrimitiveTuple[] | null {
  if (!validateEmbodimentMapping(mapping)) return null;
  const requested = mapping.lod_states[residency];
  return mapping.forms[requested] ?? mapping.forms[mapping.fallback_form] ?? null;
}

export function measureMotion(
  previous: Vector3 | undefined,
  current: Vector3,
  previousSampledAtMs: number | undefined,
  sampledAtMs: number
): MotionSample {
  if (!previous || previousSampledAtMs === undefined) {
    return { velocity: [0, 0, 0], speed: 0 };
  }
  const deltaSeconds = (sampledAtMs - previousSampledAtMs) / 1000;
  if (!Number.isFinite(deltaSeconds) || deltaSeconds <= 0) {
    return { velocity: [0, 0, 0], speed: 0 };
  }
  const velocity: Vector3 = [
    (current[0] - previous[0]) / deltaSeconds,
    (current[1] - previous[1]) / deltaSeconds,
    (current[2] - previous[2]) / deltaSeconds
  ];
  return {
    velocity,
    speed: Math.hypot(velocity[0], velocity[1], velocity[2])
  };
}

export function advanceCriticalSpring(
  state: SpringState,
  target: Vector3,
  settleSeconds: number,
  deltaSeconds: number
): SpringState {
  const safeDelta = Math.min(Math.max(deltaSeconds, 0), 1 / 15);
  const omega = 4 / Math.max(settleSeconds, 0.05);
  const nextPosition: number[] = [];
  const nextVelocity: number[] = [];

  for (let axis = 0; axis < 3; axis += 1) {
    const change = state.position[axis] - target[axis];
    const exponential = Math.exp(-omega * safeDelta);
    const temporary = (state.velocity[axis] + omega * change) * safeDelta;
    nextPosition[axis] = target[axis] + (change + temporary) * exponential;
    nextVelocity[axis] =
      (state.velocity[axis] - omega * temporary) * exponential;
  }

  return {
    position: nextPosition as unknown as Vector3,
    velocity: nextVelocity as unknown as Vector3
  };
}

export function mapBounded(
  value: number,
  binding: readonly [number, number, number, number]
): number {
  const [inputMin, inputMax, outputMin, outputMax] = binding;
  if (inputMax <= inputMin) return outputMin;
  const progress = Math.min(
    1,
    Math.max(0, (value - inputMin) / (inputMax - inputMin))
  );
  return outputMin + (outputMax - outputMin) * progress;
}

export function correctionRequired(
  previous: Vector3 | undefined,
  current: Vector3,
  profile: EmbodimentMotionProfile
): boolean {
  if (!previous) return false;
  return (
    Math.hypot(
      current[0] - previous[0],
      current[1] - previous[1],
      current[2] - previous[2]
    ) > profile.interpolation.correction_threshold
  );
}

export function deterministicPointCloud(
  count: number,
  radius: number
): Float32Array {
  const safeCount = Math.max(
    0,
    Math.min(Math.floor(count), NATIVE_MAX_PARTICLES)
  );
  const positions = new Float32Array(safeCount * 3);
  const goldenAngle = Math.PI * (3 - Math.sqrt(5));
  for (let index = 0; index < safeCount; index += 1) {
    const y = 1 - (index / Math.max(1, safeCount - 1)) * 2;
    const radial = Math.sqrt(Math.max(0, 1 - y * y));
    const theta = goldenAngle * index;
    positions[index * 3] = Math.cos(theta) * radial * radius;
    positions[index * 3 + 1] = y * radius;
    positions[index * 3 + 2] = Math.sin(theta) * radial * radius;
  }
  return positions;
}

function validVector(value: Vector3): boolean {
  return value.length === 3 && value.every(Number.isFinite);
}

function validScale(value: Vector3): boolean {
  return validVector(value) && value.every((axis) => axis > 0);
}
