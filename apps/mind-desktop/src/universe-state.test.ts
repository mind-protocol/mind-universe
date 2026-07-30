import { describe, expect, it } from "vitest";
import type { EnergyTransfer, UniverseEvent } from "./contracts";
import { applyUniverseEvent, emptyUniverseView } from "./universe-state";

const transfer = (
  transferId: string,
  relationId = "relation:1"
): EnergyTransfer => ({
  transferId,
  executionId: "execution:1",
  intentionId: "intention:1",
  revision: 7,
  tick: 12,
  relationId,
  source: "entity:1",
  target: "entity:2",
  direction: "source_to_target",
  polarity: "support",
  energy: 4,
  gate: 1,
  outcome: "measured",
  epistemic: "measured",
  visual: {
    primitive: "energy_packet",
    color: "#fff3c4",
    emissive: "#ffd76a",
    emissiveIntensity: 4,
    radius: 0.09,
    opacity: 1,
    durationMs: 240
  }
});

describe("Universe projection", () => {
  it("refuses to present a stream with a sequence gap as synchronized", () => {
    const snapshot: UniverseEvent = {
      version: 0,
      sequence: 0,
      kind: "snapshot_started",
      revision: 7
    };
    const gap: UniverseEvent = {
      version: 0,
      sequence: 2,
      kind: "control_changed",
      control: { kind: "observer" }
    };

    const coherent = applyUniverseEvent(emptyUniverseView(), snapshot);
    const stale = applyUniverseEvent(coherent, gap);

    expect(coherent.synchronized).toBe(true);
    expect(stale.synchronized).toBe(false);
    expect(stale.sequence).toBe(0);
  });

  it("requires a capability receipt before actor control is granted", () => {
    const base = applyUniverseEvent(emptyUniverseView(), {
      version: 0,
      sequence: 0,
      kind: "snapshot_started",
      revision: 1
    });
    const requested = applyUniverseEvent(base, {
      version: 0,
      sequence: 1,
      kind: "control_changed",
      control: { kind: "requested", actor: "actor:1", requestId: "request:1" }
    });
    const granted = applyUniverseEvent(requested, {
      version: 0,
      sequence: 2,
      kind: "control_changed",
      control: {
        kind: "granted",
        actor: "actor:1",
        capabilityReceipt: "receipt:1"
      }
    });

    expect(requested.control.kind).toBe("requested");
    expect(granted.control).toEqual({
      kind: "granted",
      actor: "actor:1",
      capabilityReceipt: "receipt:1"
    });
  });

  it("tracks concurrent measured transfers by receipt-backed transfer id", () => {
    const base = applyUniverseEvent(emptyUniverseView(), {
      version: 0,
      sequence: 0,
      kind: "snapshot_started",
      revision: 7
    });
    const first = applyUniverseEvent(base, {
      version: 0,
      sequence: 1,
      kind: "energy_transferred",
      transfer: transfer("transfer:1")
    });
    const second = applyUniverseEvent(first, {
      version: 0,
      sequence: 2,
      kind: "energy_transferred",
      transfer: transfer("transfer:2")
    });

    expect(second.transfers.size).toBe(2);
    expect(second.transfers.get("transfer:1")?.relationId).toBe("relation:1");
  });

  it("releases one completed transfer without removing another on the same bond", () => {
    const base = applyUniverseEvent(emptyUniverseView(), {
      version: 0,
      sequence: 0,
      kind: "snapshot_started",
      revision: 7
    });
    const first = applyUniverseEvent(base, {
      version: 0,
      sequence: 1,
      kind: "energy_transferred",
      transfer: transfer("transfer:1")
    });
    const second = applyUniverseEvent(first, {
      version: 0,
      sequence: 2,
      kind: "energy_transferred",
      transfer: transfer("transfer:2")
    });
    const released = applyUniverseEvent(second, {
      version: 0,
      sequence: 3,
      kind: "energy_transfer_released",
      transferId: "transfer:1"
    });

    expect(released.transfers.has("transfer:1")).toBe(false);
    expect(released.transfers.has("transfer:2")).toBe(true);
  });

  it("fails closed when a transfer is not measured or lacks provenance", () => {
    const base = applyUniverseEvent(emptyUniverseView(), {
      version: 0,
      sequence: 0,
      kind: "snapshot_started",
      revision: 7
    });
    const unmeasured = applyUniverseEvent(base, {
      version: 0,
      sequence: 1,
      kind: "energy_transferred",
      transfer: {
        ...transfer("transfer:unknown"),
        executionId: "",
        epistemic: "not_measured"
      }
    });

    expect(unmeasured.synchronized).toBe(false);
    expect(unmeasured.transfers.size).toBe(0);
  });
});
