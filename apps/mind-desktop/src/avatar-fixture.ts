import type {
  EmbodimentMotionProfile,
  MaterializedEntity,
  VisualEmbodimentMapping
} from "./contracts";
import type { UniverseView } from "./universe-state";

export const AVATAR_MAPPING_AUTHORITY =
  "visual-mapping:l2:mind-desktop:citizen-energy-semi-humanoid-v1";

export const avatarMappingFixture: VisualEmbodimentMapping = {
  mapping_id: "citizen-energy-semi-humanoid-v1",
  schema_version: "visual-embodiment/1",
  form_family: "energy_semi_humanoid",
  fallback_form: "energy_orb",
  primitive_budget: 10,
  particle_budget: 96,
  palette: {
    core: "#77d9ff",
    emissive: "#1b9fff",
    shell: "#a5eaff",
    particle: "#d8f7ff"
  },
  material: {
    core_opacity: 0.82,
    shell_opacity: 0.28,
    core_emissive_intensity: 2.4,
    shell_emissive_intensity: 1.7,
    fresnel_power: 2.2
  },
  forms: {
    energy_orb: [
      [
        "icosphere",
        "core",
        "core",
        [0, 0, 0],
        [0, 0, 0],
        [0.56, 0.7, 0.56],
        0,
        0
      ],
      [
        "fresnel_shell",
        "aura",
        "shell",
        [0, 0, 0],
        [0, 0, 0],
        [0.72, 0.86, 0.72],
        0,
        0
      ],
      [
        "points",
        "internal_energy",
        "particles",
        [0, 0, 0],
        [0, 0, 0],
        [1, 1, 1],
        64,
        0.62
      ]
    ],
    semi_humanoid: [
      [
        "sphere",
        "head",
        "core",
        [0, 1.22, 0],
        [0, 0, 0],
        [0.36, 0.4, 0.36],
        0,
        0
      ],
      [
        "capsule",
        "torso",
        "core",
        [0, 0.43, 0],
        [0, 0, 0],
        [0.58, 0.7, 0.34],
        0,
        0
      ],
      [
        "capsule",
        "left_arm",
        "shell",
        [-0.63, 0.42, 0],
        [0, 0, -0.16],
        [0.16, 0.62, 0.16],
        0,
        0
      ],
      [
        "capsule",
        "right_arm",
        "shell",
        [0.63, 0.42, 0],
        [0, 0, 0.16],
        [0.16, 0.62, 0.16],
        0,
        0
      ],
      [
        "capsule",
        "left_leg",
        "shell",
        [-0.26, -0.68, 0],
        [0, 0, 0],
        [0.2, 0.72, 0.2],
        0,
        0
      ],
      [
        "capsule",
        "right_leg",
        "shell",
        [0.26, -0.68, 0],
        [0, 0, 0],
        [0.2, 0.72, 0.2],
        0,
        0
      ],
      [
        "fresnel_shell",
        "aura",
        "shell",
        [0, 0.28, 0],
        [0, 0, 0],
        [0.92, 1.65, 0.72],
        0,
        0
      ],
      [
        "points",
        "internal_energy",
        "particles",
        [0, 0.25, 0],
        [0, 0, 0],
        [1, 1.45, 0.8],
        96,
        0.78
      ]
    ]
  },
  lod_states: {
    dormant: "energy_orb",
    aggregated: "energy_orb",
    sleeping: "energy_orb",
    hot: "semi_humanoid"
  },
  reduced_motion: {
    trail: false,
    noise: false,
    retain_state_readability: true
  }
};

export const avatarMotionFixture: EmbodimentMotionProfile = {
  profile_id: "fluid-energy-locomotion-v0",
  interpolation: {
    model: "critically_damped_spring",
    settle_seconds: 0.22,
    max_visual_lag_seconds: 0.35,
    correction_threshold: 2.5,
    correction_mode: "visible_reformation"
  },
  bindings: {
    speed_to_stride_hz: [0, 4, 0, 7],
    speed_to_stretch: [0, 4, 1, 1.28],
    speed_to_trail_opacity: [0, 3, 0, 0.62],
    idle_breath: { amplitude: 0.035, frequency_hz: 0.24 },
    turn_to_bank: { max_degrees: 12 }
  },
  trail: {
    max_samples: 24,
    sample_interval_seconds: 0.04,
    lifetime_seconds: 0.55
  }
};

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
