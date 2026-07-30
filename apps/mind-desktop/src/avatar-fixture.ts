import type {
  EmbodimentMotionProfile,
  MaterializedEntity,
  VisualEmbodimentMapping
} from "./contracts";
import type { UniverseView } from "./universe-state";
import visualCatalog from "../../../fixtures/assets/visual-embodiment-catalog.json";

// Single source of truth: the graph-materialized visual embodiment authority
// (crates/universe-assets/src/visual.rs + fixtures/assets/visual-embodiment-catalog.json).
// The renderer now CONSUMES this authority instead of hard-coding the mapping —
// closing the graph-first drift where the app owned the visual mapping. The JSON
// is validated at runtime by validateEmbodimentMapping(); the `as unknown` casts
// only bridge the widened JSON type to the readonly contract tuples.
export const AVATAR_MAPPING_AUTHORITY = visualCatalog.authority_id;

export const avatarMappingFixture =
  visualCatalog.mapping as unknown as VisualEmbodimentMapping;

export const avatarMotionFixture =
  visualCatalog.motion_profile as unknown as EmbodimentMotionProfile;

const avatar: MaterializedEntity = {
  id: "fixture:actor:citizen-energy-avatar",
  generation: 0,
  position: [0, 0, 0],
  visual: {
    primitive: "unknown",
    motion: "still",
    material: {
      color: "#77d9ff",
      emissive: "#1b9fff",
      emissiveIntensity: 2.4,
      opacity: 0.82,
      scale: 1
    }
  },
  embodiment: {
    source_mapping_id: AVATAR_MAPPING_AUTHORITY,
    mapping: avatarMappingFixture,
    motion_profile: avatarMotionFixture,
    residency: "hot",
    previous_position: [0, 0, 1.1],
    previous_sampled_at_ms: 500,
    sampled_at_ms: 1000
  }
};

export const avatarFixtureUniverse = (): UniverseView => ({
  revision: 1,
  sequence: 0,
  synchronized: true,
  entities: new Map([[avatar.id, avatar]]),
  relations: new Map(),
  transfers: new Map(),
  control: { kind: "observer" }
});
