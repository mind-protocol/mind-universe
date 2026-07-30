import { useFrame, useThree } from "@react-three/fiber";
import { useEffect, useRef } from "react";
import { Vector3 } from "three";
import type { Vector3 as Vec3 } from "./contracts";
import { actorDisplacement, type MotionBasis, type MotionBounds } from "./actor-control";
import { observerMotion } from "./observer-controls";

const WORLD_UP = new Vector3(0, 1, 0);

/**
 * Piloting input for the bound Actor. Renderer-side sibling of ObserverControls:
 * it reads ZQSD, derives a camera-relative bounded displacement via the pure
 * actor-control core, and hands it up through onMove — never mutating the camera
 * or writing a position directly. It emits motion ONLY while `piloting` (the
 * gate is granted); otherwise it stays silent so the refusal the scene records
 * is honest.
 */
export function ActorControls({
  bounds,
  piloting,
  onMove
}: {
  readonly bounds: MotionBounds;
  readonly piloting: boolean;
  readonly onMove: (displacement: Vec3) => void;
}) {
  const pressed = useRef(new Set<string>());
  const { camera } = useThree();

  useEffect(() => {
    const clear = () => pressed.current.clear();
    const keyDown = (event: KeyboardEvent) => pressed.current.add(event.code);
    const keyUp = (event: KeyboardEvent) => pressed.current.delete(event.code);
    window.addEventListener("keydown", keyDown);
    window.addEventListener("keyup", keyUp);
    window.addEventListener("blur", clear);
    return () => {
      window.removeEventListener("keydown", keyDown);
      window.removeEventListener("keyup", keyUp);
      window.removeEventListener("blur", clear);
    };
  }, []);

  useFrame((_, delta) => {
    if (!piloting) return;
    const intent = observerMotion(pressed.current);
    if (intent.forward === 0 && intent.right === 0 && intent.up === 0) return;

    const forward = camera.getWorldDirection(new Vector3());
    forward.y = 0;
    if (forward.lengthSq() < 0.0001) forward.set(0, 0, -1);
    forward.normalize();
    const right = new Vector3().crossVectors(forward, WORLD_UP).normalize();
    const basis: MotionBasis = {
      forward: [forward.x, forward.y, forward.z],
      right: [right.x, right.y, right.z],
      up: [WORLD_UP.x, WORLD_UP.y, WORLD_UP.z]
    };

    const displacement = actorDisplacement(intent, delta, basis, bounds);
    if (displacement[0] === 0 && displacement[1] === 0 && displacement[2] === 0) return;
    onMove(displacement);
  });

  return null;
}
