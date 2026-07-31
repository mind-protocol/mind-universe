// Materializer: ontology-registry store  ->  desktop city fixture (OFFLINE fallback).
//
// Writes the deterministic city projection at
//   apps/mind-desktop/src/fixtures/ontology-registry.viz.json
// from the LIVE store (snapshot + events.jsonl replay) via the shared
// `materializeCity` projector. This baked fixture is only the offline fallback the
// app renders when the live SSE stream (vite-plugin-universe-stream) is absent —
// e.g. a production build, or a dev run against a store that cannot be read. The
// same projector feeds the live stream, so the two never diverge.
//
// Run: node apps/mind-desktop/scripts/materialize-ontology-registry.mjs

import { writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { materializeCity } from "./materialize-city.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const STORE_DIR = resolve(repoRoot, "artifacts/ontology-registry/current/store");
const OUT = resolve(here, "../src/fixtures/ontology-registry.viz.json");

const city = materializeCity(STORE_DIR);
// The baked fixture pins its provenance to the store path (the runtime stream
// carries the live revision in its own subtitle).
const fixture = {
  ...city,
  source: "artifacts/ontology-registry/current/store/snapshot.json + events.jsonl"
};

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, JSON.stringify(fixture, null, 1) + "\n");
console.log(
  `materialized ${city.nodeCount} nodes + ${city.edgeCount} relations (rev ${city.revision}) -> ${OUT}`
);
