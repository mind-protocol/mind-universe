// Pure first-person look math, split from the R3F ObserverControls component the
// same way observer-controls.ts splits the pure motion intent. FPV means the eye
// is fixed and the *head* turns (yaw about world-up, pitch about the local right
// axis) — as opposed to an orbit camera, where the eye swings around a distant
// target. Keeping this pure makes the head-turn behavior testable without a canvas.

export interface LookOrientation {
  readonly yaw: number;
  readonly pitch: number;
}

// Just shy of straight up/down, so the view never flips over the poles or gimbals
// into a roll — the defining constraint of a first-person head versus a free orbit.
export const PITCH_LIMIT = Math.PI / 2 - 0.02;

// Apply a mouse-drag delta (in pixels) to a look orientation. Yaw accumulates
// freely (you can spin all the way around); pitch is clamped to PITCH_LIMIT.
// Dragging right/down turns the head right/down — the natural FPV mapping.
export function applyLookDelta(
  current: LookOrientation,
  deltaX: number,
  deltaY: number,
  sensitivity: number,
  pitchLimit: number = PITCH_LIMIT
): LookOrientation {
  const yaw = current.yaw - deltaX * sensitivity;
  const pitch = Math.max(
    -pitchLimit,
    Math.min(pitchLimit, current.pitch - deltaY * sensitivity)
  );
  return { yaw, pitch };
}

// Derive the yaw/pitch that makes an eye at `eye` look toward `target`, using
// three's YXZ (yaw-then-pitch) convention so it round-trips through
// Euler(pitch, yaw, 0, "YXZ"). Used for the opening framing and the R reset, so
// the FPV camera starts already looking at the focus instead of down -Z.
export function orientationFromLookAt(
  eye: readonly [number, number, number],
  target: readonly [number, number, number]
): LookOrientation {
  const dx = target[0] - eye[0];
  const dy = target[1] - eye[1];
  const dz = target[2] - eye[2];
  const length = Math.hypot(dx, dy, dz);
  if (length < 1e-6) return { yaw: 0, pitch: 0 };
  const ny = dy / length;
  return {
    // forward = (-cos(pitch)·sin(yaw), sin(pitch), -cos(pitch)·cos(yaw))
    yaw: Math.atan2(-(dx / length), -(dz / length)),
    pitch: Math.asin(Math.max(-1, Math.min(1, ny)))
  };
}

// Ground-plane forward/right basis for a given yaw. FPV walking stays level: the
// walk direction ignores pitch entirely, so looking up can never lift the camera
// off the floor. This preserves the "observer can never fly" contract that the
// old orbit rig enforced by flattening forward.y to 0.
export function groundBasis(yaw: number): {
  readonly forward: readonly [number, number, number];
  readonly right: readonly [number, number, number];
} {
  return {
    forward: [-Math.sin(yaw), 0, -Math.cos(yaw)],
    right: [Math.cos(yaw), 0, -Math.sin(yaw)]
  };
}
