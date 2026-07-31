import rawFixture from "./fixtures/postgres-identity-pilot.viz.json";
import type {
  EntityVisualDescriptor,
  EpistemicState,
  MaterializedEntity,
  MaterializedRelation,
  RelationVisualDescriptor,
  UniverseEvent,
  Vector3
} from "./contracts";
import {
  applyUniverseEvent,
  emptyUniverseView,
  type UniverseView
} from "./universe-state";
import type { PhysicalProfile } from "./relation-infrastructure";
import { NEUTRAL_DYNAMICS } from "./entity-dynamics";

export type ProjectionState = "complete" | "partial" | "stale";

export interface MeasurementPresentation {
  readonly label: string;
  readonly state: EpistemicState;
  readonly detail: string;
}

export interface EntityPresentation {
  readonly label: string;
  readonly detail: string;
  readonly epistemic: EpistemicState;
  readonly state: string;
}

export interface RelationPresentation {
  readonly label: string | null;
  // The exact graph predicate, preserved for provenance and for classifying the
  // relation into an ontology3d infrastructure family — kept even when the label
  // is hidden, so an unlabelled street still knows what kind of street it is.
  readonly predicate: string;
  // Canonical physical_profile of the predicate (ALIGN.md authority A), when the
  // source can resolve it. Present ⇒ the bond derives from canonical channels;
  // absent ⇒ the renderer falls back to the predicate family and stays honest.
  readonly physics?: PhysicalProfile;
}

export interface OfflineProjectionAuthority {
  readonly authoritative: false;
  readonly mode: "offline_verified_projection";
  readonly derivedFrom: string;
  readonly universe: string;
  readonly revision: number;
  readonly tick: number;
  readonly preReceiptSnapshotHash: string;
}

export interface PostgresPilotProjection {
  readonly authority: OfflineProjectionAuthority;
  readonly projection: {
    readonly boundedSituation: ProjectionState;
    readonly universeCoverage: ProjectionState;
    readonly streamFreshness: ProjectionState;
    readonly productionTransport: EpistemicState;
  };
  readonly measurements: readonly MeasurementPresentation[];
  readonly view: UniverseView;
  readonly entityPresentation: ReadonlyMap<string, EntityPresentation>;
  readonly relationPresentation: ReadonlyMap<string, RelationPresentation>;
}

interface RawScaffoldEntity {
  readonly id: string;
  readonly generation: number;
  readonly visualKey: string;
  readonly position: Vector3;
  readonly label: string;
  readonly detail: string;
  readonly epistemic: EpistemicState;
  readonly state: string;
}

interface RawRelation {
  readonly id: string;
  readonly source: string;
  readonly target: string;
  readonly predicate: string;
  readonly showLabel: boolean;
}

interface RawRecord {
  readonly identityId: string;
  readonly assetId: string;
  readonly mapRelationId: string;
  readonly batchRelationId: string;
  readonly mapPredicate: string;
  readonly batchPredicate: string;
  readonly sourceId: string;
  readonly sourceRevision: number;
  readonly sourceStatus: string | null;
}

interface RawFixture {
  readonly fixtureVersion: 0;
  readonly batchId: string;
  readonly authority: OfflineProjectionAuthority;
  readonly projection: PostgresPilotProjection["projection"];
  readonly measurements: readonly MeasurementPresentation[];
  readonly visualMappings: Readonly<Record<string, EntityVisualDescriptor>>;
  readonly relationVisual: RelationVisualDescriptor;
  readonly scaffold: readonly RawScaffoldEntity[];
  readonly scaffoldRelations: readonly RawRelation[];
  readonly records: readonly RawRecord[];
}

const fixture = rawFixture as unknown as RawFixture;

function ringPosition(index: number, radius: number): Vector3 {
  const angle = (index / fixture.records.length) * Math.PI * 2 - Math.PI / 2;
  return [
    Math.cos(angle) * radius,
    Math.sin(angle) * radius * 0.62,
    index % 2 === 0 ? 0.7 : -0.15
  ];
}

function visualFor(key: string): EntityVisualDescriptor {
  const descriptor = fixture.visualMappings[key];
  if (!descriptor) {
    throw new Error(`fixture visual mapping is missing: ${key}`);
  }
  return descriptor;
}

function entityEvent(
  sequence: number,
  entity: Omit<MaterializedEntity, "dynamics">
): UniverseEvent {
  return {
    version: 0,
    sequence,
    kind: "entity_materialized",
    entity: { ...entity, dynamics: NEUTRAL_DYNAMICS }
  };
}

function relationEvent(
  sequence: number,
  relation: MaterializedRelation
): UniverseEvent {
  return {
    version: 0,
    sequence,
    kind: "relation_materialized",
    relation
  };
}

export function buildPostgresPilotProjection(): PostgresPilotProjection {
  if (fixture.fixtureVersion !== 0 || fixture.authority.authoritative !== false) {
    throw new Error("the PostgreSQL Viz fixture must remain non-authoritative");
  }
  if (fixture.records.length !== 10) {
    throw new Error("the verified identity pilot must contain exactly 10 records");
  }

  const events: UniverseEvent[] = [
    {
      version: 0,
      sequence: 0,
      kind: "snapshot_started",
      revision: fixture.authority.revision
    }
  ];
  const entityPresentation = new Map<string, EntityPresentation>();
  const relationPresentation = new Map<string, RelationPresentation>();

  for (const scaffold of fixture.scaffold) {
    events.push(
      entityEvent(events.length, {
        id: scaffold.id,
        generation: scaffold.generation,
        position: scaffold.position,
        visual: visualFor(scaffold.visualKey)
      })
    );
    entityPresentation.set(scaffold.id, {
      label: scaffold.label,
      detail: scaffold.detail,
      epistemic: scaffold.epistemic,
      state: scaffold.state
    });
  }

  fixture.records.forEach((record, index) => {
    const identityPosition = ringPosition(index, 6.2);
    const assetPosition = ringPosition(index, 4.1);
    events.push(
      entityEvent(events.length, {
        id: record.identityId,
        generation: 0,
        position: identityPosition,
        visual: visualFor("identity")
      }),
      entityEvent(events.length + 1, {
        id: record.assetId,
        generation: 0,
        position: assetPosition,
        visual: visualFor("asset")
      })
    );
    const ordinal = String(index + 1).padStart(2, "0");
    entityPresentation.set(record.identityId, {
      label: `Identity ${ordinal}`,
      detail: `${record.sourceId} · source revision ${record.sourceRevision}`,
      epistemic: "observed",
      state: "resolved_for_source_revision"
    });
    entityPresentation.set(record.assetId, {
      label: `Inert asset ${ordinal}`,
      detail: `source status ${record.sourceStatus ?? "unknown"} · executable false · ontology false`,
      epistemic: "measured",
      state: "imported_inert"
    });
  });

  const materializeRelation = (relation: RawRelation) => {
    events.push(
      relationEvent(events.length, {
        id: relation.id,
        source: relation.source,
        target: relation.target,
        visual: fixture.relationVisual
      })
    );
    relationPresentation.set(relation.id, {
      label: relation.showLabel ? relation.predicate : null,
      predicate: relation.predicate
    });
  };

  fixture.scaffoldRelations.forEach(materializeRelation);
  fixture.records.forEach((record) => {
    materializeRelation({
      id: record.mapRelationId,
      source: record.identityId,
      target: record.assetId,
      predicate: record.mapPredicate,
      showLabel: true
    });
    materializeRelation({
      id: record.batchRelationId,
      source: record.assetId,
      target: fixture.batchId,
      predicate: record.batchPredicate,
      showLabel: false
    });
  });

  const view = events.reduce(applyUniverseEvent, emptyUniverseView());
  if (!view.synchronized) {
    throw new Error("the deterministic PostgreSQL fixture contains a sequence gap");
  }

  return {
    authority: fixture.authority,
    projection: fixture.projection,
    measurements: fixture.measurements,
    view,
    entityPresentation,
    relationPresentation
  };
}

export const postgresPilotProjection = buildPostgresPilotProjection();
