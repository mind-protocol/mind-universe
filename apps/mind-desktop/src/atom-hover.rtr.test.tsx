// @vitest-environment happy-dom
//
// Measures the hover runtime path the non-compositing preview cannot: a pointer
// crossing an atom emits its node id (and null on leave) through onHover, which is
// what drives World's tooltip. Driven deterministically by @react-three/test-renderer
// (manual frames, no RAF, no visible browser) — the same technique as the actor /
// observer control runtime tests.

import ReactThreeTestRenderer from "@react-three/test-renderer";
import { describe, expect, it } from "vitest";
import { GenericAtom } from "./World";
import type { MaterializedEntity } from "./contracts";
import { NEUTRAL_DYNAMICS } from "./entity-dynamics";

const atom: MaterializedEntity = {
  id: "00000000000000000000000000001000",
  generation: 0,
  position: [1, 0.5, 2],
  visual: {
    primitive: "cylinder",
    motion: "still",
    material: {
      color: "#9b8fb0",
      emissive: "#9b8fb0",
      emissiveIntensity: 0.35,
      opacity: 0.92,
      scale: 0.7
    }
  },
  dynamics: NEUTRAL_DYNAMICS
};

describe("atom hover (measured via test-renderer)", () => {
  it("emits the node id on pointer-over and null on pointer-out", async () => {
    const emitted: (string | null)[] = [];
    const renderer = await ReactThreeTestRenderer.create(
      <GenericAtom
        entity={atom}
        presentation={undefined}
        selected={false}
        onSelect={() => {}}
        onHover={(id) => emitted.push(id)}
      />
    );
    const mesh = renderer.scene.findByType("Mesh");

    await renderer.fireEvent(mesh, "onPointerOver");
    expect(emitted).toEqual([atom.id]);

    await renderer.fireEvent(mesh, "onPointerOut");
    expect(emitted).toEqual([atom.id, null]);

    await renderer.unmount();
  });
});
