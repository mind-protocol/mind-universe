import { OrbitControls } from "@react-three/drei";
import { useFrame, useThree } from "@react-three/fiber";
import {
  type ComponentRef,
  useCallback,
  useEffect,
  useRef
} from "react";
import { Vector3 } from "three";
import { observerMotion } from "./observer-controls";

const INITIAL_CAMERA = new Vector3(0, 2, 9);
const INITIAL_TARGET = new Vector3(0, 0, 0);
const WORLD_UP = new Vector3(0, 1, 0);

export function ObserverControls() {
  const controls = useRef<ComponentRef<typeof OrbitControls>>(null);
  const pressed = useRef(new Set<string>());
  const { camera, gl } = useThree();

  const reset = useCallback(() => {
    camera.position.copy(INITIAL_CAMERA);
    controls.current?.target.copy(INITIAL_TARGET);
    controls.current?.update();
  }, [camera]);

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

  useFrame((_, delta) => {
    const orbit = controls.current;
    if (!orbit) return;

    const motion = observerMotion(pressed.current);
    if (motion.forward === 0 && motion.right === 0 && motion.up === 0) return;

    const speed = 5 * motion.speedMultiplier * Math.min(delta, 0.05);
    const forward = camera.getWorldDirection(new Vector3());
    forward.y = 0;
    if (forward.lengthSq() < 0.0001) forward.set(0, 0, -1);
    forward.normalize();
    const right = new Vector3().crossVectors(forward, WORLD_UP).normalize();
    const movement = new Vector3()
      .addScaledVector(forward, motion.forward * speed)
      .addScaledVector(right, motion.right * speed)
      .addScaledVector(WORLD_UP, motion.up * speed);

    camera.position.add(movement);
    orbit.target.add(movement);
    orbit.update();
  });

  return (
    <OrbitControls
      ref={controls}
      makeDefault
      domElement={gl.domElement}
      enableDamping
      dampingFactor={0.08}
      enablePan
      enableRotate
      enableZoom
      minDistance={0.35}
      maxDistance={250}
      zoomSpeed={0.9}
      panSpeed={0.8}
      rotateSpeed={0.55}
    />
  );
}
