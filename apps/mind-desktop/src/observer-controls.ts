export interface ObserverMotion {
  readonly forward: number;
  readonly right: number;
  readonly up: number;
  readonly speedMultiplier: number;
}

const hasAny = (keys: ReadonlySet<string>, candidates: readonly string[]) =>
  candidates.some((key) => keys.has(key));

export function observerMotion(keys: ReadonlySet<string>): ObserverMotion {
  let forward =
    Number(hasAny(keys, ["KeyW", "KeyZ", "ArrowUp"])) -
    Number(hasAny(keys, ["KeyS", "ArrowDown"]));
  let right =
    Number(hasAny(keys, ["KeyD", "ArrowRight"])) -
    Number(hasAny(keys, ["KeyA", "KeyQ", "ArrowLeft"]));
  let up =
    Number(hasAny(keys, ["KeyE", "Space", "PageUp"])) -
    Number(hasAny(keys, ["KeyC", "PageDown"]));

  const length = Math.hypot(forward, right, up);
  if (length > 1) {
    forward /= length;
    right /= length;
    up /= length;
  }

  return {
    forward,
    right,
    up,
    speedMultiplier: hasAny(keys, ["ShiftLeft", "ShiftRight"])
      ? 3
      : hasAny(keys, ["AltLeft", "AltRight"])
        ? 0.25
        : 1
  };
}
