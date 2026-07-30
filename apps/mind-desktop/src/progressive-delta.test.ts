import { describe, expect, it } from "vitest";
import citizenFrames from "../../../fixtures/desktop-world-snapshot/citizen/world-snapshot-frames.json";
import deltaFrames from "../../../fixtures/desktop-world-snapshot/citizen/delta-batch2-frames.json";
import { universeEventFromServerFrame } from "./protocol-adapter";
import { applyUniverseEvent, emptyUniverseView, type UniverseView } from "./universe-state";

// Progressive visibility (G3 item 4): a new authoritative batch is streamed as a
// BOUNDED delta — only the batch's own write-set — and folds onto the existing
// view without re-sending the whole Universe. Here batch 2 adds one actor (Vega).
const VEGA = "0000000000000000000000000000b010";
const VEGA_OBSERVES_LEDGER = "0000000000000000000000000000b210";

function fold(view: UniverseView, frames: unknown[]): UniverseView {
  for (const frame of frames) {
    const event = universeEventFromServerFrame(frame);
    expect(event).not.toBeNull();
    view = applyUniverseEvent(view, event!);
  }
  return view;
}

describe("progressive batch delta", () => {
  it("streams only the batch write-set, not the whole Universe", () => {
    // The delta carries exactly the two batch-2 frames (one entity, one relation).
    expect(deltaFrames.length).toBe(2);
  });

  it("folds the delta onto the existing view to reveal the new entity", () => {
    // Start from the world after batch 1 (3 entities, 1 relation).
    const afterBatch1 = fold(emptyUniverseView(), citizenFrames);
    expect(afterBatch1.entities.size).toBe(3);
    expect(afterBatch1.relations.size).toBe(1);

    // Apply the batch-2 delta; sequence continues (6, 7) so it stays in sync.
    const afterBatch2 = fold(afterBatch1, deltaFrames);
    expect(afterBatch2.synchronized).toBe(true);
    expect(afterBatch2.entities.size).toBe(4);
    expect(afterBatch2.relations.size).toBe(2);

    // The newly committed actor is now visible, with its authority-resolved form.
    const vega = afterBatch2.entities.get(VEGA);
    expect(vega).toBeDefined();
    expect(vega!.embodiment?.mapping.mapping_id).toBe(
      "citizen-energy-semi-humanoid-v1"
    );
    expect(afterBatch2.relations.get(VEGA_OBSERVES_LEDGER)).toBeDefined();

    // The batch-1 entities were never re-sent, yet remain present.
    expect(afterBatch2.entities.has("0000000000000000000000000000b001")).toBe(true);
  });

  it("desynchronises if the delta is applied without the prior view", () => {
    // A delta whose sequence (6) does not continue an empty view (-1) must not
    // be silently accepted.
    const view = fold(emptyUniverseView(), deltaFrames);
    expect(view.synchronized).toBe(false);
  });
});
