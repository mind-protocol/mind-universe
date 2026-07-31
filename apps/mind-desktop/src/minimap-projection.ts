// Pure top-down projection for the minimap: world (x, z) -> panel pixels.
//
// The minimap is a bird's-eye 2D overview drawn in plain SVG (no WebGL), so it
// renders even when the 3D render loop is suspended. This module is the geometry
// only — deterministic and unit-testable, kept out of the React component.

export interface MinimapView {
  /** Drawable width of the map body, in pixels. */
  readonly width: number;
  /** Drawable height of the map body, in pixels. */
  readonly height: number;
  /** World radius the map must fit (the farthest node from the plaza centre). */
  readonly radius: number;
  /** User zoom factor (1 = whole city fits with padding). */
  readonly zoom: number;
  /** Inner padding in pixels so edge nodes are not clipped at zoom 1. */
  readonly pad: number;
}

/** Pixels per world unit for the given view (accounts for zoom and padding). */
export function minimapScale(view: MinimapView): number {
  const half = Math.min(view.width, view.height) / 2;
  const radius = Math.max(1e-6, view.radius);
  return ((half - view.pad) / radius) * view.zoom;
}

/**
 * Projects a world (x, z) to panel pixel coordinates. The plaza centre (0, 0)
 * maps to the panel centre; +x goes right, +z goes down (screen-natural top-down).
 */
export function projectToMinimap(
  x: number,
  z: number,
  view: MinimapView
): readonly [number, number] {
  const scale = minimapScale(view);
  return [view.width / 2 + x * scale, view.height / 2 + z * scale];
}

/** Clamps a zoom factor to a sane range so the map never inverts or vanishes. */
export function clampZoom(zoom: number, min = 0.25, max = 8): number {
  if (!Number.isFinite(zoom)) return 1;
  return Math.min(max, Math.max(min, zoom));
}

/** Clamps a panel dimension so it can never collapse below a usable size. */
export function clampSize(value: number, min = 120, max = 640): number {
  return Math.min(max, Math.max(min, value));
}
