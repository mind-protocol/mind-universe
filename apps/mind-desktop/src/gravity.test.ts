import { describe, expect, it } from "vitest";
import { settleOnGround, type GravityItem } from "./gravity";

// A flat ground at a chosen height, to isolate stacking from terrain relief.
const flat = (h: number) => () => h;

// A roughly-cubic node: footprint and half-height equal (the common case).
const cube = (
  id: string,
  x: number,
  z: number,
  half: number,
  seedY: number
): GravityItem => ({ id, x, z, footprint: half, halfHeight: half, seedY });

describe("settleOnGround (touch the ground, except when stacked)", () => {
  it("rests every non-overlapping node's base on the ground", () => {
    const items = [cube("a", 0, 0, 0.5, 4), cube("b", 10, 0, 0.5, 1)];
    const settled = settleOnGround(items, flat(-6));
    for (const id of ["a", "b"]) {
      const s = settled.get(id)!;
      expect(s.stacked).toBe(false);
      expect(s.y - 0.5).toBeCloseTo(-6, 10); // base = y - halfHeight on the ground
    }
  });

  it("follows terrain relief so the base meets the land beneath it", () => {
    const ground = (x: number, _z: number) => x; // slope: height == x
    const settled = settleOnGround([cube("a", 3, 0, 0.5, 9)], ground);
    expect(settled.get("a")!.y - 0.5).toBeCloseTo(3, 10); // base at ground(3)=3
  });

  it("stacks a node onto one whose footprint it overlaps", () => {
    const items = [cube("lower", 0, 0, 0.5, 0), cube("upper", 0, 0, 0.5, 5)];
    const settled = settleOnGround(items, flat(0));
    const lower = settled.get("lower")!;
    const upper = settled.get("upper")!;
    expect(lower.stacked).toBe(false);
    expect(lower.y).toBeCloseTo(0.5, 10);
    expect(upper.stacked).toBe(true);
    expect(upper.support).toBeCloseTo(lower.top, 10);
    expect(upper.y).toBeCloseTo(1.5, 10);
  });

  it("does not stack nodes whose footprints do not overlap", () => {
    const items = [cube("a", 0, 0, 0.5, 0), cube("b", 1.01, 0, 0.5, 9)]; // gap > 1.0
    const settled = settleOnGround(items, flat(0));
    expect(settled.get("b")!.stacked).toBe(false);
  });

  // The reason footprint and halfHeight are separate: a wide, thin plateau (a
  // `space` dais) must carry what sits on it at its SURFACE — not lifted by its
  // large footprint, which is exactly the single-radius bug this fixes.
  it("a flat plateau carries its content at its surface, not a footprint above", () => {
    const items: GravityItem[] = [
      { id: "plateau", x: 0, z: 0, footprint: 5, halfHeight: 0.3, seedY: 0 },
      { id: "tree", x: 1, z: 0, footprint: 0.5, halfHeight: 2, seedY: 9 }
    ];
    const settled = settleOnGround(items, flat(0));
    const plateau = settled.get("plateau")!;
    const tree = settled.get("tree")!;
    expect(plateau.y).toBeCloseTo(0.3, 10); // base on ground, thin dais
    expect(plateau.top).toBeCloseTo(0.6, 10); // surface just above the ground
    expect(tree.stacked).toBe(true);
    expect(tree.support).toBeCloseTo(0.6, 10); // rests on the surface, not at ~5
    expect(tree.y).toBeCloseTo(2.6, 10); // 0.6 (surface) + 2 (its own half-height)
  });

  it("builds a three-high tower when three share a footprint", () => {
    const items = [
      cube("z", 0, 0, 0.5, 2),
      cube("y", 0, 0, 0.5, 1),
      cube("x", 0, 0, 0.5, 0)
    ];
    const settled = settleOnGround(items, flat(0));
    expect(settled.get("x")!.y).toBeCloseTo(0.5, 10);
    expect(settled.get("y")!.y).toBeCloseTo(1.5, 10);
    expect(settled.get("z")!.y).toBeCloseTo(2.5, 10);
  });

  it("is deterministic and order-independent of input array order", () => {
    const a = cube("a", 0, 0, 0.5, 3);
    const b = cube("b", 0, 0, 0.5, 1);
    const one = settleOnGround([a, b], flat(0));
    const two = settleOnGround([b, a], flat(0));
    expect(one.get("a")).toEqual(two.get("a"));
    expect(one.get("b")).toEqual(two.get("b"));
  });
});
