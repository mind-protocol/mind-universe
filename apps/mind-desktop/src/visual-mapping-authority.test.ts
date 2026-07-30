import { describe, expect, it } from "vitest";
import {
  AVATAR_MAPPING_AUTHORITY,
  avatarMappingFixture,
  avatarMotionFixture
} from "./avatar-fixture";
import { validateEmbodimentMapping } from "./embodiment";

// Guards the graph-first boundary: the renderer must consume the
// graph-materialized visual mapping authority (fixtures/assets/), and that
// authority must satisfy the renderer's own contract. If the graph fixture ever
// drifts from what the renderer can render, this fails in CI.
describe("graph-materialized visual mapping authority", () => {
  it("is the citizen energy semi-humanoid authority", () => {
    expect(AVATAR_MAPPING_AUTHORITY).toBe(
      "visual-mapping:l2:mind-desktop:citizen-energy-semi-humanoid-v1"
    );
    expect(avatarMappingFixture.mapping_id).toBe("citizen-energy-semi-humanoid-v1");
    expect(avatarMappingFixture.schema_version).toBe("visual-embodiment/1");
  });

  it("validates under the renderer's own budgets and rules", () => {
    expect(validateEmbodimentMapping(avatarMappingFixture)).toBe(true);
  });

  it("carries the fluid energy locomotion motion profile", () => {
    expect(avatarMotionFixture.profile_id).toBe("fluid-energy-locomotion-v0");
    expect(avatarMotionFixture.interpolation.model).toBe("critically_damped_spring");
  });
});
