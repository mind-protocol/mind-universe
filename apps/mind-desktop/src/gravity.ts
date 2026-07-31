// Gravity / sedimentation for the ville-jardin skin.
//
// Design decision (user directive): "les nodes doivent toucher le sol (sauf si
// empilées)" — every node's base rests on the ground, EXCEPT when it is stacked,
// and stacking is *pure gravity* (chosen over hierarchy-as-stacking): a node only
// leaves the ground when its horizontal footprint overlaps a node already resting
// beneath it, in which case it settles on that node's upper surface.
//
// The kernel layout (`universe_assets::layout`) stays authoritative for the
// horizontal placement (x, z) and only seeds the drop ORDER here; the ground the
// nodes actually touch — the terrain relief — lives only in the renderer, so the
// vertical settle is a renderer-geometry projection (like `Foundation` already
// samples `terrainHeight`), not an invented position. Pure and deterministic: no
// RNG, no clock, ordered traversal — same input always yields the same heights.

export interface GravityItem {
  readonly id: string;
  readonly x: number;
  readonly z: number;
  /**
   * Horizontal footprint half-extent — decides whether two nodes overlap and so
   * whether one can rest on the other. A `space` renders as a wide, flat dais, so
   * its footprint is large even though it is thin.
   */
  readonly footprint: number;
  /**
   * Vertical half-height — so the base (`y - halfHeight`) sits exactly on its
   * support. Kept SEPARATE from `footprint`: a flat plateau (small halfHeight,
   * large footprint) carries its contents at its surface, not a footprint above.
   */
  readonly halfHeight: number;
  /**
   * Pre-gravity height. Used ONLY to order the sediment (a node that started
   * higher settles later, so it can come to rest on one that started lower).
   */
  readonly seedY: number;
}

export interface Settled {
  /** Centre height so the node's base (`y - radius`) touches its support. */
  readonly y: number;
  /** Upper surface (`y + radius`) — what a node stacking on top rests on. */
  readonly top: number;
  /** The surface this node rests on: the ground, or a supporting node's top. */
  readonly support: number;
  /** True when it came to rest on another node rather than the ground. */
  readonly stacked: boolean;
}

/**
 * Settle every item so its base touches the ground `groundAt(x, z)`, unless its
 * footprint overlaps a node already placed lower, in which case it stacks on that
 * node's top. Deterministic sedimentation.
 */
export function settleOnGround(
  items: readonly GravityItem[],
  groundAt: (x: number, z: number) => number
): Map<string, Settled> {
  // Deterministic drop order: lowest seed first (so nodes settle bottom-up and a
  // higher one can land on a lower one), ties broken by id — no RNG, no clock.
  const order = [...items].sort(
    (a, b) => a.seedY - b.seedY || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
  );

  const placed: Array<GravityItem & { readonly top: number }> = [];
  const result = new Map<string, Settled>();

  for (const item of order) {
    // Start on the ground; raise onto the highest overlapping node beneath.
    let support = groundAt(item.x, item.z);
    let stacked = false;
    for (const other of placed) {
      const dx = item.x - other.x;
      const dz = item.z - other.z;
      const reach = item.footprint + other.footprint;
      // Footprints overlap horizontally → `item` would land on `other`.
      if (dx * dx + dz * dz < reach * reach && other.top > support) {
        support = other.top;
        stacked = true;
      }
    }
    const y = support + item.halfHeight;
    const settled: Settled = { y, top: y + item.halfHeight, support, stacked };
    placed.push({ ...item, top: settled.top });
    result.set(item.id, settled);
  }

  return result;
}
