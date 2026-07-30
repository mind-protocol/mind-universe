export const DESKTOP_PROTOCOL_VERSION = 0 as const;

export type EntityId = string;
export type RelationId = string;
export type Vector3 = readonly [number, number, number];
export type PhysicalResidency = "hot" | "sleeping" | "aggregated" | "dormant";

export type EpistemicState =
  | "observed"
  | "measured"
  | "known_absent"
  | "unknown"
  | "not_measured"
  | "measurement_failed";

export interface VisualMaterial {
  readonly color: string;
  readonly emissive: string;
  readonly emissiveIntensity: number;
  readonly opacity: number;
  readonly scale: number;
}

export type EmbodimentPrimitiveKind =
  | "icosphere"
  | "sphere"
  | "capsule"
  | "points"
  | "fresnel_shell";

export type EmbodimentMaterialKind = "core" | "shell" | "particles";

export type EmbodimentPrimitiveTuple = readonly [
  primitive: EmbodimentPrimitiveKind,
  role: string,
  material: EmbodimentMaterialKind,
  offset: Vector3,
  rotation: Vector3,
  scale: Vector3,
  count: number,
  radius: number
];

export interface VisualEmbodimentMapping {
  readonly mapping_id: string;
  readonly schema_version: "visual-embodiment/1";
  readonly form_family: string;
  readonly fallback_form: string;
  readonly primitive_budget: number;
  readonly particle_budget: number;
  readonly palette: {
    readonly core: string;
    readonly emissive: string;
    readonly shell: string;
    readonly particle: string;
  };
  readonly material: {
    readonly core_opacity: number;
    readonly shell_opacity: number;
    readonly core_emissive_intensity: number;
    readonly shell_emissive_intensity: number;
    readonly fresnel_power: number;
  };
  readonly forms: Readonly<Record<string, readonly EmbodimentPrimitiveTuple[]>>;
  readonly lod_states: Readonly<Record<PhysicalResidency, string>>;
  readonly reduced_motion: {
    readonly trail: boolean;
    readonly noise: boolean;
    readonly retain_state_readability: boolean;
  };
}

export interface EmbodimentMotionProfile {
  readonly profile_id: string;
  readonly interpolation: {
    readonly model: "critically_damped_spring";
    readonly settle_seconds: number;
    readonly max_visual_lag_seconds: number;
    readonly correction_threshold: number;
    readonly correction_mode: "visible_reformation";
  };
  readonly bindings: {
    readonly speed_to_stride_hz: readonly [number, number, number, number];
    readonly speed_to_stretch: readonly [number, number, number, number];
    readonly speed_to_trail_opacity: readonly [number, number, number, number];
    readonly idle_breath: {
      readonly amplitude: number;
      readonly frequency_hz: number;
    };
    readonly turn_to_bank: {
      readonly max_degrees: number;
    };
  };
  readonly trail: {
    readonly max_samples: number;
    readonly sample_interval_seconds: number;
    readonly lifetime_seconds: number;
  };
}

export interface EntityEmbodiment {
  readonly source_mapping_id: string;
  readonly mapping: VisualEmbodimentMapping;
  readonly motion_profile: EmbodimentMotionProfile;
  readonly residency: PhysicalResidency;
  readonly sampled_at_ms: number;
  readonly previous_position?: Vector3;
  readonly previous_sampled_at_ms?: number;
  readonly reduced_motion?: boolean;
}

export type EntityVisualPrimitive =
  | "pulsing_core"
  | "open_polyhedral_attractor"
  | "oriented_ring"
  | "bounded_volume"
  | "faceted_router"
  | "slab"
  | "torus_knot"
  | "cylinder"
  | "tetrahedron"
  | "unknown";

export type EntityMotionPrimitive =
  | "outward_pulse"
  | "inward_orbit"
  | "through_flow"
  | "boundary_breath"
  | "port_transform"
  | "still";

export interface EntityVisualDescriptor {
  readonly primitive: EntityVisualPrimitive;
  readonly motion: EntityMotionPrimitive;
  readonly material: VisualMaterial;
}

export interface MaterializedEntity {
  readonly id: EntityId;
  readonly generation: number;
  readonly position: Vector3;
  readonly visual: EntityVisualDescriptor;
  readonly embodiment?: EntityEmbodiment;
}

export interface RelationVisualDescriptor {
  readonly primitive: "dual_lane_bond" | "luminous_chain" | "navigable_path" | "unknown";
  readonly material: VisualMaterial;
  readonly width: number;
  readonly laneSeparation: number;
}

export interface MaterializedRelation {
  readonly id: RelationId;
  readonly source: EntityId;
  readonly target: EntityId;
  readonly visual: RelationVisualDescriptor;
}

export type EnergyTransferPrimitive =
  | "energy_packet"
  | "inhibitory_wave"
  | "rupture";

export interface EnergyTransferVisualDescriptor {
  readonly primitive: EnergyTransferPrimitive;
  readonly color: string;
  readonly emissive: string;
  readonly emissiveIntensity: number;
  readonly radius: number;
  readonly opacity: number;
  readonly durationMs: number;
}

export interface EnergyTransfer {
  readonly transferId: string;
  readonly executionId: string;
  readonly intentionId: string;
  readonly revision: number;
  readonly tick: number;
  readonly relationId: RelationId;
  readonly source: EntityId;
  readonly target: EntityId;
  readonly direction: "source_to_target" | "target_to_source";
  readonly polarity: "support" | "inhibit" | "neutral";
  readonly energy: number;
  readonly gate: number;
  readonly outcome: "measured" | "rejected";
  readonly epistemic: EpistemicState;
  readonly visual: EnergyTransferVisualDescriptor;
}

export type ControlState =
  | { readonly kind: "observer" }
  | { readonly kind: "requested"; readonly actor: EntityId; readonly requestId: string }
  | {
      readonly kind: "granted";
      readonly actor: EntityId;
      readonly capabilityReceipt: string;
    }
  | {
      readonly kind: "refused";
      readonly actor: EntityId;
      readonly reason: string;
    };

export type UniverseEvent =
  | {
      readonly version: typeof DESKTOP_PROTOCOL_VERSION;
      readonly sequence: number;
      readonly kind: "snapshot_started";
      readonly revision: number;
    }
  | {
      readonly version: typeof DESKTOP_PROTOCOL_VERSION;
      readonly sequence: number;
      readonly kind: "entity_materialized";
      readonly entity: MaterializedEntity;
    }
  | {
      readonly version: typeof DESKTOP_PROTOCOL_VERSION;
      readonly sequence: number;
      readonly kind: "entity_released";
      readonly entity: EntityId;
    }
  | {
      readonly version: typeof DESKTOP_PROTOCOL_VERSION;
      readonly sequence: number;
      readonly kind: "relation_materialized";
      readonly relation: MaterializedRelation;
    }
  | {
      readonly version: typeof DESKTOP_PROTOCOL_VERSION;
      readonly sequence: number;
      readonly kind: "energy_transferred";
      readonly transfer: EnergyTransfer;
    }
  | {
      readonly version: typeof DESKTOP_PROTOCOL_VERSION;
      readonly sequence: number;
      readonly kind: "energy_transfer_released";
      readonly transferId: string;
    }
  | {
      readonly version: typeof DESKTOP_PROTOCOL_VERSION;
      readonly sequence: number;
      readonly kind: "control_changed";
      readonly control: ControlState;
    };

export interface ControlRequest {
  readonly version: typeof DESKTOP_PROTOCOL_VERSION;
  readonly kind: "request_actor_control";
  readonly actor: EntityId;
  readonly requestId: string;
}

export function isRenderableEnergyTransfer(
  transfer: EnergyTransfer
): boolean {
  const descriptor = transfer.visual;
  return (
    transfer.transferId.length > 0 &&
    transfer.executionId.length > 0 &&
    transfer.intentionId.length > 0 &&
    transfer.relationId.length > 0 &&
    transfer.source.length > 0 &&
    transfer.target.length > 0 &&
    Number.isSafeInteger(transfer.revision) &&
    transfer.revision >= 0 &&
    Number.isSafeInteger(transfer.tick) &&
    transfer.tick >= 0 &&
    Number.isFinite(transfer.energy) &&
    transfer.energy >= 0 &&
    Number.isFinite(transfer.gate) &&
    transfer.gate >= 0 &&
    transfer.gate <= 1 &&
    transfer.epistemic === "measured" &&
    Number.isFinite(descriptor.emissiveIntensity) &&
    descriptor.emissiveIntensity >= 0 &&
    Number.isFinite(descriptor.radius) &&
    descriptor.radius > 0 &&
    Number.isFinite(descriptor.opacity) &&
    descriptor.opacity >= 0 &&
    descriptor.opacity <= 1 &&
    Number.isSafeInteger(descriptor.durationMs) &&
    descriptor.durationMs > 0
  );
}
