import type {
  EmbodimentMotionProfile,
  EmbodimentPrimitiveTuple,
  PhysicalResidency,
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
