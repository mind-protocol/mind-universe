// Materializer: ontology-registry store  ->  desktop city fixture.
//
// Reads the REAL graph slice at
//   artifacts/ontology-registry/current/store/content-0.jsonl
// and emits a derived, deterministic city projection at
//   apps/mind-desktop/src/fixtures/ontology-registry.viz.json
//
// This is the compile-scene step of space:mind-universe:ontology3d:v1, done at
// author time (mirroring how postgres-identity-pilot.viz.json is a materialized
// projection of its store). The graph stays the source of truth; this file is a
// deterministic materialization — same store in, byte-identical fixture out.
//
// Run: node apps/mind-desktop/scripts/materialize-ontology-registry.mjs

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const STORE = resolve(
  repoRoot,
  "artifacts/ontology-registry/current/store/content-0.jsonl"
);
const OUT = resolve(here, "../src/fixtures/ontology-registry.viz.json");

// ---- parse ----------------------------------------------------------------
const records = readFileSync(STORE, "utf8")
  .split(/\r?\n/)
  .filter(Boolean)
  .map((line) => JSON.parse(line));

// Output key of a content document — how a node is addressed in the emitted city:
// an explicit id, else kind::canonical_id, else unaddressable (null). Shared by the
// node table below and the Built-position pass so both agree on node identity.
const outputKeyOf = (o) =>
  o.id ?? (o.canonical_id ? `${o.kind}::${o.canonical_id}` : null);

const isEdge = (o) =>
  (typeof o.source_id === "string" && typeof o.target_id === "string") ||
  (typeof o.source_key === "string" && typeof o.target_key === "string");

// Nodes are addressable records that are not edges. Dedupe by key, keeping the
// last occurrence (the store appends superseding versions).
const nodeByKey = new Map();
for (const o of records) {
  if (isEdge(o)) continue;
  const key = outputKeyOf(o);
  if (!key) continue;
  nodeByKey.set(key, { key, id: o.id, raw: o });
}

// ---- authored (Built) positions ------------------------------------------
// The city DERIVES every position from the force layout below. But a node may
// carry an AUTHORED position in the graph — a `built_position` reached via a
// HAS_POSITION relation — and Built must beat Derived. This is the JS mirror of
// the Rust `override_with_built`: read the authored {x,y,z} here, then override
// the derived coordinate of any rendered node that has one. A node with no
// authored position keeps its derived value; we never invent one.
//
// HAS_POSITION, the entity keys, and the relation live in the store's transaction
// layer (snapshot.json base + events.jsonl mutations), NOT in content-0.jsonl. So
// reconstruct the minimal slice the same way `UniverseStore::open` + `replay` does:
// symbols (base + interned), entities (key -> content pointer), relations. The
// predicate is resolved by NAME via the symbol table — never a hardcoded numeric id.
const STORE_DIR = dirname(STORE);
const contentBytes = readFileSync(STORE); // raw bytes: content pointers are byte offsets
const builtByOutputKey = new Map(); // rendered node key -> [x,y,z] (Built, wins over Derived)
const builtReport = []; // every authored position, rendered or not (for honest logging)
try {
  const snapshot = JSON.parse(readFileSync(resolve(STORE_DIR, "snapshot.json"), "utf8"));
  const symbols = [...(snapshot.symbols ?? [])];
  const entityByKey = new Map((snapshot.entities ?? []).map((e) => [e.key, e]));
  const relations = [...(snapshot.relations ?? [])];
  // Replay events.jsonl (the kernel write verbs): intern_symbols / put_entity /
  // put_relation, honoring tombstone_relation if present.
  let eventsText = "";
  try {
    eventsText = readFileSync(resolve(STORE_DIR, "events.jsonl"), "utf8");
  } catch {
    eventsText = "";
  }
  for (const line of eventsText.split(/\r?\n/).filter(Boolean)) {
    const mutations = JSON.parse(line)?.envelope?.payload?.mutations ?? [];
    for (const m of mutations) {
      if (m.kind === "intern_symbols") symbols.push(...(m.symbols ?? []));
      else if (m.kind === "put_entity" && m.entity) entityByKey.set(m.entity.key, m.entity);
      else if (m.kind === "put_relation" && m.relation) relations.push(m.relation);
      else if (m.kind === "tombstone_relation" && m.relation) {
        const i = relations.findIndex((r) => r.key === m.relation.key);
        if (i >= 0) relations.splice(i, 1);
      }
    }
  }
  const hasPosition = symbols.indexOf("HAS_POSITION");
  const readDoc = (entity) => {
    const p = entity?.content?.pointer;
    if (!p || p.segment !== 0) return null; // v1 store is single-segment (content-0)
    try {
      return JSON.parse(contentBytes.subarray(p.offset, p.offset + p.length).toString("utf8"));
    } catch {
      return null;
    }
  };
  const finite = (v) => typeof v === "number" && Number.isFinite(v);
  if (hasPosition >= 0) {
    for (const r of relations) {
      if (r.predicate !== hasPosition) continue;
      const target = readDoc(entityByKey.get(r.target));
      if (!target || !finite(target.x) || !finite(target.y) || !finite(target.z)) continue; // never invent
      const built = [target.x, target.y, target.z];
      // Translate the source's store key into the output keyspace this materializer
      // renders in (id / kind::canonical_id), via the source entity's content doc.
      const sourceDoc = readDoc(entityByKey.get(r.source));
      const outputKey = sourceDoc ? outputKeyOf(sourceDoc) : null;
      builtReport.push({ source: r.source, outputKey, built });
      if (outputKey) builtByOutputKey.set(outputKey, built);
    }
  }
} catch (error) {
  // Missing/partial transaction layer => no authored positions to honor. The derived
  // layout stands on its own; do not fail the whole materialization over it.
  console.warn(`built-position pass skipped: ${error.message}`);
}

// Canonical predicate -> physical_profile (ALIGN.md authority A). The single
// truth for how a link looks: family, polarity [p_ab,p_ba], hierarchy, permanence,
// mode. The renderer projects these onto orthogonal channels; it never invents a
// taxonomy. Predicates without a profile stay honestly unresolved (physics: null).
const profileByPredicate = new Map();
for (const o of records) {
  if (o.kind !== "physical_profile" || !o.profile) continue;
  const key = o.canonical_id ?? o.profile.source;
  if (!key) continue;
  const p = o.profile;
  profileByPredicate.set(key, {
    family: p.family ?? "unknown",
    polarity: Array.isArray(p.polarity) ? [p.polarity[0], p.polarity[1]] : [0, 0],
    hierarchy: typeof p.hierarchy === "number" ? p.hierarchy : 0,
    permanence: typeof p.permanence === "number" ? p.permanence : 0.5,
    mode: p.mode ?? "axis",
    calibrated: o.status !== "prototype_not_calibrated"
  });
}

// Edges with resolvable id endpoints (both sides are known node ids). key-edges
// (acoustic bindings in a different keyspace) are out of scope for v1.
const nodeIds = new Set(
  [...nodeByKey.values()].filter((n) => n.id).map((n) => n.id)
);
const edges = [];
const seenEdge = new Set();
for (const o of records) {
  if (!o.source_id || !o.target_id) continue;
  if (!nodeIds.has(o.source_id) || !nodeIds.has(o.target_id)) continue;
  const predicate = o.predicate ?? "RELATED_TO";
  let id = `${o.source_id}--${predicate}-->${o.target_id}`;
  let n = 1;
  while (seenEdge.has(id)) id = `${o.source_id}--${predicate}#${++n}-->${o.target_id}`;
  seenEdge.add(id);
  edges.push({
    id,
    source: o.source_id,
    target: o.target_id,
    predicate,
    physics: profileByPredicate.get(predicate) ?? null
  });
}

// ---- districts ------------------------------------------------------------
// Kind family -> district. Ordered so the two largest quarters (canon, physics)
// are not adjacent on the ring.
function districtOf(kind = "") {
  const k = kind.toLowerCase();
  if (k.includes("audio") || k.includes("acoustic")) return "acoustic";
  if (k.includes("physical_profile")) return "physics";
  if (k.startsWith("ontology_")) return "canon"; // canonical ontology defs/contracts
  if (k.includes("code")) return "code";
  if (k.includes("data_contract") || k.includes("schema")) return "schema";
  if (k.includes("visual_mapping") || k === "mapping" || k.includes("_mapping"))
    return "mapping";
  if (k.includes("contract")) return "contract";
  if (
    k.includes("validation") || k.includes("metric") || k.includes("health") ||
    k.includes("gap") || k.includes("task") || k.includes("receipt") ||
    k.includes("changeset") || k.includes("outcome") || k.includes("problem")
  )
    return "governance";
  if (
    k.includes("loop") || k.includes("objective") || k.includes("pattern") ||
    k.includes("policy") || k === "space"
  )
    return "civic";
  return "outskirts";
}

const DISTRICT_ORDER = [
  "civic", "canon", "code", "physics", "mapping",
  "governance", "schema", "acoustic", "contract", "outskirts"
];

const DISTRICT_STYLE = {
  civic: { color: "#6fa8ff", primitive: "bounded_volume", motion: "boundary_breath", scale: 1.05 },
  canon: { color: "#9b8fb0", primitive: "open_polyhedral_attractor", motion: "still", scale: 0.5 },
  code: { color: "#c68adf", primitive: "faceted_router", motion: "port_transform", scale: 0.72 },
  physics: { color: "#d86a5a", primitive: "torus_knot", motion: "still", scale: 0.55 },
  mapping: { color: "#3fb7c9", primitive: "oriented_ring", motion: "inward_orbit", scale: 0.7 },
  governance: { color: "#66c07a", primitive: "pulsing_core", motion: "outward_pulse", scale: 0.66 },
  schema: { color: "#8fd6ff", primitive: "cylinder", motion: "still", scale: 0.7 },
  acoustic: { color: "#f0c674", primitive: "tetrahedron", motion: "still", scale: 0.62 },
  contract: { color: "#e0a24a", primitive: "slab", motion: "still", scale: 0.72 },
  outskirts: { color: "#5a616e", primitive: "unknown", motion: "still", scale: 0.6 }
};

// ---- epistemic ------------------------------------------------------------
function epistemicOf(raw) {
  const s = (raw.information_status ?? raw.status ?? "").toLowerCase();
  if (s.startsWith("observed")) return "observed";
  if (s.startsWith("measured")) return "measured";
  if (s.includes("known_absent")) return "known_absent";
  if (s.includes("designed") || s.includes("prototype") || s.includes("ready"))
    return "not_measured";
  if (s.includes("failed")) return "measurement_failed";
  return "unknown";
}

// ---- layout ---------------------------------------------------------------
// Each district gets a center on a ring; members fill a local sunflower plot.
// Deterministic: no randomness, so the same store yields the same city.
const RING = 40;
const GOLDEN = Math.PI * (3 - Math.sqrt(5));

const grouped = new Map(DISTRICT_ORDER.map((d) => [d, []]));
for (const node of nodeByKey.values()) {
  grouped.get(districtOf(node.raw.kind)).push(node);
}
for (const list of grouped.values()) list.sort((a, b) => a.key.localeCompare(b.key));

const entities = [];
DISTRICT_ORDER.forEach((district, di) => {
  const list = grouped.get(district);
  if (list.length === 0) return;
  const angle = (di / DISTRICT_ORDER.length) * Math.PI * 2;
  const cx = Math.cos(angle) * RING;
  const cz = Math.sin(angle) * RING;
  const localSpacing = 1.1;
  const style = DISTRICT_STYLE[district];
  list.forEach((node, k) => {
    const raw = node.raw;
    // The root loop anchors the plaza at the exact city centre.
    const isRoot = raw.kind === "ontology3d_loop" && raw.id?.startsWith("space:");
    const r = localSpacing * Math.sqrt(k + 0.5);
    const theta = k * GOLDEN;
    const x = isRoot ? 0 : cx + Math.cos(theta) * r;
    const z = isRoot ? 0 : cz + Math.sin(theta) * r;
    entities.push({
      id: node.id ?? node.key,
      district,
      x: Number(x.toFixed(3)),
      z: Number(z.toFixed(3)),
      primitive: style.primitive,
      motion: style.motion,
      color: style.color,
      emissive: style.color,
      emissiveIntensity: 0.35,
      opacity: 0.92,
      scale: isRoot ? 1.6 : style.scale,
      epistemic: epistemicOf(raw),
      label: raw.name ?? raw.canonical_id ?? node.id ?? node.key,
      state: raw.status ?? raw.information_status ?? raw.kind,
      detail: `${raw.kind}${raw.purpose ? ` · ${raw.purpose}` : ""}`.slice(0, 180)
    });
  });
});

// ---- Built beats Derived --------------------------------------------------
// Override the derived plane coordinates of any rendered node that carries an
// authored Built position. The city is a top-down X/Z plane, so the Built {x,z}
// win directly in the final output (Built y is the ground axis, unused by this
// projection). Nodes without a Built position keep their derived coordinate.
const appliedKeys = new Set();
for (const entity of entities) {
  const built = builtByOutputKey.get(entity.id);
  if (!built) continue;
  entity.x = Number(built[0].toFixed(3));
  entity.z = Number(built[2].toFixed(3));
  appliedKeys.add(entity.id);
}

const fixture = {
  fixtureVersion: 0,
  source: "artifacts/ontology-registry/current/store/content-0.jsonl",
  title: "Ontologie 3D — registre canonique",
  subtitle: `${entities.length} nodes · ${edges.length} relations · ville-jardin ontology3d`,
  nodeCount: entities.length,
  edgeCount: edges.length,
  entities,
  relations: edges
};

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, JSON.stringify(fixture, null, 1) + "\n");
console.log(
  `materialized ${entities.length} nodes + ${edges.length} relations -> ${OUT}`
);
console.log(
  `built positions authored in graph: ${builtReport.length}; applied to rendered nodes: ${appliedKeys.size}`
);
for (const b of builtReport) {
  const applied = b.outputKey && appliedKeys.has(b.outputKey);
  const status = applied
    ? `APPLIED -> node "${b.outputKey}" set to x=${b.built[0]}, z=${b.built[2]}`
    : `node not in rendered set (source key ${b.source} resolves to ` +
      `${b.outputKey ? `"${b.outputKey}"` : "an unaddressable content doc, no id/canonical_id"}` +
      `) — override map built, no fake effect applied`;
  console.log(`  Built [${b.built.join(", ")}]: ${status}`);
}
