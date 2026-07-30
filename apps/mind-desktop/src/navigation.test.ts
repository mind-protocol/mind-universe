import { describe, expect, it } from "vitest";
import type { MaterializedEntity } from "./contracts";
import { applyNav, initialNav, pruneNav } from "./navigation";
import { emptyUniverseView } from "./universe-state";

const A = "entity:a";
const B = "entity:b";

describe("world-native navigation", () => {
  it("focuses and clears focus", () => {
    let nav = applyNav(initialNav(), { kind: "focus", entity: A });
    expect(nav.focus).toBe(A);
    nav = applyNav(nav, { kind: "clear_focus" });
    expect(nav.focus).toBeNull();
  });

  it("replaces selection by default and extends it additively", () => {
    let nav = applyNav(initialNav(), { kind: "select", entity: A });
    expect([...nav.selection]).toEqual([A]);
    nav = applyNav(nav, { kind: "select", entity: B, additive: true });
    expect(nav.selection.has(A)).toBe(true);
    expect(nav.selection.has(B)).toBe(true);
    nav = applyNav(nav, { kind: "select", entity: A }); // non-additive resets
    expect([...nav.selection]).toEqual([A]);
  });

  it("expands and releases, and toggles trails", () => {
    let nav = applyNav(initialNav(), { kind: "expand", entity: A });
    expect(nav.expanded.has(A)).toBe(true);
    nav = applyNav(nav, { kind: "release", entity: A });
    expect(nav.expanded.has(A)).toBe(false);
    nav = applyNav(nav, { kind: "toggle_trails" });
    expect(nav.trailsVisible).toBe(true);
  });

  it("records actor/observer control intent", () => {
    let nav = applyNav(initialNav(), { kind: "request_control", actor: A });
    expect(nav.mode).toBe("actor");
    expect(nav.actor).toBe(A);
    nav = applyNav(nav, { kind: "release_control" });
    expect(nav.mode).toBe("observer");
    expect(nav.actor).toBeNull();
  });

  it("prunes interaction state for entities no longer in the view", () => {
    let nav = initialNav();
    nav = applyNav(nav, { kind: "focus", entity: A });
    nav = applyNav(nav, { kind: "select", entity: A, additive: true });
    nav = applyNav(nav, { kind: "select", entity: B, additive: true });
    nav = applyNav(nav, { kind: "request_control", actor: B });

    // A view that only contains A — B has been released/removed.
    const view = {
      ...emptyUniverseView(),
      entities: new Map<string, MaterializedEntity>([[A, {} as MaterializedEntity]])
    };
    const pruned = pruneNav(nav, view);
    expect(pruned.focus).toBe(A);
    expect(pruned.selection.has(A)).toBe(true);
    expect(pruned.selection.has(B)).toBe(false);
    // Controlling a vanished actor falls back to observer.
    expect(pruned.mode).toBe("observer");
    expect(pruned.actor).toBeNull();
  });
});
