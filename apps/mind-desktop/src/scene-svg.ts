// Deterministic scene renderer for visual regression (G3 item 6). The live
// renderer is WebGL (React Three Fiber); GPU pixel/video diffing is a separate
// harness. This module renders the SAME UniverseView to a DETERMINISTIC SVG that
// actually resembles the 3D ontology: it projects positions with an oblique
// pseudo-3D camera (depth-sorted), and for each entity it draws the graph-
// resolved EMBODIMENT FORM — the energy core, the fresnel aura, the internal
// particles, and the semi-humanoid limbs — using the mapping's own palette, with
// epistemic modulation (a `measured` being glows; an `unknown` one is dim, no
// emission). No randomness: particle scatter is a golden-angle spiral by index.

import type { UniverseView } from "./universe-state";

export interface SceneOptions {
  readonly width?: number;
  readonly height?: number;
  /** Overrides the health label (e.g. "stale", "degraded"). */
  readonly health?: string;
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

interface Palette {
  readonly core: string;
  readonly emissive: string;
  readonly shell: string;
  readonly particle: string;
}

function resolvedForm(
  entity: UniverseView["entities"] extends ReadonlyMap<string, infer E> ? E : never
): { form: readonly PrimitiveTuple[]; palette: Palette } | null {
  const embodiment = entity.embodiment;
  if (!embodiment) return null;
  const mapping = embodiment.mapping as unknown as {
    forms: Record<string, PrimitiveTuple[]>;
    lod_states: Record<string, string>;
    fallback_form: string;
    palette: Palette;
  };
  const requested = mapping.lod_states?.[embodiment.residency];
  const name = requested && mapping.forms[requested] ? requested : mapping.fallback_form;
  const form = mapping.forms[name];
  if (!form) return null;
  return { form, palette: mapping.palette };
}

function paletteColor(palette: Palette, bucket: string): string {
  if (bucket === "core") return palette.core;
  if (bucket === "shell") return palette.shell;
  if (bucket === "particles") return palette.particle;
  return palette.core;
}

// Renders one embodied being: its primitives, back-to-front, modulated by the
// entity's material (opacity = presence, emissiveIntensity = glow/confidence).
function renderBeing(
  base: Vec3,
  form: readonly PrimitiveTuple[],
  palette: Palette,
  alpha: number,
  glow: number,
  cx: number,
  cy: number
): string[] {
  const order = (kind: string) =>
    kind === "fresnel_shell" ? 0 : kind === "points" ? 1 : kind === "capsule" ? 2 : 3;
  const sorted = [...form].sort((a, b) => order(a[0]) - order(b[0]));
  const parts: string[] = [];

  for (const [kind, , bucket, offset, , scale, count] of sorted) {
    const center = project(add(base, offset), cx, cy);
    const color = paletteColor(palette, bucket);

    if (kind === "fresnel_shell") {
      const rx = f(Math.max(scale[0], scale[2]) * UNIT * 0.85);
      const ry = f(scale[1] * UNIT * 0.85);
      parts.push(
        `<ellipse cx="${f(center.x)}" cy="${f(center.y)}" rx="${rx}" ry="${ry}" fill="${color}" fill-opacity="${f(0.16 * alpha)}"/>`
      );
    } else if (kind === "points") {
      const n = Math.min(count, 40);
      const spread = Math.max(scale[0], scale[1]) * UNIT * 0.9;
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
      const half = (scale[1] * UNIT) / 2;
      const wpx = f(Math.max(scale[0], scale[2]) * UNIT);
      parts.push(
        `<line x1="${f(center.x)}" y1="${f(center.y - half)}" x2="${f(center.x)}" y2="${f(center.y + half)}" stroke="${color}" stroke-opacity="${f(0.7 * alpha)}" stroke-width="${wpx}" stroke-linecap="round"/>`
      );
    } else {
      // icosphere / sphere — the energy core, with a soft emissive halo.
      const radius = ((scale[0] + scale[2]) / 2) * UNIT * 0.62;
      if (glow > 0) {
        parts.push(
          `<circle cx="${f(center.x)}" cy="${f(center.y)}" r="${f(radius * 1.9)}" fill="${palette.emissive}" fill-opacity="${f(0.22 * glow * alpha)}"/>`
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

  // Relations first (behind the beings), as gentle bonds.
  const bonds: string[] = [];
  for (const relation of relations) {
    const source = center.get(relation.source);
    const target = center.get(relation.target);
    if (!source || !target) continue;
    const a = project(source, cx, cy);
    const b = project(target, cx, cy);
    const midX = (a.x + b.x) / 2;
    const midY = (a.y + b.y) / 2 - 26;
    bonds.push(
      `<path d="M ${f(a.x)} ${f(a.y)} Q ${f(midX)} ${f(midY)} ${f(b.x)} ${f(b.y)}" fill="none" stroke="${relation.visual.material.color}" stroke-opacity="${f(0.5 * relation.visual.material.opacity)}" stroke-width="1.5"/>`
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
    const glow = Math.max(0, Math.min(1, material.emissiveIntensity / 2.4));
    const resolved = resolvedForm(entity);
    const p = project(entity.position, cx, cy);
    if (resolved) {
      beings.push(...renderBeing(entity.position, resolved.form, resolved.palette, alpha, glow, cx, cy));
    } else {
      // No graph-resolved form: an honest dim marker, not a fake body.
      if (glow > 0) {
        beings.push(
          `<circle cx="${f(p.x)}" cy="${f(p.y)}" r="7.6" fill="${material.emissive}" fill-opacity="${f(0.2 * glow * alpha)}"/>`
        );
      }
      beings.push(
        `<circle cx="${f(p.x)}" cy="${f(p.y)}" r="4" fill="${material.color}" fill-opacity="${f(alpha)}"/>`
      );
    }
  }

  const health = options.health ?? (view.synchronized ? "synchronized" : "unsynchronized");
  const badge = `<text x="12" y="24" font-family="monospace" font-size="12" fill="#8a97a8">health: ${health} · entities: ${entities.length} · relations: ${relations.length}</text>`;

  return [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}">`,
    `<defs><radialGradient id="space" cx="50%" cy="42%" r="70%"><stop offset="0%" stop-color="#141b26"/><stop offset="100%" stop-color="#080a0e"/></radialGradient></defs>`,
    `<rect width="${width}" height="${height}" fill="url(#space)"/>`,
    ...bonds,
    ...beings,
    badge,
    `</svg>`
  ].join("\n");
}
