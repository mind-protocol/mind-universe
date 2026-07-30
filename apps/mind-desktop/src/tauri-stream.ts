// Webview glue for the native transport bridge (G3 item 1). Subscribes to the
// `universe-frame` events the Tauri `start_universe_stream` command emits and
// folds each frame into the stream reducer. The frame-handling logic
// (`ingestFrame`) is unit-tested; this file is the thin Tauri wiring on top and
// only runs inside the desktop shell.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ingestFrame, initialStream, type StreamState } from "./stream-client";

export interface UniverseStreamHandle {
  readonly stop: UnlistenFn;
}

/**
 * Starts the native stream and folds incoming frames, reporting each new stream
 * state (view + health + metrics) to `onState`. The secret is passed straight to
 * the native side; it is never persisted here.
 */
export async function startUniverseStream(
  address: string,
  secret: string,
  onState: (state: StreamState) => void
): Promise<UniverseStreamHandle> {
  let state = initialStream();
  const unlisten = await listen("universe-frame", (event) => {
    state = ingestFrame(state, event.payload, performance.now());
    onState(state);
  });
  await invoke("start_universe_stream", { address, secret });
  return { stop: unlisten };
}
