import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import rawFixture from "./fixtures/postgres-identity-pilot.viz.json";
import { buildPostgresPilotProjection } from "./postgres-pilot-fixture";

const verifiedStore = new URL(
  "../../../artifacts/postgres-import/identity-pilot-20260730-001/store-verified/",
  import.meta.url
);

describe("verified PostgreSQL identity pilot projection", () => {
  it("is pinned to independently readable records in the verified store", () => {
    const snapshot = JSON.parse(
      readFileSync(new URL("snapshot.json", verifiedStore), "utf8")
    ) as {
      universe: string;
      symbols: string[];
      entities: Array<{
        key: string;
        content: { pointer: { offset: number; length: number } };
      }>;
      relations: Array<{
        key: string;
        source: string;
        target: string;
        predicate: number;
      }>;
    };
    const event = JSON.parse(
      readFileSync(new URL("events.jsonl", verifiedStore), "utf8").trim()
    ) as {
      envelope: {
        revision: number;
        tick: number;
        payload: {
          mutations: Array<
            | {
                kind: "put_entity";
                entity: {
                  key: string;
                  content: { pointer: { offset: number; length: number } };
                };
              }
            | {
                kind: "put_relation";
                relation: {
                  key: string;
                  source: string;
                  target: string;
                  predicate: number;
                };
              }
          >;
        };
      };
    };
    const content = readFileSync(
      new URL("content-0.jsonl", verifiedStore)
    );
    const eventEntities = event.envelope.payload.mutations
      .filter((mutation) => mutation.kind === "put_entity")
      .map((mutation) => mutation.entity);
    const eventRelations = event.envelope.payload.mutations
      .filter((mutation) => mutation.kind === "put_relation")
      .map((mutation) => mutation.relation);
    const entities = new Map(
      [...snapshot.entities, ...eventEntities].map((entity) => [
        entity.key,
        entity
      ])
    );
    const relations = new Map(
      [...snapshot.relations, ...eventRelations].map((relation) => [
        relation.key,
        relation
      ])
    );
    const readEntityContent = (id: string) => {
      const entity = entities.get(id);
      expect(entity, `missing verified entity ${id}`).toBeDefined();
      const { offset, length } = entity!.content.pointer;
      return JSON.parse(
        content.subarray(offset, offset + length).toString("utf8").trim()
      ) as Record<string, unknown>;
    };
    const expectRelation = (
      id: string,
      source: string,
      target: string,
      predicate: string
    ) => {
      const relation = relations.get(id);
      expect(relation, `missing verified relation ${id}`).toBeDefined();
      expect(relation).toMatchObject({ source, target });
      expect(snapshot.symbols[relation!.predicate]).toBe(predicate);
    };

    expect(snapshot.universe).toBe(rawFixture.authority.universe);
    expect(event.envelope.revision).toBe(rawFixture.authority.revision);
    expect(event.envelope.tick).toBe(rawFixture.authority.tick);

    for (const relation of rawFixture.scaffoldRelations) {
      expectRelation(
        relation.id,
        relation.source,
        relation.target,
        relation.predicate
      );
    }
    for (const record of rawFixture.records) {
      expect(readEntityContent(record.identityId)).toMatchObject({
        source_id: record.sourceId,
        target: record.assetId,
        status: "resolved_for_source_revision"
      });
      expect(readEntityContent(record.assetId)).toMatchObject({
        source_id: record.sourceId,
        source_revision: record.sourceRevision,
        source_status: record.sourceStatus,
        target_status: "imported_inert",
        executable: false,
        ontology_activated: false,
        payload_imported: false
      });
      expectRelation(
        record.mapRelationId,
        record.identityId,
        record.assetId,
        record.mapPredicate
      );
      expectRelation(
        record.batchRelationId,
        record.assetId,
        rawFixture.batchId,
        record.batchPredicate
      );
    }

    expect(
      readEntityContent("00000000000000000000000000005005")
    ).toMatchObject({
      batch_id: "postgres-identity-pilot-20260730-001",
      content_records_read_back: 49,
      imported_nodes: 10,
      executable_nodes: 0,
      ontology_activated: false,
      information_status: "measured",
      pre_receipt_snapshot_hash:
        rawFixture.authority.preReceiptSnapshotHash
    });
  });

  it("replays the complete bounded batch without claiming Universe completeness", () => {
    const projection = buildPostgresPilotProjection();

    expect(projection.authority.authoritative).toBe(false);
    expect(projection.authority.mode).toBe("offline_verified_projection");
    expect(projection.projection.boundedSituation).toBe("complete");
    expect(projection.projection.universeCoverage).toBe("partial");
    expect(projection.projection.streamFreshness).toBe("stale");
    expect(projection.view.synchronized).toBe(true);
    expect(projection.view.revision).toBe(1);
  });

  it("shows all ten identity-to-inert-asset mappings and their provenance", () => {
    const projection = buildPostgresPilotProjection();
    const records = rawFixture.records;

    expect(records).toHaveLength(10);
    expect(projection.view.entities).toHaveLength(26);
    expect(projection.view.relations).toHaveLength(25);

    for (const record of records) {
      const identity = projection.entityPresentation.get(record.identityId);
      const asset = projection.entityPresentation.get(record.assetId);
      const mapRelation = projection.view.relations.get(record.mapRelationId);
      const batchRelation = projection.view.relations.get(record.batchRelationId);

      expect(identity?.state).toBe("resolved_for_source_revision");
      expect(identity?.detail).toContain(record.sourceId);
      expect(asset?.state).toBe("imported_inert");
      expect(asset?.detail).toContain("executable false");
      expect(asset?.detail).toContain("ontology false");
      expect(mapRelation).toMatchObject({
        source: record.identityId,
        target: record.assetId
      });
      expect(batchRelation).toMatchObject({
        source: record.assetId,
        target: rawFixture.batchId
      });
    }
  });

  it("keeps measurement failure distinct from absence and success", () => {
    const projection = buildPostgresPilotProjection();
    const relationIntegrity = projection.measurements.find(
      (measurement) => measurement.label === "global relation integrity"
    );
    const importedCode = projection.measurements.find(
      (measurement) => measurement.label === "imported code"
    );
    const sourceStatus = projection.measurements.find(
      (measurement) => measurement.label === "source status"
    );

    expect(relationIntegrity?.state).toBe("measurement_failed");
    expect(relationIntegrity?.detail).toContain("not a global proof");
    expect(importedCode?.state).toBe("known_absent");
    expect(sourceStatus?.state).toBe("unknown");
  });

  it("takes predicate labels and all visual descriptors from fixture data", () => {
    const projection = buildPostgresPilotProjection();

    for (const relation of rawFixture.scaffoldRelations) {
      expect(
        projection.relationPresentation.get(relation.id)?.label
      ).toBe(relation.showLabel ? relation.predicate : null);
    }
    for (const record of rawFixture.records) {
      expect(
        projection.relationPresentation.get(record.mapRelationId)?.label
      ).toBe(record.mapPredicate);
      expect(
        projection.relationPresentation.get(record.batchRelationId)?.label
      ).toBeNull();
    }
  });
});
