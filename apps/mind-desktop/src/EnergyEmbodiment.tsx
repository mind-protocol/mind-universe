import { Line } from "@react-three/drei";
import { useFrame } from "@react-three/fiber";
import { useMemo, useRef } from "react";
import {
  AdditiveBlending,
  Color,
  type Group,
  Vector3 as ThreeVector3
} from "three";
import type {
  EmbodimentPrimitiveTuple,
  EntityEmbodiment,
  MaterializedEntity,
  Vector3
} from "./contracts";
import {
  advanceCriticalSpring,
  correctionRequired,
  deterministicPointCloud,
  mapBounded,
  measureMotion,
  resolveEmbodimentForm,
  type SpringState
} from "./embodiment";

const FRESNEL_VERTEX_SHADER = `
varying vec3 vNormal;
varying vec3 vViewDirection;
void main() {
  vec4 worldPosition = modelMatrix * vec4(position, 1.0);
  vNormal = normalize(mat3(modelMatrix) * normal);
  vViewDirection = normalize(cameraPosition - worldPosition.xyz);
  gl_Position = projectionMatrix * viewMatrix * worldPosition;
}`;

const FRESNEL_FRAGMENT_SHADER = `
uniform vec3 uColor;
uniform float uOpacity;
uniform float uPower;
varying vec3 vNormal;
varying vec3 vViewDirection;
void main() {
  float rim = pow(1.0 - abs(dot(normalize(vNormal), normalize(vViewDirection))), uPower);
  gl_FragColor = vec4(uColor * rim, rim * uOpacity);
}`;

type EmbodiedEntity = MaterializedEntity & {
  readonly embodiment: EntityEmbodiment;
};

export function Embodiment({
  entity,
  synchronized
}: {
  readonly entity: EmbodiedEntity;
  readonly synchronized: boolean;
}) {
  const { embodiment } = entity;
  const root = useRef<Group>(null);
  const spring = useRef<SpringState>({
    position: entity.position,
    velocity: [0, 0, 0]
  });
  const form =
    resolveEmbodimentForm(embodiment.mapping, embodiment.residency) ?? [];
  const motion = measureMotion(
    embodiment.previous_position,
    entity.position,
    embodiment.previous_sampled_at_ms,
    embodiment.sampled_at_ms
  );
  const correction = correctionRequired(
    embodiment.previous_position,
    entity.position,
    embodiment.motion_profile
  );
  const systemPrefersReducedMotion =
    typeof globalThis.matchMedia === "function" &&
    globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const reducedMotion =
    embodiment.reduced_motion ?? systemPrefersReducedMotion;
  const stretch = reducedMotion
    ? 1
    : mapBounded(
        motion.speed,
        embodiment.motion_profile.bindings.speed_to_stretch
      );
  const trailOpacity =
    reducedMotion || !synchronized
      ? 0
      : mapBounded(
          motion.speed,
          embodiment.motion_profile.bindings.speed_to_trail_opacity
        );

  useFrame(({ clock }, delta) => {
    const group = root.current;
    if (!group) return;
    spring.current = advanceCriticalSpring(
      spring.current,
      entity.position,
      embodiment.motion_profile.interpolation.settle_seconds,
      delta
    );
    group.position.fromArray([...spring.current.position]);
    const breath = reducedMotion
      ? 1
      : 1 +
        Math.sin(
          clock.elapsedTime *
            Math.PI *
            2 *
            embodiment.motion_profile.bindings.idle_breath.frequency_hz
        ) *
          embodiment.motion_profile.bindings.idle_breath.amplitude;
    group.scale.set(stretch, breath, 1 / Math.sqrt(stretch));
    const horizontalSpeed = Math.hypot(
      motion.velocity[0],
      motion.velocity[2]
    );
    if (horizontalSpeed > 0.01) {
      group.rotation.y = Math.atan2(motion.velocity[0], motion.velocity[2]);
    }
  });

  return (
    <group
      ref={root}
      position={[...entity.position]}
      userData={{
        embodimentMapping: embodiment.source_mapping_id,
        synchronized,
        correction
      }}
    >
      {form.map((primitive, index) => (
        <GraphPrimitive
          key={`${primitive[1]}-${index}`}
          primitive={primitive}
          entity={entity}
          synchronized={synchronized}
          correction={correction}
        />
      ))}
      {trailOpacity > 0 ? (
        <MotionTrail
          velocity={motion.velocity}
          opacity={trailOpacity}
          color={embodiment.mapping.palette.shell}
          maxSamples={embodiment.motion_profile.trail.max_samples}
        />
      ) : null}
    </group>
  );
}

function GraphPrimitive({
  primitive,
  entity,
  synchronized,
  correction
}: {
  readonly primitive: EmbodimentPrimitiveTuple;
  readonly entity: EmbodiedEntity;
  readonly synchronized: boolean;
  readonly correction: boolean;
}) {
  const [kind, role, material, offset, rotation, scale, count, radius] =
    primitive;
  const mapping = entity.embodiment.mapping;
  const staleOpacity = synchronized ? 1 : 0.42;
  const color =
    material === "particles"
      ? mapping.palette.particle
      : material === "shell"
        ? mapping.palette.shell
        : mapping.palette.core;

  if (kind === "points") {
    const positions = deterministicPointCloud(count, radius);
    return (
      <points
        position={[...offset]}
        rotation={[...rotation]}
        scale={[...scale]}
        userData={{ role }}
      >
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            args={[positions, 3]}
          />
        </bufferGeometry>
        <pointsMaterial
          color={color}
          size={0.045}
          transparent
          opacity={0.72 * staleOpacity}
          depthWrite={false}
          blending={AdditiveBlending}
        />
      </points>
    );
  }

  if (kind === "fresnel_shell") {
    return (
      <mesh
        position={[...offset]}
        rotation={[...rotation]}
        scale={[...scale]}
        userData={{ role }}
      >
        <sphereGeometry args={[1, 32, 32]} />
        <shaderMaterial
          vertexShader={FRESNEL_VERTEX_SHADER}
          fragmentShader={FRESNEL_FRAGMENT_SHADER}
          uniforms={{
            uColor: { value: new Color(correction ? "#ffffff" : color) },
            uOpacity: {
              value:
                mapping.material.shell_opacity *
                staleOpacity *
                (correction ? 1.8 : 1)
            },
            uPower: { value: mapping.material.fresnel_power }
          }}
          transparent
          depthWrite={false}
          blending={AdditiveBlending}
        />
      </mesh>
    );
  }

  return (
    <mesh
      position={[...offset]}
      rotation={[...rotation]}
      scale={[...scale]}
      userData={{ role }}
    >
      {kind === "icosphere" ? (
        <icosahedronGeometry args={[1, 3]} />
      ) : kind === "capsule" ? (
        <capsuleGeometry args={[1, 1, 8, 20]} />
      ) : kind === "box" ? (
        <boxGeometry args={[1, 1, 1]} />
      ) : kind === "cylinder" ? (
        <cylinderGeometry args={[1, 1, 1, 24]} />
      ) : kind === "cone" ? (
        <coneGeometry args={[1, 1, 24]} />
      ) : kind === "torus" ? (
        <torusGeometry args={[1, 0.35, 16, 32]} />
      ) : kind === "plane" ? (
        <planeGeometry args={[1, 1]} />
      ) : kind === "tube" ? (
        // a thin conduit — a slender cylinder along its local Y axis
        <cylinderGeometry args={[0.18, 0.18, 1, 16]} />
      ) : (
        <sphereGeometry args={[1, 28, 28]} />
      )}
      <meshStandardMaterial
        color={color}
        emissive={mapping.palette.emissive}
        emissiveIntensity={
          material === "core"
            ? mapping.material.core_emissive_intensity
            : mapping.material.shell_emissive_intensity
        }
        transparent
        opacity={
          (material === "core"
            ? mapping.material.core_opacity
            : mapping.material.shell_opacity) * staleOpacity
        }
        roughness={0.2}
        metalness={0.04}
        depthWrite={material === "core"}
        wireframe={!synchronized}
      />
    </mesh>
  );
}

function MotionTrail({
  velocity,
  opacity,
  color,
  maxSamples
}: {
  readonly velocity: Vector3;
  readonly opacity: number;
  readonly color: string;
  readonly maxSamples: number;
}) {
  const points = useMemo(() => {
    const direction = new ThreeVector3(...velocity);
    if (direction.lengthSq() < 0.0001) return [[0, 0, 0] as Vector3];
    direction.normalize().multiplyScalar(-1);
    const count = Math.max(2, Math.min(maxSamples, 24));
    return Array.from({ length: count }, (_, index) => {
      const distance = (index / (count - 1)) ** 1.35 * 1.8;
      return [
        direction.x * distance,
        direction.y * distance,
        direction.z * distance
      ] as Vector3;
    });
  }, [maxSamples, velocity[0], velocity[1], velocity[2]]);

  return (
    <Line
      points={points.map((point) => [...point])}
      color={color}
      lineWidth={3}
      transparent
      opacity={opacity}
      userData={{ role: "motion_trail", evidence: "projection_only" }}
    />
  );
}
