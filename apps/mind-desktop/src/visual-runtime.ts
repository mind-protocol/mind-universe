import type {
  EnergyTransferPrimitive,
  EnergyTransferVisualDescriptor,
  Vector3
} from "./contracts";

export interface TransferVisualFrame {
  readonly position: Vector3;
  readonly progress: number;
  readonly scale: number;
  readonly opacity: number;
  readonly visible: boolean;
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

function interpolate(source: Vector3, target: Vector3, progress: number): Vector3 {
  return [
    source[0] + (target[0] - source[0]) * progress,
    source[1] + (target[1] - source[1]) * progress,
    source[2] + (target[2] - source[2]) * progress
  ];
}

function primitiveScale(
  primitive: EnergyTransferPrimitive,
  radius: number,
  progress: number
): number {
  switch (primitive) {
    case "energy_packet":
      return radius * (0.88 + Math.sin(progress * Math.PI * 6) * 0.12);
    case "inhibitory_wave":
      return radius * (0.65 + progress * 1.35);
    case "rupture":
      return radius * (0.5 + Math.sin(progress * Math.PI) * 1.5);
  }
}

export function sampleTransferVisual(
  descriptor: EnergyTransferVisualDescriptor,
  source: Vector3,
  target: Vector3,
  elapsedMs: number
): TransferVisualFrame {
  const progress = clamp01(elapsedMs / descriptor.durationMs);
  const visible =
    Number.isFinite(elapsedMs) &&
    descriptor.durationMs > 0 &&
    elapsedMs >= 0 &&
    elapsedMs <= descriptor.durationMs;
  const fade = descriptor.primitive === "energy_packet" ? 1 : 1 - progress * 0.35;

  return {
    position: interpolate(source, target, progress),
    progress,
    scale: primitiveScale(descriptor.primitive, descriptor.radius, progress),
    opacity: descriptor.opacity * fade,
    visible
  };
}
