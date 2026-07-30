import { describe, expect, it } from "vitest";
import { universeEventFromServerFrame } from "./protocol-adapter";
import { applyUniverseEvent, emptyUniverseView } from "./universe-state";

const snapshotFrame = {
  protocol_version: 0,
  stream_id: "mind-desktop-energy-e2e",
  sequence: 1,
  correlation: "execution:1",
  payload: {
    message_type: "snapshot",
    universe: "00000000000000000000000000000001",
    revision: 1,
    tick: 0,
    origin: "00000000000000000000000000003071",
    completion: "partial",
    entities: [],
    relations: []
  }
};

const energyFrame = {
  protocol_version: 0,
  stream_id: "mind-desktop-energy-e2e",
  sequence: 2,
  correlation: "execution:1",
  payload: {
    message_type: "energy_transfer",
    universe: "00000000000000000000000000000001",
    revision: 1,
    tick: 1,
    transfer_id: "transfer:1",
    execution_id: "execution:1",
    intention_id: "00000000000000000000000000003075",
    relation_id: "00000000000000000000000000003070",
    source: "00000000000000000000000000003071",
    target: "00000000000000000000000000003072",
    direction: "source_to_target",
    polarity: "support",
    energy: 100,
    gate_microunits: 1_000_000,
    outcome: "measured",
    epistemic: "measured",
    visual: {
      primitive: "energy_packet",
      color: "#fff3c4",
      emissive: "#ffd76a",
      emissive_intensity_microunits: 4_000_000,
      radius_microunits: 90_000,
      opacity_microunits: 1_000_000,
      duration_ms: 240
    }
  }
};

describe("production protocol adapter", () => {
  it("converts Rust microunits without selecting visual meaning", () => {
    const transfer = universeEventFromServerFrame(energyFrame);
    expect(transfer?.kind).toBe("energy_transferred");
    if (transfer?.kind !== "energy_transferred") return;
    expect(transfer.transfer.visual).toEqual({
      primitive: "energy_packet",
      color: "#fff3c4",
      emissive: "#ffd76a",
      emissiveIntensity: 4,
      radius: 0.09,
      opacity: 1,
      durationMs: 240
    });
  });

  it("accepts a resynchronization snapshot at its real server sequence", () => {
    const snapshot = universeEventFromServerFrame(snapshotFrame);
    const transfer = universeEventFromServerFrame(energyFrame);
    expect(snapshot).not.toBeNull();
    expect(transfer).not.toBeNull();

    const projected = applyUniverseEvent(
      applyUniverseEvent(emptyUniverseView(), snapshot!),
      transfer!
    );

    expect(projected.sequence).toBe(2);
    expect(projected.synchronized).toBe(true);
    expect(projected.transfers.has("transfer:1")).toBe(true);
  });

  it("lets the reducer reject an unmeasured frame", () => {
    const invalid = {
      ...energyFrame,
      payload: { ...energyFrame.payload, epistemic: "not_measured" }
    };
    const snapshot = universeEventFromServerFrame(snapshotFrame);
    const transfer = universeEventFromServerFrame(invalid);
    expect(snapshot).not.toBeNull();
    expect(transfer?.kind).toBe("energy_transferred");

    const projected = applyUniverseEvent(
      applyUniverseEvent(emptyUniverseView(), snapshot!),
      transfer!
    );
    expect(projected.synchronized).toBe(false);
    expect(projected.transfers.size).toBe(0);
  });
});
