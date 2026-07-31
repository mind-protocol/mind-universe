import type {
  EmbodimentMotionProfile,
  MaterializedEntity,
  Vector3,
  VisualEmbodimentMapping
} from "./contracts";
import type { UniverseView } from "./universe-state";
import { NEUTRAL_DYNAMICS } from "./entity-dynamics";
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

export const AVATAR_ENTITY_ID = "fixture:actor:citizen-energy-avatar";

const avatar: MaterializedEntity = {
  id: AVATAR_ENTITY_ID,
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
  dynamics: NEUTRAL_DYNAMICS,
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
  control: { kind: "observer" },
  available_actions: []
});

/** The bound avatar Actor, positioned at `at` — the entity the piloting loop moves. */
export const avatarEntityAt = (at: Vector3): MaterializedEntity => ({
  ...avatar,
  position: at
});

/**
 * Composes the controllable avatar into an existing (non-avatar) universe so the
 * piloting loop can move it inside a populated scene — e.g. the default city
 * projection. The base view is not mutated; the avatar is added as one more
 * entity at `at`, leaving relations, transfers, and control untouched.
 */
export const withAvatar = (base: UniverseView, at: Vector3): UniverseView => {
  const entities = new Map(base.entities);
  entities.set(AVATAR_ENTITY_ID, avatarEntityAt(at));
  return { ...base, entities };
};
