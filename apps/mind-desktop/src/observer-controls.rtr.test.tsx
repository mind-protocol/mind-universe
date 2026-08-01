// @vitest-environment happy-dom
//
// Measures the REAL ObserverControls useFrame/reset glue via
// @react-three/test-renderer: mount the controller, read the actual camera the
// scene uses, and assert the first-person eye stands on the floor at a fixed
// height — at rest and while walking over relief. Complements the pure
// first-person-look unit tests, which can't see the live camera.

import ReactThreeTestRenderer from "@react-three/test-renderer";
import { useThree } from "@react-three/fiber";
import { afterEach, describe, expect, it } from "vitest";
import type { Camera } from "three";
import { ObserverControls } from "./ObserverControls";

const EYE = 1.8;
// A tilted, non-flat floor so "rides the terrain" is actually exercised.
const slope = (x: number, z: number) => 0.2 * x - 0.15 * z + 1;

let captured: Camera | null = null;
function CaptureCamera() {
  captured = useThree((state) => state.camera);
  return null;
}

function press(code: string) {
  window.dispatchEvent(new KeyboardEvent("keydown", { code }));
}
function release(code: string) {
  window.dispatchEvent(new KeyboardEvent("keyup", { code }));
}

afterEach(() => {
  for (const code of ["KeyZ", "KeyD", "KeyW", "KeyS"]) release(code);
  captured = null;
});

describe("ObserverControls runtime eye height (measured via test-renderer)", () => {
  it("stands the eye at floor + 1.8 at rest, ignoring the opening y", async () => {
    const renderer = await ReactThreeTestRenderer.create(
      <>
        <CaptureCamera />
        <ObserverControls
          initialCamera={[3, 999, -4]}
          groundHeight={slope}
          eyeHeight={EYE}
        />
      </>
    );
    await renderer.advanceFrames(1, 1 / 60);
    const cam = captured!;
    expect(cam.position.x).toBeCloseTo(3);
    expect(cam.position.z).toBeCloseTo(-4);
    expect(cam.position.y).toBeCloseTo(slope(3, -4) + EYE); // opening y=999 discarded
    await renderer.unmount();
  });

  it("keeps the eye pinned to floor + 1.8 as it walks over the relief", async () => {
    const renderer = await ReactThreeTestRenderer.create(
      <>
        <CaptureCamera />
        <ObserverControls
          initialCamera={[0, 2, 9]}
          groundHeight={slope}
          eyeHeight={EYE}
        />
      </>
    );
    press("KeyD"); // strafe so x changes and the floor height under the eye changes
    await renderer.advanceFrames(20, 1 / 60);
    release("KeyD");
    const cam = captured!;
    // Whatever (x, z) the walk reached, the invariant holds exactly.
    expect(cam.position.y).toBeCloseTo(slope(cam.position.x, cam.position.z) + EYE);
    await renderer.unmount();
  });

  // The live city re-renders whenever a store frame lands (and whenever the SSE
  // bridge drops and reconnects), handing down a FRESH framing array each time.
  // A re-render that frames the same place is not a framing change: it must not
  // drag the observer back to the opening viewpoint.
  it("leaves the walked observer where they stand when the parent re-renders", async () => {
    const scene = (framing: [number, number, number]) => (
      <>
        <CaptureCamera />
        <ObserverControls
          initialCamera={framing}
          groundHeight={slope}
          eyeHeight={EYE}
        />
      </>
    );
    const renderer = await ReactThreeTestRenderer.create(scene([0, 2, 9]));
    press("KeyZ"); // walk forward, away from the opening framing
    await renderer.advanceFrames(20, 1 / 60);
    release("KeyZ");
    const walked = captured!.position.clone();
    expect(Math.hypot(walked.x, walked.z - 9)).toBeGreaterThan(0.2); // it really moved

    // Same framing VALUES, brand-new array — exactly what a re-render hands down.
    await renderer.update(scene([0, 2, 9]));
    await renderer.advanceFrames(1, 1 / 60);
    expect(captured!.position.x).toBeCloseTo(walked.x);
    expect(captured!.position.z).toBeCloseTo(walked.z);
    await renderer.unmount();
  });

  // The opening framing may still re-aim an eye nobody has touched — the city
  // arriving late and widening the shot must land, not be ignored.
  it("re-frames an untouched observer when the framing itself moves", async () => {
    const scene = (framing: [number, number, number]) => (
      <>
        <CaptureCamera />
        <ObserverControls
          initialCamera={framing}
          groundHeight={slope}
          eyeHeight={EYE}
        />
      </>
    );
    const renderer = await ReactThreeTestRenderer.create(scene([0, 2, 9]));
    await renderer.advanceFrames(1, 1 / 60);
    expect(captured!.position.z).toBeCloseTo(9);

    await renderer.update(scene([0, 40, 80])); // the wide-city framing
    await renderer.advanceFrames(1, 1 / 60);
    expect(captured!.position.z).toBeCloseTo(80);
    expect(captured!.position.y).toBeCloseTo(slope(0, 80) + EYE);
    await renderer.unmount();
  });
});
