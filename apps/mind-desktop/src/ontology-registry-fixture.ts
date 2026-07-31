import rawFixture from "./fixtures/ontology-registry.viz.json";
import type {
  EpistemicState,
  MaterializedEntity,
  RelationVisualDescriptor,
  UniverseEvent,
  Vector3
} from "./contracts";
import {
  applyUniverseEvent,
  emptyUniverseView,
  type UniverseView
} from "./universe-state";
import type {
  EntityPresentation,
  RelationPresentation
} from "./postgres-pilot-fixture";
import { terrainHeight } from "./terrain";
import type { EntityVisualPrimitive, EntityMotionPrimitive } from "./contracts";
import { NEUTRAL_DYNAMICS } from "./entity-dynamics";

// How far a building floats above the land it sits on. Small, so the foundation
// reads as a footing rather than a stalk (the lollipop problem).
const BUILDING_LIFT = 0.7;

interface RawCityEntity {
  readonly id: string;
  readonly district: string;
  readonly x: number;
  readonly z: number;
  readonly primitive: EntityVisualPrimitive;
  readonly motion: EntityMotionPrimitive;
  readonly color: string;
  readonly emissive: string;
  readonly emissiveIntensity: number;
  readonly opacity: number;
  readonly scale: number;
  readonly epistemic: EpistemicState;
  readonly label: string;
  readonly state: string;
  readonly detail: string;
}

interface RawCityRelation {
  readonly id: string;
  readonly source: string;
  readonly target: string;
  readonly predicate: string;
  readonly physics: {
    readonly family: string;
    readonly polarity: readonly [number, number];
    readonly hierarchy: number;
    readonly permanence: number;
    readonly mode: string;
    readonly calibrated: boolean;
  } | null;
}

interface RawCityFixture {
  readonly fixtureVersion: 0;
  readonly source: string;
  readonly title: string;
  readonly subtitle: string;
  readonly nodeCount: number;
  readonly edgeCount: number;
  readonly entities: readonly RawCityEntity[];
  readonly relations: readonly RawCityRelation[];
}

export interface OntologyRegistryProjection {
  readonly title: string;
  readonly subtitle: string;
  readonly source: string;
  readonly nodeCount: number;
  readonly edgeCount: number;
  readonly view: UniverseView;
  readonly entityPresentation: ReadonlyMap<string, EntityPresentation>;
  readonly relationPresentation: ReadonlyMap<string, RelationPresentation>;
}

const fixture = rawFixture as unknown as RawCityFixture;

// A single benign relation descriptor; the Bond renderer restyles each street by
// its predicate family, so the descriptor here is only a structural default.
const RELATION_VISUAL: RelationVisualDescriptor = {
  primitive: "dual_lane_bond",
  material: {
    color: "#7890b5",
    emissive: "#32496a",
    emissiveIntensity: 0.3,
    opacity: 0.45,
    scale: 1
  },
  width: 0.7,
  laneSeparation: 0.04
};

function buildProjection(): OntologyRegistryProjection {
  const events: UniverseEvent[] = [
    { version: 0, sequence: 0, kind: "snapshot_started", revision: 1 }
  ];
  const entityPresentation = new Map<string, EntityPresentation>();
  const relationPresentation = new Map<string, RelationPresentation>();

  for (const entity of fixture.entities) {
    const position: Vector3 = [
      entity.x,
      terrainHeight(entity.x, entity.z) + BUILDING_LIFT,
      entity.z
    ];
    const materialized: MaterializedEntity = {
      id: entity.id,
      generation: 0,
      position,
      visual: {
        primitive: entity.primitive,
        motion: entity.motion,
        material: {
          color: entity.color,
          emissive: entity.emissive,
          emissiveIntensity: entity.emissiveIntensity,
          opacity: entity.opacity,
          scale: entity.scale
        }
      },
      dynamics: NEUTRAL_DYNAMICS
    };
    events.push({
      version: 0,
      sequence: events.length,
      kind: "entity_materialized",
      entity: materialized
    });
    entityPresentation.set(entity.id, {
      label: entity.label,
      detail: entity.detail,
      epistemic: entity.epistemic,
      state: entity.state
    });
  }

  for (const relation of fixture.relations) {
    events.push({
      version: 0,
      sequence: events.length,
      kind: "relation_materialized",
      relation: {
        id: relation.id,
        source: relation.source,
        target: relation.target,
        visual: RELATION_VISUAL
      }
    });
    // Labels stay hidden (149 streets would be noise); the predicate is kept for
    // provenance and the canonical physical_profile (when resolved) so the Bond
    // renderer derives the link from the single canonical table (ALIGN.md §2).
    relationPresentation.set(relation.id, {
      label: null,
      predicate: relation.predicate,
      physics: relation.physics ?? undefined
    });
  }

  const view = events.reduce(applyUniverseEvent, emptyUniverseView());
  if (!view.synchronized) {
    throw new Error("the ontology-registry city fixture contains a sequence gap");
  }

  return {
    title: fixture.title,
    subtitle: fixture.subtitle,
    source: fixture.source,
    nodeCount: fixture.nodeCount,
    edgeCount: fixture.edgeCount,
    view,
    entityPresentation,
    relationPresentation
  };
}

export const ontologyRegistryProjection = buildProjection();
