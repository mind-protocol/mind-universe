import { useFrame } from "@react-three/fiber";
import { useRef } from "react";
import type { Group, MeshStandardMaterial } from "three";
import type { EnergyTransfer, MaterializedEntity } from "./contracts";
import { sampleTransferVisual } from "./visual-runtime";

function TransferGeometry({
  primitive
}: {
  readonly primitive: EnergyTransfer["visual"]["primitive"];
}) {
  switch (primitive) {
    case "energy_packet":
      return <sphereGeometry args={[1, 20, 20]} />;
    case "inhibitory_wave":
      return <torusGeometry args={[1, 0.18, 10, 28]} />;
    case "rupture":
      return <octahedronGeometry args={[1, 0]} />;
  }
}

export function EnergyTransferEffect({
  transfer,
  source,
  target
}: {
  readonly transfer: EnergyTransfer;
  readonly source: MaterializedEntity;
  readonly target: MaterializedEntity;
}) {
  const group = useRef<Group>(null);
  const material = useRef<MeshStandardMaterial>(null);
  const startedAt = useRef<number | null>(null);

  useFrame(({ clock }) => {
    if (!group.current) return;
    const now = clock.elapsedTime * 1_000;
    startedAt.current ??= now;
    const frame = sampleTransferVisual(
      transfer.visual,
      source.position,
      target.position,
      now - startedAt.current
    );
    group.current.position.set(...frame.position);
    group.current.scale.setScalar(frame.scale);
    group.current.visible = frame.visible;
    if (material.current) material.current.opacity = frame.opacity;
  });

  return (
    <group ref={group}>
      <mesh>
        <TransferGeometry primitive={transfer.visual.primitive} />
        <meshStandardMaterial
          ref={material}
          color={transfer.visual.color}
          emissive={transfer.visual.emissive}
          emissiveIntensity={transfer.visual.emissiveIntensity}
          opacity={transfer.visual.opacity}
          transparent
          roughness={0.2}
          metalness={0.05}
          wireframe={transfer.visual.primitive === "rupture"}
        />
      </mesh>
    </group>
  );
}
