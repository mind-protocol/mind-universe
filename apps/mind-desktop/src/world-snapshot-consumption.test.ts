import { describe, expect, it } from "vitest";
import worldSnapshotFrames from "../../../fixtures/desktop-world-snapshot/visual/world-snapshot-frames.json";
import { universeEventFromServerFrame } from "./protocol-adapter";
import { applyUniverseEvent, emptyUniverseView } from "./universe-state";

// End-to-end proof that the renderer can display the bounded situation ACTUALLY
// held in a real Universe store: the frames fixture is projected by the Rust bin
// `desktop_world_snapshot` from `artifacts/assets/visual-mapping-*/store` (the
// materialized visual-mapping authority), then folded here exactly as the live
// wire path would fold it. If the projection or the adapter drift, this fails.
describe("real-store world snapshot consumption", () => {
  it("folds every projected frame into the universe view", () => {
    let view = emptyUniverseView();
    let parsed = 0;
    for (const frame of worldSnapshotFrames) {
      const event = universeEventFromServerFrame(frame);
      expect(event).not.toBeNull();
      parsed += 1;
      view = applyUniverseEvent(view, event!);
    }

    // Every frame parsed and applied; the stream never desynchronised.
    expect(parsed).toBe(worldSnapshotFrames.length);
    expect(view.synchronized).toBe(true);
    expect(view.revision).toBe(0);

    // The five real entities and five relations from the store are present.
    expect(view.entities.size).toBe(5);
    expect(view.relations.size).toBe(5);

    // The materialized visual-mapping catalog Atom (0x7011) is one of them, and
    // is projected with an honest "unknown" visual (no invented meaning).
    const catalog = view.entities.get(
      "00000000000000000000000000007011"
    );
    expect(catalog).toBeDefined();
    expect(catalog!.visual.primitive).toBe("unknown");
    expect(catalog!.visual.motion).toBe("still");
  });

  it("rejects a frame whose sequence breaks continuity", () => {
    // A gap must desynchronise the view rather than silently accept the frame.
    const snapshot = universeEventFromServerFrame(worldSnapshotFrames[0]);
    const secondEntity = universeEventFromServerFrame(worldSnapshotFrames[2]);
    expect(snapshot).not.toBeNull();
    expect(secondEntity).not.toBeNull();
    let view = applyUniverseEvent(emptyUniverseView(), snapshot!);
    // Skip sequence — apply frame 2 (sequence 3) right after the snapshot (1).
    view = applyUniverseEvent(view, secondEntity!);
    expect(view.synchronized).toBe(false);
  });
});
