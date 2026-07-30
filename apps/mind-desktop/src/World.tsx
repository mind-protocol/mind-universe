import { Html, Line, Stars } from "@react-three/drei";
import { Canvas, useFrame } from "@react-three/fiber";
import { Fragment, useRef, useState } from "react";
import type { BufferGeometry, Group } from "three";
import type { MaterializedEntity, MaterializedRelation } from "./contracts";
import { ActorControls } from "./ActorControls";
import type { MotionBounds } from "./actor-control";
import type { Vector3 as Vec3 } from "./contracts";
import { Embodiment } from "./EnergyEmbodiment";
import { validateEmbodimentMapping } from "./embodiment";
import { EnergyTransferEffect } from "./EnergyTransfer";
import { ObserverControls } from "./ObserverControls";
import type {
  EntityPresentation,
  RelationPresentation
} from "./postgres-pilot-fixture";
import type { UniverseView } from "./universe-state";

// Optional actor-piloting wiring. When present and `piloting` is true, ZQSD
// drives the bound Actor (ActorControls) and the camera's keyboard translation
// steps aside; otherwise ZQSD flies the observer camera as before.
export interface ActorControlProps {
  readonly bounds: MotionBounds;
  readonly piloting: boolean;
  readonly onMove: (displacement: Vec3) => void;
}

function EntityGeometry({
  primitive
}: {
  readonly primitive: MaterializedEntity["visual"]["primitive"];
}) {
  switch (primitive) {
    case "pulsing_core":
      return <sphereGeometry args={[1, 24, 24]} />;
    case "open_polyhedral_attractor":
      return <octahedronGeometry args={[1, 1]} />;
    case "oriented_ring":
      return <torusGeometry args={[0.78, 0.16, 12, 36]} />;
    case "bounded_volume":
      return <boxGeometry args={[1.6, 1.6, 1.6]} />;
    case "faceted_router":
      return <dodecahedronGeometry args={[1, 0]} />;
    case "slab":
      return <boxGeometry args={[1.2, 1.8, 0.2]} />;
    case "torus_knot":
      return <torusKnotGeometry args={[0.8, 0.2, 64, 12]} />;
    case "cylinder":
      return <cylinderGeometry args={[0.4, 0.4, 1.6, 12]} />;
    case "tetrahedron":
      return <tetrahedronGeometry args={[1.2, 0]} />;
    case "unknown":
      return <icosahedronGeometry args={[1, 1]} />;
  }
}

function Atom({
  entity,
  presentation,
  selected,
  onSelect,
  synchronized
}: {
  readonly entity: MaterializedEntity;
  readonly presentation: EntityPresentation | undefined;
  readonly selected: boolean;
  readonly onSelect: () => void;
  readonly synchronized: boolean;
}) {
  if (
    entity.embodiment &&
    validateEmbodimentMapping(entity.embodiment.mapping)
  ) {
    return (
      <Embodiment
        entity={entity as MaterializedEntity & {
          readonly embodiment: NonNullable<MaterializedEntity["embodiment"]>;
        }}
        synchronized={synchronized}
      />
    );
  }
  return (
    <GenericAtom
      entity={entity}
      presentation={presentation}
      selected={selected}
      onSelect={onSelect}
    />
  );
}

function GenericAtom({
  entity,
  presentation,
  selected,
  onSelect
}: {
  readonly entity: MaterializedEntity;
  readonly presentation: EntityPresentation | undefined;
  readonly selected: boolean;
  readonly onSelect: () => void;
}) {
  const group = useRef<Group>(null);
  const { primitive, motion, material } = entity.visual;
  const { color, emissive, emissiveIntensity, opacity, scale } = material;

  useFrame(({ clock }, delta) => {
    if (!group.current) return;
    const phase = clock.elapsedTime;
    switch (motion) {
      case "outward_pulse":
        group.current.scale.setScalar(scale * (1 + Math.sin(phase * 2.4) * 0.08));
        break;
      case "inward_orbit":
        group.current.rotation.y -= delta * 0.32;
        break;
      case "through_flow":
        group.current.rotation.x += delta * 0.5;
        break;
      case "boundary_breath":
        group.current.scale.setScalar(scale * (1 + Math.sin(phase * 0.8) * 0.04));
        break;
      case "port_transform":
        group.current.rotation.y += delta * 0.42;
        group.current.rotation.z -= delta * 0.18;
        break;
      case "still":
        break;
    }
  });

  return (
    <group ref={group} position={[...entity.position]} scale={scale}>
      <mesh
        onClick={(event) => {
          event.stopPropagation();
          onSelect();
        }}
      >
        <EntityGeometry primitive={primitive} />
        <meshStandardMaterial
          color={color}
          emissive={emissive}
          emissiveIntensity={emissiveIntensity}
          transparent={opacity < 1}
          opacity={opacity}
          roughness={0.32}
          metalness={0.08}
          wireframe={
            primitive === "open_polyhedral_attractor" ||
            primitive === "bounded_volume"
          }
        />
      </mesh>
      {presentation && (
        <Html
          center
          distanceFactor={11}
          position={[0, selected ? 1.85 : 1.45, 0]}
          zIndexRange={[20, 0]}
        >
          <button
            type="button"
            className={`entity-label${selected ? " selected" : ""}`}
            data-epistemic={presentation.epistemic}
            onClick={onSelect}
          >
            <strong>{presentation.label}</strong>
            <span>{presentation.state.replaceAll("_", " ")}</span>
            {selected && <small>{presentation.detail}</small>}
          </button>
        </Html>
      )}
    </group>
  );
}

function Bond({
  relation,
  entities,
  presentation
}: {
  readonly relation: MaterializedRelation;
  readonly entities: UniverseView["entities"];
  readonly presentation: RelationPresentation | undefined;
}) {
  const source = entities.get(relation.source);
  const target = entities.get(relation.target);
  if (!source || !target) return null;
  const { material, width, primitive } = relation.visual;

  const midpoint = source.position.map(
    (coordinate, index) => (coordinate + target.position[index]) / 2
  ) as unknown as [number, number, number];

  const isLuminous = primitive === "luminous_chain";
  const isNavigable = primitive === "navigable_path";
  const color = isLuminous ? material.emissive : material.color;
  const opacity = isLuminous ? Math.min(1, material.opacity * 1.5) : material.opacity;
  const finalWidth = isLuminous || isNavigable ? width * 2 : width;
  const dashed = isNavigable;

  return (
    <Fragment>
      <Line
        points={[[...source.position], [...target.position]]}
        color={color}
        lineWidth={finalWidth}
        transparent
        opacity={opacity}
        dashed={dashed}
        dashSize={dashed ? 0.4 : undefined}
        gapSize={dashed ? 0.2 : undefined}
      />
      {presentation?.label && (
        <Html center distanceFactor={13} position={midpoint} zIndexRange={[10, 0]}>
          <span className="relation-label">{presentation.label}</span>
        </Html>
      )}
    </Fragment>
  );
}

// The world is a floor, not a void. Entities float above a ground reference
// that is always present — it may roll up or down, but there is always a "sol"
// beneath the scene so height reads as height. The surface undulates slowly so
// it feels alive; its base sits below the lowest fixture entity (world y -3.85).
function Ground() {
  const geometry = useRef<BufferGeometry>(null);
  const SIZE = 120;
  const SEGMENTS = 72;
  const BASE_Y = -5.4;

  useFrame(({ clock }) => {
    const geom = geometry.current;
    if (!geom) return;
    const position = geom.attributes.position;
    const time = clock.elapsedTime;
    // Plane is rotated flat about X, so local (x, y) span the floor and local z
    // becomes world height. Displace only z to make the terrain rise and fall.
    for (let index = 0; index < position.count; index += 1) {
      const x = position.getX(index);
      const y = position.getY(index);
      const height =
        Math.sin(x * 0.16 + time * 0.35) * 0.7 +
        Math.cos(y * 0.13 - time * 0.28) * 0.55 +
        Math.sin((x + y) * 0.08 + time * 0.18) * 0.4;
      position.setZ(index, height);
    }
    position.needsUpdate = true;
  });

  return (
    <mesh
      rotation={[-Math.PI / 2, 0, 0]}
      position={[0, BASE_Y, 0]}
      // Chrome, not an entity: never intercept clicks meant for atoms.
      raycast={() => null}
    >
      <planeGeometry ref={geometry} args={[SIZE, SIZE, SEGMENTS, SEGMENTS]} />
      <meshBasicMaterial
        color="#2a4a7a"
        wireframe
        transparent
        opacity={0.28}
      />
    </mesh>
  );
}

export function World({
  universe,
  entityPresentation,
  relationPresentation,
  actorControl
}: {
  readonly universe: UniverseView;
  readonly entityPresentation: ReadonlyMap<string, EntityPresentation>;
  readonly relationPresentation: ReadonlyMap<string, RelationPresentation>;
  readonly actorControl?: ActorControlProps;
}) {
  const [selected, setSelected] = useState<string | null>(
    "00000000000000000000000000005005"
  );
  const hasEmbodiedEntity = [...universe.entities.values()].some(
    (entity) => entity.embodiment !== undefined
  );

  return (
    <Canvas
      camera={{
        position: hasEmbodiedEntity ? [0, 1.1, 6] : [0, 2.2, 18],
        fov: hasEmbodiedEntity ? 46 : 52
      }}
      dpr={[1, 2]}
      onPointerMissed={() => setSelected(null)}
    >
      <color attach="background" args={["#020307"]} />
      <fog attach="fog" args={["#020307", 18, 80]} />
      <ambientLight intensity={0.08} />
      <pointLight position={[0, 5, 2]} intensity={10} color="#c9dcff" />
      <Stars radius={70} depth={30} count={900} factor={1.5} fade speed={0.1} />
      <Ground />
      {[...universe.relations.values()].map((relation) => (
        <Bond
          key={relation.id}
          relation={relation}
          entities={universe.entities}
          presentation={relationPresentation.get(relation.id)}
        />
      ))}
      {[...universe.entities.values()].map((entity) => (
        <Atom
          key={entity.id}
          entity={entity}
          presentation={entityPresentation.get(entity.id)}
          selected={selected === entity.id}
          onSelect={() => setSelected(entity.id)}
          synchronized={universe.synchronized}
        />
      ))}
      {[...universe.transfers.values()].map((transfer) => {
        const source = universe.entities.get(transfer.source);
        const target = universe.entities.get(transfer.target);
        if (!source || !target) return null;
        return (
          <EnergyTransferEffect
            key={transfer.transferId}
            transfer={transfer}
            source={source}
            target={target}
          />
        );
      })}
      <ObserverControls movementEnabled={!actorControl?.piloting} />
      {actorControl && (
        <ActorControls
          bounds={actorControl.bounds}
          piloting={actorControl.piloting}
          onMove={actorControl.onMove}
        />
      )}
    </Canvas>
  );
}
