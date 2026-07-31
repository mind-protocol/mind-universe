// Deterministic scene renderer for visual regression (G3 item 6). The live
// renderer is WebGL (React Three Fiber); GPU pixel/video diffing is a separate
// harness. This module renders the SAME UniverseView to a DETERMINISTIC SVG that
// actually resembles the 3D ontology: it projects positions with an oblique
// pseudo-3D camera (depth-sorted), and for each entity it draws the graph-
// resolved EMBODIMENT FORM — the energy core, the fresnel aura, the internal
// particles, and the semi-humanoid limbs — using the mapping's own palette, with
// epistemic modulation (a `measured` being glows; an `unknown` one is dim, no
// emission). No randomness: particle scatter is a golden-angle spiral by index.

import type { EmbodimentDynamics } from "./contracts";
import { deriveEntityDynamics, primitiveJitter } from "./entity-dynamics";
import type { UniverseView } from "./universe-state";
import resolutionPolicy from "../../../fixtures/assets/visual-resolution-policy-v1.json";

export interface SceneOptions {
  readonly width?: number;
  readonly height?: number;
  /** Overrides the health label (e.g. "stale", "degraded"). */
  readonly health?: string;
  /**
   * Lantern mode (the Cloître grammar): reveal epistemic status without moving.
   * A `measured` node stays clear and lit; an `unknown` / `not_measured` node
   * (no emission) is shrouded in Fog — barely visible, never rendered as if it
   * were measured. This makes the epistemic-honesty jewel legible on screen.
   */
  readonly lantern?: boolean;
}

type Vec3 = readonly [number, number, number];
type PrimitiveTuple = readonly [
  string, // primitive kind
  string, // role
  string, // material bucket: core | shell | particles
  Vec3, // offset
  Vec3, // rotation (unused in 2D projection)
  Vec3, // scale
  number, // count (points)
  number // radius (points)
];

const UNIT = 24; // world → screen units
const YAW = 0.62; // camera yaw (radians)
const TILT = 0.5; // vertical foreshortening of depth

function f(value: number): string {
  return Number.isFinite(value) ? value.toFixed(2) : "0.00";
}

interface Projected {
  readonly x: number;
  readonly y: number;
  readonly depth: number;
}

function project(p: Vec3, cx: number, cy: number): Projected {
  const sin = Math.sin(YAW);
  const cos = Math.cos(YAW);
  const px = p[0] * cos - p[2] * sin;
  const depth = p[0] * sin + p[2] * cos;
  return { x: cx + px * UNIT, y: cy - p[1] * UNIT + depth * TILT * UNIT, depth };
}

function add(a: Vec3, b: Vec3): Vec3 {
  return [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
}

function scaleVec(v: Vec3, k: number): Vec3 {
  return [v[0] * k, v[1] * k, v[2] * k];
}

function mulVec(a: Vec3, b: Vec3): Vec3 {
  return [a[0] * b[0], a[1] * b[1], a[2] * b[2]];
}

// Yaw around the vertical axis — the embedding-driven orientation. A zero yaw
// returns the vector untouched, so an un-oriented node is byte-identical to the
// un-modulated render (the identity path the committed goldens depend on).
function rotateY(v: Vec3, yaw: number): Vec3 {
  if (yaw === 0) return v;
  const c = Math.cos(yaw);
  const s = Math.sin(yaw);
  return [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c];
}

// Per-node modulation of a base form (see entity-dynamics.ts): weight → scale,
// energy → emission, embedding → orientation + procedural micro-variation.
interface BeingModulation {
  readonly scale: number;
  readonly yaw: number;
  readonly emissiveMultiplier: number;
  readonly microvariation: number;
  readonly embedding?: readonly number[];
}

const IDENTITY_MODULATION: BeingModulation = {
  scale: 1,
  yaw: 0,
  emissiveMultiplier: 1,
  microvariation: 0
};

interface Palette {
  readonly core: string;
  readonly emissive: string;
  readonly shell: string;
  readonly particle: string;
}

type ResolvedForm = { form: readonly PrimitiveTuple[]; palette: Palette };

// The honest bare-presence point (visual-resolution-policy-v1.json → fallback):
// a node with no producing-toolkit binding is drawn as a recognizably "unbound"
// point, NEVER a defaulted citizen and never an invented form. Provenance-based
// resolution's terminal case.
const FALLBACK_PRESENCE: ResolvedForm = {
  form: resolutionPolicy.fallback_presence.form as unknown as readonly PrimitiveTuple[],
  palette: resolutionPolicy.fallback_presence.palette as Palette
};

// A role-keyed toolkit binding (e.g. the underground toolkit,
// visual-embodiment/1-role-keyed): archetypes keyed by name, each declaring the
// role_axis / semantic types it dresses and its per-residency forms.
type RoleKeyedArchetype = {
  role_axis?: string;
  declared_for?: string[];
  fallback_form?: string;
  forms: Record<string, PrimitiveTuple[]>;
};
type RoleKeyedMapping = {
  archetypes: Record<string, RoleKeyedArchetype>;
  dormant_form?: Record<string, PrimitiveTuple[]>;
  palette: Palette;
};

// Finds the archetype a node's provenance resolves to within a role-keyed
// binding: first by `declared_for` (exact semantic_type), then by `role_axis`,
// then by the archetype name itself. No match ⇒ the node stays unbound.
function archetypeFor(
  mapping: RoleKeyedMapping,
  provenance: { semanticType?: string; roleAxis?: string } | undefined
): RoleKeyedArchetype | undefined {
  if (!provenance) return undefined;
  const { semanticType, roleAxis } = provenance;
  const entries = Object.entries(mapping.archetypes);
  if (semanticType) {
    const byType = entries.find(
      ([name, arch]) =>
        arch.declared_for?.includes(semanticType) || name === semanticType
    );
    if (byType) return byType[1];
  }
  if (roleAxis) {
    const byRole = entries.find(([, arch]) => arch.role_axis === roleAxis);
    if (byRole) return byRole[1];
  }
  return undefined;
}

function resolvedForm(
  entity: UniverseView["entities"] extends ReadonlyMap<string, infer E> ? E : never
): ResolvedForm {
  const embodiment = entity.embodiment;
  // No producing-toolkit binding inlined ⇒ the honest bare-presence fallback.
  if (!embodiment) return FALLBACK_PRESENCE;
  const mapping = embodiment.mapping as unknown as Partial<RoleKeyedMapping> & {
    forms?: Record<string, PrimitiveTuple[]>;
    lod_states?: Record<string, string>;
    fallback_form?: string;
    palette: Palette;
  };

  // Role-keyed binding (the underground toolkit and its kin): resolve the
  // archetype for the node's provenance, then its form for the residency, falling
  // back to the archetype's dormant form, then to the bare presence.
  if (mapping.archetypes) {
    const roleKeyed = mapping as unknown as RoleKeyedMapping;
    const archetype = archetypeFor(roleKeyed, entity.provenance);
    if (!archetype) return FALLBACK_PRESENCE;
    const requested = archetype.forms[embodiment.residency];
    const dormant =
      archetype.fallback_form && roleKeyed.dormant_form
        ? roleKeyed.dormant_form[archetype.fallback_form]
        : undefined;
    const form = requested ?? dormant;
    if (!form) return FALLBACK_PRESENCE;
    return { form, palette: roleKeyed.palette };
  }

  // LOD-keyed binding (the citizen-energy catalog) — the existing path, unchanged.
  const requested = mapping.lod_states?.[embodiment.residency];
  const name =
    requested && mapping.forms?.[requested] ? requested : mapping.fallback_form;
  const form = name ? mapping.forms?.[name] : undefined;
  if (!form) return FALLBACK_PRESENCE;
  return { form, palette: mapping.palette };
}

function paletteColor(palette: Palette, bucket: string): string {
  if (bucket === "core") return palette.core;
  if (bucket === "shell") return palette.shell;
  // Both spellings are in the wild: the citizen catalog uses "particles"; the
  // role-keyed toolkit bindings and the fallback presence use "particle".
  if (bucket === "particles" || bucket === "particle") return palette.particle;
  return palette.core;
}

// Renders one embodied being: its primitives, back-to-front, modulated by the
// entity's material (opacity = presence, emissiveIntensity = glow/confidence) AND
// its per-node dynamics (`mod`): weight scales the whole body, the embedding
// orients it and jitters each primitive, and energy widens the emissive halo.
// At identity modulation every transform is a strict no-op, so an un-modulated
// being renders byte-for-byte as before.
function renderBeing(
  base: Vec3,
  form: readonly PrimitiveTuple[],
  palette: Palette,
  alpha: number,
  glow: number,
  cx: number,
  cy: number,
  mod: BeingModulation = IDENTITY_MODULATION
): string[] {
  const order = (kind: string) =>
    kind === "fresnel_shell" ? 0 : kind === "points" ? 1 : kind === "capsule" ? 2 : 3;
  // Keep each primitive's ORIGINAL index so its jitter is stable under the
  // depth-ordering sort — the variation is tied to the part, not its draw order.
  const indexed = form.map((primitive, index) => ({ primitive, index }));
  const sorted = indexed.sort((a, b) => order(a.primitive[0]) - order(b.primitive[0]));
  const haloBoost = Math.min(1.6, mod.emissiveMultiplier);
  const parts: string[] = [];

  for (const { primitive, index } of sorted) {
    const [kind, , bucket, offset, , scale, count] = primitive;
    const jitter = primitiveJitter(mod.embedding, index, mod.microvariation);
    // Offset: jitter, then body scale, then embedding orientation, around the node.
    const localOffset = rotateY(scaleVec(add(offset, jitter.offset), mod.scale), mod.yaw);
    const center = project(add(base, localOffset), cx, cy);
    // Geometry scale: per-primitive jitter × overall body scale.
    const effScale = scaleVec(mulVec(scale, jitter.scale), mod.scale);
    const color = paletteColor(palette, bucket);

    if (kind === "fresnel_shell") {
      const rx = f(Math.max(effScale[0], effScale[2]) * UNIT * 0.85);
      const ry = f(effScale[1] * UNIT * 0.85);
      parts.push(
        `<ellipse cx="${f(center.x)}" cy="${f(center.y)}" rx="${rx}" ry="${ry}" fill="${color}" fill-opacity="${f(0.16 * alpha)}"/>`
      );
    } else if (kind === "points") {
      const n = Math.min(count, 40);
      const spread = Math.max(effScale[0], effScale[1]) * UNIT * 0.9;
      for (let i = 0; i < n; i += 1) {
        const r = spread * Math.sqrt((i + 0.5) / n);
        const a = i * 2.399963229; // golden angle — deterministic scatter
        const px = center.x + r * Math.cos(a);
        const py = center.y + r * Math.sin(a) * 0.7;
        parts.push(
          `<circle cx="${f(px)}" cy="${f(py)}" r="1.1" fill="${color}" fill-opacity="${f(0.5 * alpha)}"/>`
        );
      }
    } else if (kind === "capsule") {
      const half = (effScale[1] * UNIT) / 2;
      const wpx = f(Math.max(effScale[0], effScale[2]) * UNIT);
      parts.push(
        `<line x1="${f(center.x)}" y1="${f(center.y - half)}" x2="${f(center.x)}" y2="${f(center.y + half)}" stroke="${color}" stroke-opacity="${f(0.7 * alpha)}" stroke-width="${wpx}" stroke-linecap="round"/>`
      );
    } else if (kind === "box") {
      // a machined/masonry block — a rect, hard edges (no rounding)
      const w = effScale[0] * UNIT;
      const h = effScale[1] * UNIT;
      parts.push(
        `<rect x="${f(center.x - w / 2)}" y="${f(center.y - h / 2)}" width="${f(w)}" height="${f(h)}" fill="${color}" fill-opacity="${f(alpha)}"/>`
      );
    } else if (kind === "cylinder" || kind === "tube") {
      // a shaft / conduit — a rounded-end rect; a tube is slender
      const w = (kind === "tube" ? 0.36 : 1) * effScale[0] * UNIT;
      const h = effScale[1] * UNIT;
      parts.push(
        `<rect x="${f(center.x - w / 2)}" y="${f(center.y - h / 2)}" width="${f(w)}" height="${f(h)}" rx="${f(w / 2)}" fill="${color}" fill-opacity="${f(alpha)}"/>`
      );
    } else if (kind === "cone") {
      // a horn / funnel / gain — an upward triangle
      const w = effScale[0] * UNIT;
      const h = effScale[1] * UNIT;
      parts.push(
        `<polygon points="${f(center.x)},${f(center.y - h / 2)} ${f(center.x - w / 2)},${f(center.y + h / 2)} ${f(center.x + w / 2)},${f(center.y + h / 2)}" fill="${color}" fill-opacity="${f(alpha)}"/>`
      );
    } else if (kind === "torus") {
      // a port / rim / ring — a stroked circle, hollow centre
      const r = f(((effScale[0] + effScale[2]) / 2) * UNIT * 0.6);
      const sw = f(Math.max(1, effScale[1] * UNIT * 0.3));
      parts.push(
        `<circle cx="${f(center.x)}" cy="${f(center.y)}" r="${r}" fill="none" stroke="${color}" stroke-opacity="${f(alpha)}" stroke-width="${sw}"/>`
      );
    } else if (kind === "plane") {
      // a membrane / surface / card — a thin flat rect
      const w = effScale[0] * UNIT;
      const h = Math.max(1, effScale[1] * UNIT * 0.25);
      parts.push(
        `<rect x="${f(center.x - w / 2)}" y="${f(center.y - h / 2)}" width="${f(w)}" height="${f(h)}" fill="${color}" fill-opacity="${f(0.8 * alpha)}"/>`
      );
    } else {
      // icosphere / sphere — the energy core, with a soft emissive halo. Energy
      // both brightens (glow) and widens (haloBoost) the halo, so a high-energy
      // node reads hotter without ever lighting an un-measured one (glow gated).
      const radius = ((effScale[0] + effScale[2]) / 2) * UNIT * 0.62;
      if (glow > 0) {
        parts.push(
          `<circle cx="${f(center.x)}" cy="${f(center.y)}" r="${f(radius * 1.9 * haloBoost)}" fill="${palette.emissive}" fill-opacity="${f(0.22 * glow * alpha)}"/>`
        );
      }
      parts.push(
        `<circle cx="${f(center.x)}" cy="${f(center.y)}" r="${f(radius)}" fill="${color}" fill-opacity="${f(alpha)}"/>`
      );
    }
  }
  return parts;
}

export function renderSceneSvg(view: UniverseView, options: SceneOptions = {}): string {
  const width = options.width ?? 480;
  const height = options.height ?? 340;
  const cx = width / 2;
  const cy = height / 2 + 30;

  const entities = [...view.entities.values()].sort((a, b) => (a.id < b.id ? -1 : 1));
  const relations = [...view.relations.values()].sort((a, b) => (a.id < b.id ? -1 : 1));
  const center = new Map(entities.map((entity) => [entity.id, entity.position]));

  // Relations first (behind the beings), as gentle bonds. Two graph-derived
  // channels modulate the arc (ALIGN §2), each strictly opt-in so a wire that
  // carries neither draws the exact same neutral bond as before:
  //   • polarity sign → colour: excitation (+) vs inhibition (−);
  //   • hierarchy → slope: a hierarchical bond leans its crown toward the whole
  //     and arcs higher, so "part → whole" reads without flattening.
  const EXCITATION = "#46e0d0";
  const INHIBITION = "#d8607a";
  const bonds: string[] = [];
  for (const relation of relations) {
    const source = center.get(relation.source);
    const target = center.get(relation.target);
    if (!source || !target) continue;
    const a = project(source, cx, cy);
    const b = project(target, cx, cy);
    let crownX = (a.x + b.x) / 2;
    let crownY = (a.y + b.y) / 2 - 26;
    const { hierarchy, polarity } = relation.visual;
    if (hierarchy !== undefined && hierarchy !== 0) {
      crownX -= hierarchy * (b.x - a.x) * 0.28;
      crownY -= Math.abs(hierarchy) * 10;
    }
    let stroke = relation.visual.material.color;
    if (polarity) {
      const mean = (polarity[0] + polarity[1]) / 2;
      if (mean > 0) stroke = EXCITATION;
      else if (mean < 0) stroke = INHIBITION;
    }
    bonds.push(
      `<path d="M ${f(a.x)} ${f(a.y)} Q ${f(crownX)} ${f(crownY)} ${f(b.x)} ${f(b.y)}" fill="none" stroke="${stroke}" stroke-opacity="${f(0.5 * relation.visual.material.opacity)}" stroke-width="1.5"/>`
    );
  }

  // Beings, depth-sorted far → near.
  const drawn = entities
    .map((entity) => ({ entity, depth: project(entity.position, cx, cy).depth }))
    .sort((a, b) => a.depth - b.depth);

  const beings: string[] = [];
  for (const { entity } of drawn) {
    const material = entity.visual.material;
    const alpha = material.opacity;
    const baseGlow = Math.max(0, Math.min(1, material.emissiveIntensity / 2.4));
    // Per-node dynamics from the graph-declared envelope (mapping.dynamics) and
    // the node's live signals. A node whose confidence is 0 (base glow 0) cannot
    // be brightened by energy — the ALIGN §3 honesty gate, enforced here.
    const dyn = deriveEntityDynamics(
      entity.embodiment?.mapping.dynamics as EmbodimentDynamics | undefined,
      {
        energy: entity.dynamics?.energy,
        weight: entity.dynamics?.weight,
        embedding: entity.dynamics?.embedding,
        confident: baseGlow > 0
      }
    );
    const glow = Math.max(0, Math.min(1, baseGlow * dyn.emissiveMultiplier));
    const resolved = resolvedForm(entity);
    const p = project(entity.position, cx, cy);
    if (options.lantern && glow <= 0) {
      // Fog-shrouded: an un-measured node the Lantern cannot resolve. It is misted,
      // not hidden and not faked — the honest "here is something we don't know".
      beings.push(
        `<circle cx="${f(p.x)}" cy="${f(p.y)}" r="30" fill="#47566d" fill-opacity="0.14"/>`,
        `<circle cx="${f(p.x)}" cy="${f(p.y)}" r="18" fill="#3a4658" fill-opacity="0.20"/>`,
        `<circle cx="${f(p.x)}" cy="${f(p.y)}" r="4.5" fill="#6b7688" fill-opacity="0.30" stroke="#8a97a8" stroke-opacity="0.35" stroke-width="0.6" stroke-dasharray="2 2"/>`,
        `<text x="${f(p.x)}" y="${f(p.y + 3)}" text-anchor="middle" font-size="8" fill="#9aa7b8" fill-opacity="0.7">?</text>`
      );
      continue;
    }
    // Every node resolves to a form: its toolkit binding's archetype, or — when
    // unbound — the bare-presence fallback point. There is no un-formed branch and
    // no universal default; an unbound node is drawn honestly as a bare presence.
    beings.push(
      ...renderBeing(entity.position, resolved.form, resolved.palette, alpha, glow, cx, cy, {
        scale: dyn.scale,
        yaw: dyn.yaw,
        emissiveMultiplier: dyn.emissiveMultiplier,
        microvariation: dyn.microvariation,
        embedding: entity.dynamics?.embedding
      })
    );
  }

  const health = options.health ?? (view.synchronized ? "synchronized" : "unsynchronized");
  const mode = options.lantern ? " · lantern: revealing epistemic status" : "";
  const badge = `<text x="12" y="24" font-family="monospace" font-size="12" fill="#8a97a8">health: ${health} · entities: ${entities.length} · relations: ${relations.length}${mode}</text>`;

  // In lantern mode a warm lamp pool sits at the observer's foot: what falls in
  // its light and is measured reads clear; the un-measured stays in the Fog.
  const lamp = options.lantern
    ? `<circle cx="${f(cx)}" cy="${f(cy + 70)}" r="230" fill="url(#lamp)"/>`
    : "";

  return [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}">`,
    `<defs><radialGradient id="space" cx="50%" cy="42%" r="70%"><stop offset="0%" stop-color="#141b26"/><stop offset="100%" stop-color="#080a0e"/></radialGradient>`,
    `<radialGradient id="lamp" cx="50%" cy="50%" r="50%"><stop offset="0%" stop-color="#ffe4a8" stop-opacity="0.10"/><stop offset="100%" stop-color="#ffe4a8" stop-opacity="0"/></radialGradient></defs>`,
    `<rect width="${width}" height="${height}" fill="url(#space)"/>`,
    lamp,
    ...bonds,
    ...beings,
    badge,
    `</svg>`
  ].join("\n");
}
