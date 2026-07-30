import type { EntityId, MaterializedEntity } from "./contracts";

// Pure, DOM-free loop reconciliation. Given the entities currently in the view
// and the set of loops presently sounding, it decides which loops to START and
// which to STOP so that exactly the audio-bearing entities are looping.
//
// The rule the feature promises: a `thing` whose graph content declared an audio
// pointer loops while it is present in the local situation, and stops the moment
// it is released, removed, muted, or its source changes. No timers, no hidden
// policy — the desired set is a deterministic function of (entities, muted).

export interface AudioLoopCommand {
  readonly id: EntityId;
  readonly src: string;
  readonly loop: boolean;
  readonly gain: number;
}

export interface AudioReconciliation {
  // Loops to (re)start now. A source change for an already-active id appears in
  // both `stop` and `start` so the caller tears down the old element first.
  readonly start: readonly AudioLoopCommand[];
  // Ids whose loop must stop now (released, muted, audio removed, or src changed).
  readonly stop: readonly EntityId[];
  // The active map the caller should hold after applying this reconciliation:
  // id -> the source now sounding for it.
  readonly active: ReadonlyMap<EntityId, string>;
}

// The loops that SHOULD be sounding for a view. Empty when muted — muting is an
// honest silence, not a paused-but-tracked state. Keyed by entity id; the value
// is the full command so the caller has src/loop/gain without a second lookup.
export function desiredAudioLoops(
  entities: Iterable<MaterializedEntity>,
  muted: boolean
): Map<EntityId, AudioLoopCommand> {
  const desired = new Map<EntityId, AudioLoopCommand>();
  if (muted) return desired;
  for (const entity of entities) {
    const audio = entity.audio;
    if (!audio || audio.src.length === 0) continue;
    desired.set(entity.id, {
      id: entity.id,
      src: audio.src,
      loop: audio.loop,
      gain: Math.min(1, Math.max(0, audio.gain))
    });
  }
  return desired;
}

// Diff the presently-sounding loops against what the view now wants.
export function reconcileAudioLoops(
  active: ReadonlyMap<EntityId, string>,
  entities: Iterable<MaterializedEntity>,
  muted: boolean
): AudioReconciliation {
  const desired = desiredAudioLoops(entities, muted);
  const start: AudioLoopCommand[] = [];
  const stop: EntityId[] = [];

  // Stop anything active that is no longer desired, or whose source changed.
  for (const [id, src] of active) {
    const wanted = desired.get(id);
    if (!wanted || wanted.src !== src) stop.push(id);
  }
  // Start anything desired that is not already sounding the same source.
  for (const command of desired.values()) {
    if (active.get(command.id) !== command.src) start.push(command);
  }

  const nextActive = new Map<EntityId, string>();
  for (const command of desired.values()) nextActive.set(command.id, command.src);

  return { start, stop, active: nextActive };
}
