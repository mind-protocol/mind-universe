import type {
  EmbodimentPrimitiveTuple,
  MaterializedEntity,
  Vector3,
  VisualEmbodimentMapping
} from "./contracts";
import type { UniverseView } from "./universe-state";
import { NEUTRAL_DYNAMICS } from "./entity-dynamics";
import { avatarMotionFixture } from "./avatar-fixture";
import appearanceToolkit from "../../../fixtures/ontology/appearance-toolkit-v0.json";

// The "fix": adapt an `affordance-materialization/1` binding into the
// `visual-embodiment/1` mapping the desktop renderer already speaks. A construct's
// form is the UNION of its affordances' materializations — each materialization's
// form_primitive_tuples are concatenated into one renderable form. The desktop then
// renders the SAME output the Appearance toolkit's `wield_appearance` bin proved
// (proven in the store; here made VISIBLE), using the extended primitive palette
// (cylinder body, torus rim, plane liquid, fresnel aura, spark points).

type MaterializationTuple = [
  string,
  string,
  string,
  number[],
  number[],
  number[],
  number,
  number
];

interface AffordanceMaterialization {
  readonly affordance: { readonly subtype: string; readonly kernel_kind: string };
  readonly form: MaterializationTuple[];
  readonly inverse_mapping: string;
}

const codeMember = (
  appearanceToolkit as unknown as {
    members: {
      id: string;
      content?: {
        magic_cup_binding?: {
          palette: Record<string, string>;
          affordance_materializations: AffordanceMaterialization[];
        };
      };
    }[];
  }
).members.find((member) => member.id.startsWith("code:"));

const cup = codeMember?.content?.magic_cup_binding;
if (!cup) {
  throw new Error("appearance toolkit fixture has no code.magic_cup_binding");
}

// The renderer's material buckets are core | shell | particles; the toolkit binding
// uses core | shell | emissive | particle. Bridge the singular/plural + `emissive`
// names so each part colours through the same palette the renderer reads.
const rendererBucket = (role: string): string =>
  role === "particle" ? "particles" : role === "emissive" ? "shell" : role;

const toEmbodimentTuple = (tuple: MaterializationTuple): EmbodimentPrimitiveTuple =>
  [
    tuple[0],
    tuple[1],
    rendererBucket(tuple[2]),
    tuple[3],
    tuple[4],
    tuple[5],
    tuple[6],
    tuple[7]
  ] as unknown as EmbodimentPrimitiveTuple;

// UNION of every affordance's materialization = the whole magic cup.
const unionForm: EmbodimentPrimitiveTuple[] = cup.affordance_materializations.flatMap(
  (materialization) => materialization.form.map(toEmbodimentTuple)
);

// A reduced LOD: the structural affordances only (contain + sip = body + rim).
const structuralForm: EmbodimentPrimitiveTuple[] = cup.affordance_materializations
  .filter(
    (materialization) =>
      materialization.affordance.subtype === "contain" ||
      materialization.affordance.subtype === "sip"
  )
  .flatMap((materialization) => materialization.form.map(toEmbodimentTuple));

export const MAGIC_CUP_MAPPING_ID = "appearance:magic-cup-v0";

// Built by the adapter above, then cast to the renderer contract (same pattern as
// avatar-fixture bridging the visual-embodiment catalog JSON).
const MAGIC_CUP_MAPPING = {
  mapping_id: MAGIC_CUP_MAPPING_ID,
  schema_version: "visual-embodiment/1",
  form_family: "affordance_materialization",
  fallback_form: "magic_cup",
  primitive_budget: 12,
  particle_budget: 96,
  palette: {
    core: "#f4e3b0",
    emissive: "#8fd6ff",
    shell: "#c9a86a",
    particle: "#8fd6ff"
  },
  material: {
    core_opacity: 0.9,
    shell_opacity: 0.3,
    core_emissive_intensity: 1.6,
    shell_emissive_intensity: 1.2,
    fresnel_power: 2.2
  },
  forms: {
    magic_cup: unionForm,
    magic_cup_structural: structuralForm,
    magic_cup_dormant: [
      ["points", "presence", "particles", [0, 0, 0], [0, 0, 0], [1, 1, 1], 1, 0.4]
    ]
  },
  lod_states: {
    dormant: "magic_cup_dormant",
    aggregated: "magic_cup_structural",
    sleeping: "magic_cup_structural",
    hot: "magic_cup"
  }
} as unknown as VisualEmbodimentMapping;

export const MAGIC_CUP_ENTITY_ID = "fixture:construct:magic-cup";

const magicCup: MaterializedEntity = {
  id: MAGIC_CUP_ENTITY_ID,
  generation: 0,
  position: [0, 0, 0],
  visual: {
    primitive: "unknown",
    motion: "still",
    material: {
      color: "#c9a86a",
      emissive: "#8fd6ff",
      emissiveIntensity: 1.6,
      opacity: 0.9,
      scale: 1
    }
  },
  dynamics: NEUTRAL_DYNAMICS,
  embodiment: {
    source_mapping_id: MAGIC_CUP_MAPPING_ID,
    mapping: MAGIC_CUP_MAPPING,
    motion_profile: avatarMotionFixture,
    residency: "hot",
    previous_position: [0, 0, 0],
    previous_sampled_at_ms: 500,
    sampled_at_ms: 1000
  }
};

/**
 * A one-construct universe: the magic cup, its form the UNION of its affordances'
 * materializations, rendered by the desktop's existing embodiment pipeline over the
 * extended primitive palette. This is the visible counterpart of `wield_appearance`.
 */
export const magicCupFixtureUniverse = (): UniverseView => ({
  revision: 1,
  sequence: 0,
  synchronized: true,
  entities: new Map([[magicCup.id, magicCup]]),
  relations: new Map(),
  transfers: new Map(),
  control: { kind: "observer" },
  available_actions: []
});

/** The magic cup positioned at `at`. */
export const magicCupEntityAt = (at: Vector3): MaterializedEntity => ({
  ...magicCup,
  position: at
});

/**
 * Composes the magic cup into an existing (populated) universe so it stands IN a
 * scene rather than floating alone — e.g. on the plaza of the pilot city. The base
 * view is not mutated; the cup is added as one more entity at `at`.
 */
export const withMagicCup = (base: UniverseView, at: Vector3): UniverseView => {
  const entities = new Map(base.entities);
  entities.set(MAGIC_CUP_ENTITY_ID, magicCupEntityAt(at));
  return { ...base, entities };
};
