import { useFrame, useThree } from "@react-three/fiber";
import { useCallback, useEffect, useRef } from "react";
import { Euler } from "three";
import { observerMotion } from "./observer-controls";
import {
  applyLookDelta,
  groundBasis,
  type LookOrientation,
  orientationFromLookAt
} from "./first-person-look";

const DEFAULT_CAMERA: readonly [number, number, number] = [0, 2, 9];
const DEFAULT_TARGET: readonly [number, number, number] = [0, 0, 0];
// Standing eye height (metres above the floor). A first-person observer is a
// person on the ground, not a drone — the eye rides the terrain at this offset.
const DEFAULT_EYE_HEIGHT = 1.8;
// Flat ground fallback when no terrain sampler is supplied (e.g. bare unit tests).
const FLAT_GROUND = () => 0;
// Radians per pixel of drag. Tuned for a natural head-turn: a full 180° sweep is
// roughly a two-thirds screen-width drag at 1080p.
const LOOK_SENSITIVITY = 0.0026;
// A press that moves less than this stays a click, so atoms remain selectable.
// Past it, the press becomes a look-drag and the trailing click is swallowed.
const DRAG_THRESHOLD = 4;

// A first-person camera: the eye stays put and the head turns. Drag the mouse to
// look (yaw/pitch about the eye, NOT an orbit around a distant target); ZQSD/WASD
// walk in the direction you face. Movement stays on the ground plane — looking up
// never lifts you off the floor — so the observer roams but can never fly. R
// returns to the opening framing. A plain click still selects atoms.
export function ObserverControls({
  movementEnabled = true,
  initialCamera,
  initialTarget,
  groundHeight = FLAT_GROUND,
  eyeHeight = DEFAULT_EYE_HEIGHT,
  onPose
}: {
  // When the avatar is being piloted, keyboard translation is handed to
  // ActorControls so ZQSD moves the body, not the camera. Mouse look and the R
  // reset stay available so the pilot can still frame the avatar.
  readonly movementEnabled?: boolean;
  // Optional opening framing. When a view supplies a default focus (e.g. the
  // latest active ActorSession), the eye starts here already looking at the
  // target, and R resets back to it instead of the world origin.
  readonly initialCamera?: readonly [number, number, number];
  readonly initialTarget?: readonly [number, number, number];
  // Terrain sampler + standing eye height: the eye is pinned to the floor beneath
  // it plus eyeHeight, so the observer stands on the ground and follows its relief
  // rather than floating at whatever y the opening framing happened to use.
  readonly groundHeight?: (x: number, z: number) => number;
  readonly eyeHeight?: number;
  // Reports the observer's top-down pose (ground x/z + yaw) whenever it changes, so
  // an overlay (the minimap) can track the viewpoint. Called from the frame loop;
  // consumers should absorb it into a ref, not React state, to avoid re-rendering
  // the scene each frame.
  readonly onPose?: (pose: { x: number; z: number; yaw: number }) => void;
} = {}) {
  const { camera, gl } = useThree();
  const pressed = useRef(new Set<string>());
  const look = useRef<LookOrientation>({ yaw: 0, pitch: 0 });
  const lastPose = useRef<{ x: number; z: number; yaw: number } | null>(null);
  // Whether the observer has taken the viewpoint into their own hands (walked or
  // turned). Until then the opening framing may still re-aim the eye — after it,
  // a changing framing only moves where R takes them back to. The world keeps
  // changing under a live stream; that must never teleport someone mid-step.
  const taken = useRef(false);

  const homeCamera = initialCamera ?? DEFAULT_CAMERA;
  const homeTarget = initialTarget ?? DEFAULT_TARGET;
  // The framing arrives as a fresh array on every parent render (the live city
  // re-renders whenever a frame lands, or the stream drops and reconnects). Key
  // the re-framing on its VALUES: a re-render that frames the same place is not
  // a framing change and must move nothing.
  const framingKey = `${homeCamera.join()}|${homeTarget.join()}`;
  const home = useRef({ camera: homeCamera, target: homeTarget });

  const applyOrientation = useCallback(() => {
    camera.quaternion.setFromEuler(
      new Euler(look.current.pitch, look.current.yaw, 0, "YXZ")
    );
  }, [camera]);

  // Standing eye position at (x, z): floor height there + eyeHeight.
  const eyeAt = useCallback(
    (x: number, z: number): [number, number, number] => [
      x,
      groundHeight(x, z) + eyeHeight,
      z
    ],
    [groundHeight, eyeHeight]
  );

  // Goes to the framing held in `home` — read through the ref so `reset` keeps a
  // stable identity and a re-render can never re-fire the effects below.
  const reset = useCallback(() => {
    // Ignore the opening y: stand on the floor at the framing's (x, z). The look
    // is derived from the true eye position so the pitch is honest.
    const { camera: from, target } = home.current;
    const eye = eyeAt(from[0], from[2]);
    camera.position.set(eye[0], eye[1], eye[2]);
    look.current = orientationFromLookAt(eye, target);
    applyOrientation();
  }, [camera, applyOrientation, eyeAt]);

  // Establish the opening head orientation, so the FPV eye starts looking at the
  // focus rather than blankly down -Z — and re-aim it if the framing genuinely
  // moves (e.g. the city arrives and widens) while the view is still the app's to
  // frame. Once the observer has walked or turned, a new framing only updates
  // where R goes back to.
  useEffect(() => {
    home.current = { camera: homeCamera, target: homeTarget };
    if (!taken.current) reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the framing
    // VALUES; the arrays themselves are fresh on every parent render.
  }, [framingKey, reset]);

  // Keyboard: R resets the framing; every other key feeds the movement intent.
  useEffect(() => {
    const clear = () => pressed.current.clear();
    const keyDown = (event: KeyboardEvent) => {
      if (event.code === "KeyR" && !event.repeat) {
        reset();
        return;
      }
      pressed.current.add(event.code);
    };
    const keyUp = (event: KeyboardEvent) => pressed.current.delete(event.code);

    window.addEventListener("keydown", keyDown);
    window.addEventListener("keyup", keyUp);
    window.addEventListener("blur", clear);
    return () => {
      window.removeEventListener("keydown", keyDown);
      window.removeEventListener("keyup", keyUp);
      window.removeEventListener("blur", clear);
    };
  }, [reset]);

  // Mouse-drag look. A press under DRAG_THRESHOLD stays a click (atom selection);
  // once it crosses the threshold we capture the pointer, turn the head, and
  // swallow the trailing click so a look-drag never selects or deselects.
  useEffect(() => {
    const el = gl.domElement;
    let dragging = false;
    let looked = false;
    let lastX = 0;
    let lastY = 0;
    let startX = 0;
    let startY = 0;

    const down = (event: PointerEvent) => {
      if (event.button !== 0) return;
      dragging = true;
      looked = false;
      startX = lastX = event.clientX;
      startY = lastY = event.clientY;
    };
    const move = (event: PointerEvent) => {
      if (!dragging) return;
      const dx = event.clientX - lastX;
      const dy = event.clientY - lastY;
      lastX = event.clientX;
      lastY = event.clientY;
      if (
        !looked &&
        Math.hypot(event.clientX - startX, event.clientY - startY) < DRAG_THRESHOLD
      ) {
        return;
      }
      if (!looked) {
        looked = true;
        taken.current = true; // the head is theirs now; stop re-framing it
        el.style.cursor = "grabbing";
        el.setPointerCapture?.(event.pointerId);
      }
      look.current = applyLookDelta(look.current, dx, dy, LOOK_SENSITIVITY);
      applyOrientation();
    };
    const up = () => {
      if (!dragging) return;
      dragging = false;
      el.style.cursor = "";
      if (looked) {
        // R3F derives both onClick and onPointerMissed from the DOM `click`
        // event; swallowing it in the capture phase stops a look-drag from
        // selecting whatever ends up under the cursor or deselecting on a miss.
        const swallow = (click: MouseEvent) => {
          click.stopPropagation();
          el.removeEventListener("click", swallow, true);
        };
        el.addEventListener("click", swallow, true);
      }
    };

    el.addEventListener("pointerdown", down);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      el.style.cursor = "";
      el.removeEventListener("pointerdown", down);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  }, [gl, applyOrientation]);

  useFrame((_, delta) => {
    // Report the pose first, every frame, independent of movement — a look-drag
    // changes yaw without moving, and the minimap needle must follow it. The diff
    // guard means a still, unturning observer emits nothing.
    if (onPose) {
      const x = camera.position.x;
      const z = camera.position.z;
      const yaw = look.current.yaw;
      const last = lastPose.current;
      if (
        !last ||
        Math.abs(last.x - x) > 0.03 ||
        Math.abs(last.z - z) > 0.03 ||
        Math.abs(last.yaw - yaw) > 0.005
      ) {
        lastPose.current = { x, z, yaw };
        onPose({ x, z, yaw });
      }
    }

    if (!movementEnabled) return;

    // Walk in the facing direction, on the ground plane only (pitch ignored, so
    // looking up can never lift the eye off the floor).
    const motion = observerMotion(pressed.current);
    if (motion.forward === 0 && motion.right === 0) return;
    taken.current = true; // they are walking; the eye is theirs to place

    const speed = 5 * motion.speedMultiplier * Math.min(delta, 0.05);
    const { forward, right } = groundBasis(look.current.yaw);
    camera.position.x +=
      (forward[0] * motion.forward + right[0] * motion.right) * speed;
    camera.position.z +=
      (forward[2] * motion.forward + right[2] * motion.right) * speed;
    // Ride the terrain: keep the eye a fixed height above the floor as it moves,
    // so walking over hills raises the view instead of clipping through ground.
    camera.position.y = groundHeight(camera.position.x, camera.position.z) + eyeHeight;
  });

  return null;
}
