import { useEffect, useRef } from "react";
import type { MaterializedEntity } from "./contracts";
import { reconcileAudioLoops } from "./audio-loops";

// The renderer's audio layer. It renders nothing; it keeps one looping
// HTMLAudioElement alive per audio-bearing entity and reconciles that set every
// time the view (or the mute flag) changes, using the pure controller in
// `audio-loops.ts`. All policy — which entity sounds, from which source — comes
// from the graph-projected `entity.audio` facet; this component only obeys it.
//
// Autoplay policy: play() may reject until the user has interacted with the
// window. We swallow that rejection so the first gesture (which App also uses to
// unlock audio) lets the loops start on the next reconciliation.
export function AudioLoops({
  entities,
  muted
}: {
  readonly entities: ReadonlyMap<string, MaterializedEntity>;
  readonly muted: boolean;
}) {
  const elements = useRef<Map<string, HTMLAudioElement>>(new Map());

  useEffect(() => {
    if (typeof Audio === "undefined") return;
    const live = elements.current;
    const active = new Map<string, string>();
    for (const [id, element] of live) active.set(id, element.src);

    const { start, stop } = reconcileAudioLoops(active, entities.values(), muted);

    for (const id of stop) {
      const element = live.get(id);
      if (element) {
        element.pause();
        element.src = "";
      }
      live.delete(id);
    }
    for (const command of start) {
      const existing = live.get(command.id);
      if (existing) {
        existing.pause();
        existing.src = "";
      }
      const element = new Audio(command.src);
      element.loop = command.loop;
      element.volume = command.gain;
      live.set(command.id, element);
      void element.play().catch(() => {
        /* Autoplay blocked until a user gesture; retried on next reconcile. */
      });
    }
  }, [entities, muted]);

  // Tear every loop down when the layer unmounts.
  useEffect(() => {
    const live = elements.current;
    return () => {
      for (const element of live.values()) {
        element.pause();
        element.src = "";
      }
      live.clear();
    };
  }, []);

  return null;
}
