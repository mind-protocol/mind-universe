import { Html, Stars } from "@react-three/drei";
import { Canvas, useFrame } from "@react-three/fiber";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  CatmullRomCurve3,
  PlaneGeometry,
  TubeGeometry,
  Vector3 as ThreeVector3
} from "three";
import type { Group } from "three";
import type {
  EntityVisualPrimitive,
  MaterializedEntity,
  MaterializedRelation
} from "./contracts";
import {
  TERRAIN_SEGMENTS,
  TERRAIN_SIZE,
  terrainHeight
} from "./terrain";
import { settleOnGround } from "./gravity";
import {
  drapedRoute,
  infrastructureStyle,
  projectPhysicalProfile
} from "./relation-infrastructure";
import { ActorControls } from "./ActorControls";
import type { MotionBounds } from "./actor-control";
import type { Vector3 as Vec3 } from "./contracts";
import { Embodiment } from "./EnergyEmbodiment";
import { validateEmbodimentMapping } from "./embodiment";
import { EnergyTransferEffect } from "./EnergyTransfer";
import { ObserverControls } from "./ObserverControls";
import { orientationFromLookAt } from "./first-person-look";
import { Minimap, type MinimapPose } from "./Minimap";
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

// A node's physical extent, read from the geometry it is ACTUALLY drawn with
// (see EntityGeometry) — honest sizing from the node's own form, not an invented
// per-role table. Footprint (horizontal) and half-height (vertical) are kept
// separate so gravity stacks correctly: a flat/wide shape carries what sits on it
// at its surface. Dimensions mirror the geometry args as full extents [width,
// height]; a future `space` plateau primitive (wide + thin) drops in here.
const GEOMETRY_EXTENT: Record<
  EntityVisualPrimitive,
  { readonly width: number; readonly height: number }
> = {
  pulsing_core: { width: 2, height: 2 },
  open_polyhedral_attractor: { width: 2, height: 2 },
  oriented_ring: { width: 1.88, height: 0.32 },
  bounded_volume: { width: 1.6, height: 1.6 },
  faceted_router: { width: 2, height: 2 },
  slab: { width: 1.2, height: 1.8 },
  torus_knot: { width: 2, height: 2 },
  cylinder: { width: 0.8, height: 1.6 },
  tetrahedron: { width: 2.4, height: 2.4 },
  unknown: { width: 2, height: 2 }
};

function nodeExtent(entity: MaterializedEntity): {
  readonly footprint: number;
  readonly halfHeight: number;
} {
  const dim = GEOMETRY_EXTENT[entity.visual.primitive];
  const scale = entity.visual.material.scale;
  return {
    footprint: Math.max(0.2, (dim.width / 2) * scale),
    halfHeight: Math.max(0.2, (dim.height / 2) * scale)
  };
}

function Atom({
  entity,
  presentation,
  selected,
  onSelect,
  onHover,
  synchronized
}: {
  readonly entity: MaterializedEntity;
  readonly presentation: EntityPresentation | undefined;
  readonly selected: boolean;
  readonly onSelect: () => void;
  readonly onHover: (id: string | null) => void;
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
      onHover={onHover}
    />
  );
}

export function GenericAtom({
  entity,
  selected,
  onSelect,
  onHover
}: {
  readonly entity: MaterializedEntity;
  readonly presentation: EntityPresentation | undefined;
  readonly selected: boolean;
  readonly onSelect: () => void;
  readonly onHover: (id: string | null) => void;
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
        onPointerOver={(event) => {
          event.stopPropagation();
          onHover(entity.id);
        }}
        onPointerOut={(event) => {
          event.stopPropagation();
          onHover(null);
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
    </group>
  );
}

// A Bond is a street between two building footprints, not a wire between floating
// centres (ontology3d forbids generic lines). Its predicate resolves to an
// infrastructure family — causeway, channel, provenance beam, zone band, … — and
// the route drapes over the land so it reads as a road you could walk. An
// unclassified predicate stays a foggy "unknown" street rather than pretending.
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
  // Single canonical table (ALIGN.md §2), fed from EITHER carrier of the same
  // channels so the bond looks identical whether it came from a fixture or the
  // live store:
  //   1. the fixture projection's full physical_profile (presentation.physics), or
  //   2. the wire, which carries polarity/hierarchy on relation.visual
  //      (desktop_world_snapshot → protocol-adapter, ALIGN.md §4).
  // The wire omits permanence and any calibration signal, so a wire-sourced bond
  // renders a neutral thickness and is treated as uncalibrated (honest fog) rather
  // than asserting a calibration it never received. A predicate with neither
  // carrier stays on the predicate-family fallback below.
  const wirePolarity: readonly [number, number] | undefined =
    relation.visual.polarity;
  const wireHierarchy: number | undefined = relation.visual.hierarchy;
  const wireChannels =
    wirePolarity !== undefined || wireHierarchy !== undefined
      ? projectPhysicalProfile({
          family: "unknown",
          polarity: wirePolarity ?? [0, 0],
          hierarchy: wireHierarchy ?? 0,
          permanence: 0.5,
          mode: "axis",
          calibrated: false
        })
      : null;
  const channels = presentation?.physics
    ? projectPhysicalProfile(presentation.physics)
    : wireChannels;
  const style = infrastructureStyle(presentation?.predicate);
  const color = channels ? channels.lightColor : style.color;
  const radius = channels ? channels.radius : 0.02 + style.width * 0.012;
  const slope = channels ? channels.slope : 0;

  // The bond is a soft, organic vein rather than a hard wire: a Catmull-Rom curve
  // swept as a slender tube, self-lit yet faint like a filament.
  const geometry = useMemo(() => {
    if (!source || !target) return null;
    let route: Vec3[];
    if (channels) {
      // Canonical path: route between the REAL building positions and pitch the
      // conduit by hierarchy (+slope ⇒ source below target). No flattening — the
      // link keeps the third dimension the layout gives it.
      const [ax, ay, az] = source.position;
      const [bx, by, bz] = target.position;
      const samples = 18;
      route = Array.from({ length: samples + 1 }, (_, i) => {
        const t = i / samples;
        const x = ax + (bx - ax) * t;
        const z = az + (bz - az) * t;
        const baseY = ay + (by - ay) * t;
        const tilt = (t - 0.5) * slope * 2.2;
        const arch = Math.sin(t * Math.PI) * 0.35;
        return [x, baseY + tilt + arch, z] as Vec3;
      });
    } else {
      // Fallback: drape along the land between footprints (predicate-only source).
      route = drapedRoute(
        [source.position[0], 0, source.position[2]],
        [target.position[0], 0, target.position[2]]
      );
    }
    const curve = new CatmullRomCurve3(
      route.map((point) => new ThreeVector3(point[0], point[1], point[2])),
      false,
      "catmullrom",
      0.5
    );
    return new TubeGeometry(curve, 48, radius, 8, false);
  }, [source, target, radius, slope, channels]);

  useEffect(() => () => geometry?.dispose(), [geometry]);

  if (!geometry) return null;

  return (
    <mesh geometry={geometry} raycast={() => null}>
      <meshStandardMaterial
        color="#05070d"
        emissive={color}
        emissiveIntensity={2.6}
        transparent
        // Uncalibrated profiles read fainter — the honesty channel (ALIGN §2).
        opacity={channels && !channels.calibrated ? 0.11 : 0.16}
        roughness={1}
        metalness={0}
        toneMapped={false}
        depthWrite={false}
      />
    </mesh>
  );
}

// The city sits on land, not in a void (ontology3d: Space is a territory). The
// ground is a stable elevation field — it rises and falls across the map as
// districts, but is solid rather than animated, so buildings can rest on it. The
// same relief is rendered twice: a dark land mass for body, and a luminous grid on
// top reading as streets/blocks. Both share one displaced geometry so there is a
// single, deterministic surface (which the foundations also sample).
function Ground() {
  const geometry = useMemo(() => {
    const geom = new PlaneGeometry(
      TERRAIN_SIZE,
      TERRAIN_SIZE,
      TERRAIN_SEGMENTS,
      TERRAIN_SEGMENTS
    );
    const position = geom.attributes.position;
    // The mesh is rotated flat about X, so a plane vertex (px, py) lands at world
    // (px, localZ, -py). Displacing local z therefore sets world height y, and the
    // world footprint is (px, -py) — exactly what terrainHeight expects.
    for (let index = 0; index < position.count; index += 1) {
      const px = position.getX(index);
      const py = position.getY(index);
      position.setZ(index, terrainHeight(px, -py));
    }
    position.needsUpdate = true;
    geom.computeVertexNormals();
    return geom;
  }, []);

  useEffect(() => () => geometry.dispose(), [geometry]);

  return (
    // Chrome, not entities: neither surface intercepts clicks meant for buildings.
    <group rotation={[-Math.PI / 2, 0, 0]} raycast={() => null}>
      <mesh geometry={geometry} raycast={() => null}>
        {/* Opaque land: at standing eye height the floor must read as solid
            ground, never a translucent sheet you can see the void through. */}
        <meshStandardMaterial color="#0a1424" roughness={0.95} metalness={0} />
      </mesh>
      <mesh geometry={geometry} raycast={() => null}>
        <meshBasicMaterial color="#2f5488" wireframe transparent opacity={0.3} />
      </mesh>
    </group>
  );
}

// Every building is rooted to the land. A foundation drops a slender support
// column from an entity down to the terrain directly beneath it and marks its plot
// with a luminous footprint. This grounds otherwise-floating atoms into a skyline
// without moving them — the position stays authored; only its relation to the land
// is made visible (ontology3d foundation family: roots, pillars, foundations).
function Foundation({ entity }: { readonly entity: MaterializedEntity }) {
  const [x, y, z] = entity.position;
  const groundY = terrainHeight(x, z);
  const height = Math.max(0.1, y - groundY);
  const color = entity.embodiment
    ? entity.embodiment.mapping.palette.shell
    : entity.visual.material.color;

  return (
    <group raycast={() => null}>
      <mesh position={[x, groundY + height / 2, z]} raycast={() => null}>
        <cylinderGeometry args={[0.045, 0.11, height, 6]} />
        <meshBasicMaterial color={color} transparent opacity={0.22} />
      </mesh>
      <mesh
        position={[x, groundY + 0.03, z]}
        rotation={[-Math.PI / 2, 0, 0]}
        raycast={() => null}
      >
        <ringGeometry args={[0.24, 0.42, 24]} />
        <meshBasicMaterial color={color} transparent opacity={0.42} />
      </mesh>
    </group>
  );
}

export function World({
  universe,
  entityPresentation,
  relationPresentation,
  actorControl,
  focus
}: {
  readonly universe: UniverseView;
  readonly entityPresentation: ReadonlyMap<string, EntityPresentation>;
  readonly relationPresentation: ReadonlyMap<string, RelationPresentation>;
  readonly actorControl?: ActorControlProps;
  // Optional default framing: preselect this entity and point the observer
  // camera at `target`, so the viz opens looking at (e.g.) the latest active
  // ActorSession rather than the world origin.
  readonly focus?: { readonly entityId: string; readonly target: Vec3 };
}) {
  const [selected, setSelected] = useState<string | null>(
    focus?.entityId ?? "00000000000000000000000000005005"
  );
  // The node under the pointer (its label/detail float above it). Hover is a pure
  // observation — it reads presentation, never mutates the world.
  const [hovered, setHovered] = useState<string | null>(null);
  useEffect(() => {
    if (!hovered) return;
    document.body.style.cursor = "pointer";
    return () => {
      document.body.style.cursor = "";
    };
  }, [hovered]);
  // Gravity: every building's base rests on the land, except when its footprint
  // overlaps another and it stacks on top (ontology3d: a city rests on solid
  // ground, not in a void). The kernel layout keeps x/z authoritative; here we
  // only settle the height onto the terrain relief the scene actually draws, so
  // atoms sit on the ground instead of floating over a foundation column.
  const groundedEntities = useMemo(() => {
    const settled = settleOnGround(
      [...universe.entities.values()].map((entity) => {
        const extent = nodeExtent(entity);
        return {
          id: entity.id,
          x: entity.position[0],
          z: entity.position[2],
          footprint: extent.footprint,
          halfHeight: extent.halfHeight,
          seedY: entity.position[1]
        };
      }),
      terrainHeight
    );
    const grounded = new Map<string, MaterializedEntity>();
    for (const entity of universe.entities.values()) {
      const drop = settled.get(entity.id);
      grounded.set(
        entity.id,
        drop
          ? { ...entity, position: [entity.position[0], drop.y, entity.position[2]] }
          : entity
      );
    }
    return grounded;
  }, [universe.entities]);
  // A *solo* embodiment fixture (just the avatar, nothing else) earns the close,
  // intimate framing. An avatar dropped into a populated city is NOT solo — the
  // scene is still a city and must keep its city framing, or one embodied entity
  // would collapse the whole view onto the avatar.
  const soloEmbodiment =
    universe.entities.size <= 2 &&
    [...universe.entities.values()].some(
      (entity) => entity.embodiment !== undefined
    );
  // Frame the camera and atmosphere to the city's footprint so a large ontology
  // registry (~90 units across) is fully visible, while small fixtures keep their
  // close framing. cityRadius is the farthest building from the plaza centre.
  let cityRadius = 0;
  for (const entity of universe.entities.values()) {
    cityRadius = Math.max(
      cityRadius,
      Math.hypot(entity.position[0], entity.position[2])
    );
  }
  const wideCity = !soloEmbodiment && cityRadius > 25;
  // A default focus (e.g. the latest active ActorSession) overrides framing: the
  // observer stands right next to the main node, a short gap out from it (its own
  // footprint plus a small margin) and looks at it. The eye's y is pinned to the
  // floor + standing height by ObserverControls, so only x/z matter here.
  const focusEntity = focus ? universe.entities.get(focus.entityId) : undefined;
  // Stand just clear of the node: its own footprint plus a ~3-unit margin of space.
  const focusStandoff = focusEntity ? nodeExtent(focusEntity).footprint + 3 : 3;
  const cameraPosition: Vec3 = focus
    ? [focus.target[0], focus.target[1], focus.target[2] + focusStandoff]
    : soloEmbodiment
    ? [0, 1.1, 6]
    : wideCity
      ? [0, cityRadius * 0.72, cityRadius * 1.5]
      : [0, 2.2, 18];
  const cameraFov = soloEmbodiment ? 46 : wideCity ? 55 : 52;
  const fogFar = wideCity ? cityRadius * 2.8 : 80;
  const starRadius = wideCity ? cityRadius * 2.1 : 70;

  // The observer's live top-down pose, shared with the minimap through a ref so a
  // camera move updates only the minimap marker, never the whole scene. Seeded from
  // the opening framing so the marker is placed correctly before the first frame
  // (which matters where the render loop is suspended, e.g. a hidden preview).
  const poseRef = useRef<MinimapPose>({
    x: cameraPosition[0],
    z: cameraPosition[2],
    yaw: orientationFromLookAt(cameraPosition, focus?.target ?? [0, 0, 0]).yaw
  });

  // Bounded-neighborhood filter (ontology3d query-budget spirit): keep only the
  // buildings within a radius of the plaza centre, culling the most distant first.
  // `null` means the whole city. The control appears only for large graphs.
  // Top-K nearest the plaza centre: dragging the control down keeps fewer, denser
  // buildings and drops the most distant first. `null` means the whole city.
  const [keepCount, setKeepCount] = useState<number | null>(null);
  const totalNodes = universe.entities.size;
  const showFilter = totalNodes > 50;
  const sortedByDistance = [...groundedEntities.values()].sort(
    (a, b) =>
      a.position[0] ** 2 + a.position[2] ** 2 -
      (b.position[0] ** 2 + b.position[2] ** 2)
  );
  const keep = keepCount ?? totalNodes;
  const filterMin = Math.min(10, totalNodes);
  const visibleEntities = sortedByDistance.slice(0, keep);
  const visibleIds = new Set(visibleEntities.map((entity) => entity.id));
  const visibleRelations = [...universe.relations.values()].filter(
    (relation) =>
      visibleIds.has(relation.source) && visibleIds.has(relation.target)
  );

  // The hover tooltip anchors just above the hovered node's crown (its grounded
  // centre + half-height + a small gap), and only for a node currently visible.
  const hoveredEntity =
    hovered && visibleIds.has(hovered) ? groundedEntities.get(hovered) : undefined;
  const hoveredTooltip = hoveredEntity
    ? {
        id: hoveredEntity.id,
        presentation: entityPresentation.get(hoveredEntity.id),
        position: [
          hoveredEntity.position[0],
          hoveredEntity.position[1] + nodeExtent(hoveredEntity).halfHeight + 0.9,
          hoveredEntity.position[2]
        ] as Vec3
      }
    : null;

  return (
    <>
    <Canvas
      camera={{ position: cameraPosition, fov: cameraFov }}
      dpr={[1, 2]}
      onPointerMissed={() => setSelected(null)}
    >
      <color attach="background" args={["#020307"]} />
      <fog attach="fog" args={["#020307", 18, fogFar]} />
      <ambientLight intensity={wideCity ? 0.16 : 0.08} />
      <pointLight position={[0, 5, 2]} intensity={10} color="#c9dcff" />
      <Stars radius={starRadius} depth={30} count={900} factor={1.5} fade speed={0.1} />
      <Ground />
      {visibleEntities.map((entity) => (
        <Foundation key={`foundation-${entity.id}`} entity={entity} />
      ))}
      {visibleRelations.map((relation) => (
        <Bond
          key={relation.id}
          relation={relation}
          entities={groundedEntities}
          presentation={relationPresentation.get(relation.id)}
        />
      ))}
      {visibleEntities.map((entity) => (
        <Atom
          key={entity.id}
          entity={entity}
          presentation={entityPresentation.get(entity.id)}
          selected={selected === entity.id}
          onSelect={() => setSelected(entity.id)}
          onHover={setHovered}
          synchronized={universe.synchronized}
        />
      ))}
      {hoveredTooltip && (
        <Html
          position={hoveredTooltip.position}
          center
          zIndexRange={[100, 0]}
          style={{ pointerEvents: "none" }}
        >
          <div className="node-tooltip">
            <strong>{hoveredTooltip.presentation?.label ?? hoveredTooltip.id}</strong>
            {hoveredTooltip.presentation?.detail ? (
              <span className="node-tooltip__detail">
                {hoveredTooltip.presentation.detail}
              </span>
            ) : null}
            {hoveredTooltip.presentation ? (
              <span className="node-tooltip__meta">
                {hoveredTooltip.presentation.state} ·{" "}
                {hoveredTooltip.presentation.epistemic}
              </span>
            ) : null}
          </div>
        </Html>
      )}
      {[...universe.transfers.values()].map((transfer) => {
        const source = groundedEntities.get(transfer.source);
        const target = groundedEntities.get(transfer.target);
        if (!source || !target) return null;
        if (!visibleIds.has(transfer.source) || !visibleIds.has(transfer.target))
          return null;
        return (
          <EnergyTransferEffect
            key={transfer.transferId}
            transfer={transfer}
            source={source}
            target={target}
          />
        );
      })}
      <ObserverControls
        movementEnabled={!actorControl?.piloting}
        initialCamera={cameraPosition}
        initialTarget={focus?.target}
        groundHeight={terrainHeight}
        onPose={(pose) => {
          poseRef.current = pose;
        }}
      />
      {actorControl && (
        <ActorControls
          bounds={actorControl.bounds}
          piloting={actorControl.piloting}
          onMove={actorControl.onMove}
        />
      )}
    </Canvas>
    {totalNodes > 12 && (
      <Minimap
        entities={universe.entities}
        poseRef={poseRef}
        cityRadius={cityRadius}
      />
    )}
    {showFilter && (
      <div
        className="node-filter"
        style={{
          position: "absolute",
          right: 16,
          bottom: 16,
          zIndex: 30,
          display: "flex",
          flexDirection: "column",
          gap: 4,
          width: 210,
          padding: "10px 12px",
          background: "rgba(4,8,16,0.72)",
          border: "1px solid #22406e",
          borderRadius: 8,
          color: "#c9dcff",
          font: "12px/1.4 system-ui, sans-serif",
          backdropFilter: "blur(4px)"
        }}
      >
        <label style={{ display: "flex", justifyContent: "space-between" }}>
          <span>Voisinage borné</span>
          <strong>
            {visibleEntities.length}/{totalNodes}
          </strong>
        </label>
        <input
          type="range"
          min={filterMin}
          max={totalNodes}
          step={1}
          value={keep}
          onChange={(event) => {
            const value = Number(event.target.value);
            setKeepCount(value >= totalNodes ? null : value);
          }}
        />
        <small style={{ opacity: 0.7 }}>
          {keepCount == null
            ? "ville entière"
            : `${keep} plus proches · plus distants masqués`}
        </small>
      </div>
    )}
    </>
  );
}

