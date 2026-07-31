import { describe, expect, it } from "vitest";
import { ontologyRegistryProjection } from "./ontology-registry-fixture";
import { terrainHeight } from "./terrain";

const projection = ontologyRegistryProjection;

describe("ontologyRegistryProjection", () => {
  it("renders the whole kernel registry — far more than the 26-node pilot", () => {
    expect(projection.view.entities.size).toBeGreaterThan(200);
    expect(projection.nodeCount).toBe(projection.view.entities.size);
    // The registry is richly connected: its streets (relations) must survive
    // materialization. A city with 0 relations is the exact regression this
    // guards against — the materializer once read the content segment (which
    // carries no adjacency) instead of the structural snapshot. The absolute
    // count drifts as the live store grows and as view filters (e.g. the severed
    // registry root) drop edges, so the floor stays well below the current total.
    expect(projection.view.relations.size).toBeGreaterThan(300);
    expect(projection.edgeCount).toBe(projection.view.relations.size);
  });

  it("folds into a coherent (synchronized) universe", () => {
    expect(projection.view.synchronized).toBe(true);
  });

  it("every street connects two buildings that exist in the city", () => {
    for (const relation of projection.view.relations.values()) {
      expect(projection.view.entities.has(relation.source)).toBe(true);
      expect(projection.view.entities.has(relation.target)).toBe(true);
    }
    expect(projection.view.relations.size).toBe(projection.edgeCount);
  });

  it("sits every building on the land (y = terrain beneath it + lift)", () => {
    for (const entity of projection.view.entities.values()) {
      const [x, y, z] = entity.position;
      expect(Number.isFinite(x) && Number.isFinite(y) && Number.isFinite(z)).toBe(true);
      // Just above the terrain directly beneath — no floating in the void, no
      // sinking under the land.
      expect(y).toBeGreaterThan(terrainHeight(x, z));
      expect(y).toBeLessThan(terrainHeight(x, z) + 1.5);
    }
  });

  it("gives every building a label and every street a predicate for classification", () => {
    for (const [id] of projection.view.entities) {
      expect(projection.entityPresentation.get(id)?.label).toBeTruthy();
    }
    for (const [id] of projection.view.relations) {
      expect(projection.relationPresentation.get(id)?.predicate).toBeTruthy();
    }
  });
});
