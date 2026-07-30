// Shared ground surface for the ontology3d "ville-jardin" city skin.
//
// Per space:mind-universe:ontology3d:v1, Space is a territory the scene sits on,
// not a void. The floor is a stable land relief (a city rests on solid land) with
// gentle elevation — it "monte ou descendre" across the map but does not wobble in
// time. Both the ground mesh and every building foundation sample this one pure
// function, so a foundation always meets the terrain exactly beneath it and the
// layout is deterministic for a given revision/viewport (spatial-mapping invariant).

// Base height of the land. Set below the lowest fixture entity (world y ~ -3.84)
// so foundations always have positive length and buildings read as rising from land.
export const TERRAIN_BASE_Y = -6;

// Extent of the ground plane in world units (square, centered on origin).
export const TERRAIN_SIZE = 120;

// Segments per side of the ground mesh. Coarser than graph paper so cells read as
// districts/blocks rather than a dense wireframe.
export const TERRAIN_SEGMENTS = 44;

/**
 * World-space height (y) of the land at a given horizontal (x, z).
 *
 * Pure and time-invariant: same coordinates always yield the same elevation, so
 * the terrain is solid ground rather than an animated surface. The relief is a sum
 * of low-frequency waves, giving broad rolling hills and shallow valleys — the
 * "districts" of the garden-city.
 */
export function terrainHeight(x: number, z: number): number {
  const relief =
    Math.sin(x * 0.15) * 0.7 +
    Math.cos(z * 0.12) * 0.55 +
    Math.sin((x + z) * 0.07) * 0.4;
  return TERRAIN_BASE_Y + relief;
}
