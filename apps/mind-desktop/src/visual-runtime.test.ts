import { describe, expect, it } from "vitest";
import type { EnergyTransferVisualDescriptor } from "./contracts";
import { sampleTransferVisual } from "./visual-runtime";

const descriptor = (
  primitive: EnergyTransferVisualDescriptor["primitive"]
): EnergyTransferVisualDescriptor => ({
  primitive,
  color: "#ffffff",
  emissive: "#ffffff",
  emissiveIntensity: 4,
  radius: 0.1,
  opacity: 1,
  durationMs: 200
});

describe("graph-resolved transfer visual sampling", () => {
  it("interpolates a measured packet between attested endpoints", () => {
    const frame = sampleTransferVisual(
      descriptor("energy_packet"),
      [0, 0, 0],
      [4, 2, -2],
      100
    );

    expect(frame.position).toEqual([2, 1, -1]);
    expect(frame.progress).toBe(0.5);
    expect(frame.visible).toBe(true);
  });

  it("lets the supplied primitive descriptor control motion shape", () => {
    const packet = sampleTransferVisual(
      descriptor("energy_packet"),
      [0, 0, 0],
      [1, 0, 0],
      100
    );
    const wave = sampleTransferVisual(
      descriptor("inhibitory_wave"),
      [0, 0, 0],
      [1, 0, 0],
      100
    );

    expect(wave.scale).toBeGreaterThan(packet.scale);
  });

  it("hides an expired effect instead of treating relation presence as activity", () => {
    const frame = sampleTransferVisual(
      descriptor("energy_packet"),
      [0, 0, 0],
      [1, 0, 0],
      201
    );

    expect(frame.visible).toBe(false);
  });
});
