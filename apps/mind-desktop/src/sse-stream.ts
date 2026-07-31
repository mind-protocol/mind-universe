// Browser SSE client for the dev-time live-store bridge (vite-plugin-universe-stream).
//
// It is the thin transport that feeds the tested stream reducer: each server-sent
// frame is JSON-parsed and folded through `ingestFrame`, and every resulting
// stream state (view + health + metrics) is reported to `onUpdate`, together with
// the presentation maps (label/detail/predicate) recovered for the hover tooltip.
// The frame shape and geometry reducer are identical to the native Tauri path —
// only the transport (EventSource over Vite's dev server) differs.
//
// Updates are coalesced with a short timer: a full store batch is ~1000 frames,
// and folding each into React state individually would thrash the scene. A timer
// (not requestAnimationFrame) is used deliberately — rAF is suspended while the
// document is hidden/not compositing, which would stall the stream entirely in a
// headless or backgrounded window.

import {
  ingestFrame,
  initialStream,
  onDisconnect,
  type StreamState
} from "./stream-client";
import {
  entityPresentationFromFrame,
  relationPresentationFromFrame
} from "./stream-presentation";
import type {
  EntityPresentation,
  RelationPresentation
} from "./postgres-pilot-fixture";

export interface LiveStore {
  readonly state: StreamState;
  readonly entityPresentation: ReadonlyMap<string, EntityPresentation>;
  readonly relationPresentation: ReadonlyMap<string, RelationPresentation>;
}

// Coalesce a burst of frames into one flush ~one frame later. setTimeout fires
// even when the page is hidden (unlike requestAnimationFrame), so the stream keeps
// syncing off-screen.
const COALESCE_MS = 16;

/**
 * Opens the SSE stream at `url` and folds incoming frames, reporting each new
 * LiveStore (view + health + presentation) to `onUpdate`, coalesced to one call
 * per animation frame. Returns a stop function. In an environment without
 * `EventSource` (e.g. a headless test), it is an inert no-op.
 */
export function startSseStream(
  url: string,
  onUpdate: (live: LiveStore) => void
): () => void {
  if (typeof EventSource === "undefined") return () => {};

  let state = initialStream();
  let entityPresentation = new Map<string, EntityPresentation>();
  let relationPresentation = new Map<string, RelationPresentation>();
  let closed = false;
  let scheduled = false;

  const flush = () => {
    scheduled = false;
    if (closed) return;
    onUpdate({ state, entityPresentation, relationPresentation });
  };
  const schedule = () => {
    if (scheduled || closed) return;
    scheduled = true;
    setTimeout(flush, COALESCE_MS);
  };

  const source = new EventSource(url);

  source.onmessage = (event) => {
    let frame: unknown;
    try {
      frame = JSON.parse(event.data);
    } catch {
      return; // a malformed frame is ignored, never applied
    }

    // A fresh snapshot resets the presentation maps to match the geometry reducer,
    // which discards its view on `snapshot_started` — the two stay in lockstep.
    const payload =
      frame && typeof frame === "object"
        ? (frame as { payload?: { message_type?: unknown } }).payload
        : undefined;
    if (payload?.message_type === "snapshot") {
      entityPresentation = new Map();
      relationPresentation = new Map();
    }
    const entity = entityPresentationFromFrame(frame);
    if (entity) entityPresentation.set(entity.id, entity.presentation);
    const relation = relationPresentationFromFrame(frame);
    if (relation) relationPresentation.set(relation.id, relation.presentation);

    state = ingestFrame(state, frame, performance.now());
    schedule();
  };

  source.onerror = () => {
    // The dev bridge dropped (server restart, prod build with no endpoint). Report
    // disconnected so the app can fall back to its baked fixture.
    state = onDisconnect(state);
    schedule();
  };

  return () => {
    closed = true;
    source.close();
  };
}
