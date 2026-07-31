// Shared city materializer: the LIVE ontology-registry store -> desktop city.
//
// The store is snapshot + event log. The base `snapshot.json` is only revision 0;
// the world's CURRENT state is that snapshot with `events.jsonl` replayed on top
// (put/supersede/tombstone + interned symbols). Reading the snapshot alone is
// doubly frozen — a build-time JSON, AND stale by every event appended since. This
// module reads the store, replays the log to the current revision, resolves each
// record's content by byte pointer, and projects it into the deterministic
// ville-jardin city (districts + sunflower layout + per-district style).
//
// It is the single source shared by BOTH carriers so they cannot diverge:
//   - materialize-ontology-registry.mjs writes the offline fallback fixture, and
//   - vite-plugin-universe-stream.mjs streams the live store over SSE.
//
// Same store in => same city out (no randomness, no clock).

import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

// ---- read the store: snapshot + replayed event log ------------------------
function readStore(storeDir) {
  const snapshot = JSON.parse(
    readFileSync(resolve(storeDir, "snapshot.json"), "utf8")
  );
  const symbols = [...snapshot.symbols];
  const entities = new Map(); // key -> { key, symbol, content }
  for (const entity of snapshot.entities) entities.set(entity.key, entity);
  const relations = new Map(); // key -> { key, source, target, predicate, content }
  for (const relation of snapshot.relations) relations.set(relation.key, relation);
  let revision = snapshot.revision ?? 0;

  const eventsPath = resolve(storeDir, "events.jsonl");
  if (existsSync(eventsPath)) {
    const lines = readFileSync(eventsPath, "utf8").split(/\r?\n/).filter(Boolean);
    for (const line of lines) {
      const envelope = JSON.parse(line).envelope;
      if (typeof envelope.revision === "number") revision = envelope.revision;
      const payload = envelope.payload;
      const mutations = payload.kind === "batch" ? payload.mutations : [payload];
      for (const mutation of mutations) {
        applyMutation(mutation, { symbols, entities, relations });
      }
    }
  }
  return { storeDir, symbols, entities, relations, revision };
}

// Replay one kernel mutation onto the working state. Only the record-shaping
// mutations matter for the city projection; anything else is honestly ignored
// (it never fabricates a node/edge it does not understand).
function applyMutation(mutation, state) {
  switch (mutation.kind) {
    case "intern_symbols":
      for (const name of mutation.symbols ?? []) state.symbols.push(name);
      break;
    case "put_entity":
    case "supersede_entity":
      if (mutation.entity?.key) state.entities.set(mutation.entity.key, mutation.entity);
      break;
    case "tombstone_entity":
      if (typeof mutation.entity === "string") state.entities.delete(mutation.entity);
      break;
    case "put_relation":
      if (mutation.relation?.key) state.relations.set(mutation.relation.key, mutation.relation);
      break;
    case "tombstone_relation":
      if (typeof mutation.relation === "string") state.relations.delete(mutation.relation);
      break;
    default:
      break;
  }
}

// Content pointers address a byte slice of a content segment. Resolve one to its
// parsed JSON payload, lazily loading (and caching) each segment as a raw buffer
// so offsets are honoured exactly (the segment is NOT re-serialized JSONL).
function makeContentResolver(storeDir) {
  const segmentBuffers = new Map();
  return (content) => {
    const pointer = content?.pointer;
    if (!pointer) return {};
    let buffer = segmentBuffers.get(pointer.segment);
    if (!buffer) {
      buffer = readFileSync(resolve(storeDir, `content-${pointer.segment}.jsonl`));
      segmentBuffers.set(pointer.segment, buffer);
    }
    const slice = buffer.subarray(pointer.offset, pointer.offset + pointer.length);
    return JSON.parse(slice.toString("utf8"));
  };
}

// ---- districts (data-sourced, never embedded policy) ----------------------
// The kind -> district assignment and the per-district visual style are NOT
// code here. CLAUDE.md forbids bootstrap/native code from carrying "a generated
// semantic layout for a city" or "one privileged visual interpretation of a
// semantic type or predicate". The projector is a mechanism only: it loads an
// authored `city_layout_policy` DATA asset and applies its ordered rules and
// style map generically. Changing the metaphor or the placement means editing
// the data file, never this script. (The store carries no district asset yet;
// when it does, `loadLayoutPolicy` can resolve it from the graph instead.)
const POLICY_PATH = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "city-layout-policy.json"
);

function loadLayoutPolicy() {
  const policy = JSON.parse(readFileSync(POLICY_PATH, "utf8"));
  if (!Array.isArray(policy.district_order) || !policy.styles) {
    throw new Error(`city-layout-policy.json is malformed: ${POLICY_PATH}`);
  }
  return policy;
}

// Assign a kind to a district by walking the policy's ordered rules: first rule
// whose `includes` (substring), `startsWith` (prefix) or `equals` (exact) hits
// wins; otherwise the declared default. Pure data interpretation — no metaphor.
function districtForKind(policy, kind = "") {
  const k = String(kind).toLowerCase();
  for (const rule of policy.rules ?? []) {
    const includes = rule.includes ?? [];
    const startsWith = rule.startsWith ?? [];
    const equals = rule.equals ?? [];
    if (
      includes.some((s) => k.includes(s)) ||
      startsWith.some((s) => k.startsWith(s)) ||
      equals.some((s) => k === s)
    ) {
      return rule.district;
    }
  }
  return policy.default_district ?? "outskirts";
}

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

// A readable name for a building. The store carries no free-text `name`; the
// canonical_id is the stable public handle, the executable label a friendlier
// gloss, and the symbol type the last honest fallback before the raw handle.
function labelOf(node) {
  const o = node.content;
  return (
    o.canonical_id ??
    o.executable?.label ??
    o.executable?.id ??
    node.type ??
    node.key
  );
}

// ---- layout ---------------------------------------------------------------
const RING = 40;
const GOLDEN = Math.PI * (3 - Math.sqrt(5));

/**
 * Materialize the live store at `storeDir` into the deterministic desktop city
 * (the ontology-registry.viz.json schema), plus the current `revision`.
 */
export function materializeCity(storeDir) {
  const policy = loadLayoutPolicy();
  const DISTRICT_ORDER = policy.district_order;
  const DISTRICT_STYLE = policy.styles;
  const districtOf = (kind) => districtForKind(policy, kind);
  const store = readStore(storeDir);
  const resolveContent = makeContentResolver(store.storeDir);
  const symbolName = (index) =>
    (typeof index === "number" ? store.symbols[index] : undefined) ?? String(index);

  const nodes = [...store.entities.values()]
    .map((entity) => ({
      key: entity.key,
      type: symbolName(entity.symbol),
      content: resolveContent(entity.content)
    }))
    // The registry root (ontology_manifest) is a bookkeeping INDEX, not a place:
    // it sits at the city centre with every atom's PART_OF membership edge
    // converging on it, so the whole world reads as "encapsulated" inside one
    // space. It is not somewhere you stand and its inbound PART_OF edges are
    // membership, not streets — so it is omitted from the city projection.
    // Dropping the node also drops those edges (the endpoint check below skips a
    // relation whose target no longer exists), removing the converging-streets
    // clutter. The STORE is untouched; this is a view decision, reversible by
    // deleting this filter.
    .filter((node) => node.content.kind !== "ontology_manifest");
  const nodeKeys = new Set(nodes.map((node) => node.key));

  // Canonical predicate -> physical_profile (ALIGN.md authority A). The single
  // truth for how a link looks. Predicates without a profile stay honestly
  // unresolved (physics: null).
  const profileByPredicate = new Map();
  for (const node of nodes) {
    const o = node.content;
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

  // Edges: every relation whose endpoints both survive replay. The predicate is
  // the relation's symbol name; its physical profile (when one exists) feeds the
  // Bond renderer.
  const edges = [];
  const seenEdge = new Set();
  for (const relation of store.relations.values()) {
    const { source, target } = relation;
    if (!nodeKeys.has(source) || !nodeKeys.has(target)) continue;
    const predicate = symbolName(relation.predicate);
    let id = `${source}--${predicate}-->${target}`;
    let n = 1;
    while (seenEdge.has(id)) id = `${source}--${predicate}#${++n}-->${target}`;
    seenEdge.add(id);
    edges.push({
      id,
      source,
      target,
      predicate,
      physics: profileByPredicate.get(predicate) ?? null
    });
  }

  // Layout: each district gets a center on a ring; members fill a local sunflower
  // plot, sorted by key for determinism.
  const grouped = new Map(DISTRICT_ORDER.map((d) => [d, []]));
  for (const node of nodes) grouped.get(districtOf(node.content.kind)).push(node);
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
      const raw = node.content;
      const r = localSpacing * Math.sqrt(k + 0.5);
      const theta = k * GOLDEN;
      const x = cx + Math.cos(theta) * r;
      const z = cz + Math.sin(theta) * r;
      const description = raw.executable?.description;
      entities.push({
        id: node.key,
        district,
        x: Number(x.toFixed(3)),
        z: Number(z.toFixed(3)),
        primitive: style.primitive,
        motion: style.motion,
        color: style.color,
        emissive: style.color,
        emissiveIntensity: 0.35,
        opacity: 0.92,
        scale: style.scale,
        epistemic: epistemicOf(raw),
        label: labelOf(node),
        state: raw.status ?? raw.information_status ?? raw.kind,
        detail: `${raw.kind ?? node.type}${description ? ` · ${description}` : ""}`.slice(0, 180)
      });
    });
  });

  return {
    fixtureVersion: 0,
    source: "artifacts/ontology-registry/current/store (snapshot + events replay)",
    title: "Ontologie 3D — registre canonique",
    subtitle: `${entities.length} nodes · ${edges.length} relations · rev ${store.revision} · ville-jardin ontology3d`,
    revision: store.revision,
    nodeCount: entities.length,
    edgeCount: edges.length,
    entities,
    relations: edges
  };
}
