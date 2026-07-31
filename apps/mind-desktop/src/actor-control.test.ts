import { describe, expect, it } from "vitest";
import type { ControlState, MaterializedEntity, Vector3 } from "./contracts";
import { NEUTRAL_DYNAMICS } from "./entity-dynamics";
import type { UniverseView } from "./universe-state";
import { emptyUniverseView } from "./universe-state";
import {
  actorDisplacement,
  applyActorMotion,
  applyControlCommand,
  applySceneAction,
  gateIntent,
  initialControlSession,
  motionBounds,
  observeActorMotion,
  validateMotionBounds,
  type ActorScene,
  type IntentGate,
  type MotionBasis,
  type MotionBounds
} from "./actor-control";

const AVATAR = motionBounds.boundActor;
const OTHER = "fixture:actor:bystander";

const BASIS: MotionBasis = {
  forward: [0, 0, -1],
  right: [1, 0, 0],
  up: [0, 1, 0]
};

const full = { forward: 1, right: 0, up: 0, speedMultiplier: 1 } as const;

function entity(id: string, position: Vector3): MaterializedEntity {
  return {
    id,
    generation: 0,
    position,
    visual: {
      primitive: "unknown",
      motion: "still",
      material: { color: "#fff", emissive: "#fff", emissiveIntensity: 0, opacity: 1, scale: 1 }
    },
    dynamics: NEUTRAL_DYNAMICS
  };
}

function viewWith(...entities: MaterializedEntity[]): UniverseView {
  return {
    ...emptyUniverseView(),
    synchronized: true,
    entities: new Map(entities.map((e) => [e.id, e]))
  };
}

const granted: ControlState = {
  kind: "granted",
  actor: AVATAR,
  capabilityReceipt: "test"
};

describe("gate", () => {
  it("authorizes only a grant over the bound actor", () => {
    expect(gateIntent(granted, AVATAR).kind).toBe("granted");
  });

  it("refuses observer, requested and refused states with a reason", () => {
    const states: ControlState[] = [
      { kind: "observer" },
      { kind: "requested", actor: AVATAR, requestId: "r" },
      { kind: "refused", actor: AVATAR, reason: "no capability" }
    ];
    for (const control of states) {
      const gate = gateIntent(control, AVATAR);
      expect(gate.kind).toBe("refused");
    }
  });

  it("refuses a grant over a different actor", () => {
    const gate = gateIntent({ ...granted, actor: OTHER }, AVATAR);
    expect(gate.kind).toBe("refused");
  });
});

describe("bounded displacement", () => {
  it("moves along the camera-relative forward axis", () => {
    const d = actorDisplacement(full, 0.016, BASIS, motionBounds);
    expect(d[2]).toBeLessThan(0); // forward basis is -Z
    expect(d[0]).toBeCloseTo(0);
  });

  it("withholds the vertical axis on the shipped (grounded) fixture — no flying", () => {
    const upIntent = { forward: 0, right: 0, up: 1, speedMultiplier: 1 } as const;
    const d = actorDisplacement(upIntent, 0.016, BASIS, motionBounds);
    expect(d).toEqual([0, 0, 0]); // the contract denies `up`, so nothing lifts
  });

  it("withholds any axis the contract does not permit", () => {
    const grounded: MotionBounds = { ...motionBounds, axes: { forward: true, right: true, up: false } };
    const upIntent = { forward: 0, right: 0, up: 1, speedMultiplier: 1 } as const;
    expect(actorDisplacement(upIntent, 0.05, BASIS, grounded)).toEqual([0, 0, 0]);
  });

  it("never exceeds max_tick_displacement, even on a stalled frame", () => {
    const d = actorDisplacement(full, 100, BASIS, motionBounds);
    expect(Math.hypot(...d)).toBeLessThanOrEqual(motionBounds.maxTickDisplacement + 1e-9);
  });

  it("treats NaN or non-positive dt as no motion", () => {
    expect(actorDisplacement(full, Number.NaN, BASIS, motionBounds)).toEqual([0, 0, 0]);
    expect(actorDisplacement(full, 0, BASIS, motionBounds)).toEqual([0, 0, 0]);
    expect(actorDisplacement(full, -1, BASIS, motionBounds)).toEqual([0, 0, 0]);
  });
});

describe("apply motion", () => {
  it("moves only the bound actor and leaves bystanders untouched", () => {
    const before = viewWith(entity(AVATAR, [0, 0, 0]), entity(OTHER, [5, 0, 0]));
    const after = applyActorMotion(before, AVATAR, [0.1, 0, 0]);
    expect(after.entities.get(AVATAR)?.position).toEqual([0.1, 0, 0]);
    expect(after.entities.get(OTHER)?.position).toEqual([5, 0, 0]);
  });

  it("returns the same view when the actor is absent — never invents one", () => {
    const before = viewWith(entity(OTHER, [5, 0, 0]));
    expect(applyActorMotion(before, AVATAR, [0.1, 0, 0])).toBe(before);
  });
});

describe("observer", () => {
  const refused: IntentGate = { kind: "refused", reason: "observer" };
  const grantedGate: IntentGate = { kind: "granted", actor: AVATAR };

  it("passes a legitimate granted move within bounds", () => {
    const before = viewWith(entity(AVATAR, [0, 0, 0]), entity(OTHER, [5, 0, 0]));
    const after = applyActorMotion(before, AVATAR, [0.2, 0, 0]);
    const obs = observeActorMotion(before, after, motionBounds, grantedGate);
    expect(obs.ok).toBe(true);
    expect(obs.displacement).toBeCloseTo(0.2);
  });

  // Observer-validation: the observer must FAIL on a buggy applier.
  it("fails when a bystander is moved (isolation broken)", () => {
    const before = viewWith(entity(AVATAR, [0, 0, 0]), entity(OTHER, [5, 0, 0]));
    const after = viewWith(entity(AVATAR, [0.1, 0, 0]), entity(OTHER, [5.1, 0, 0]));
    expect(observeActorMotion(before, after, motionBounds, grantedGate).onlyBoundActorMoved).toBe(false);
  });

  it("fails when the applied displacement exceeds the bound", () => {
    const before = viewWith(entity(AVATAR, [0, 0, 0]));
    const after = viewWith(entity(AVATAR, [10, 0, 0]));
    expect(observeActorMotion(before, after, motionBounds, grantedGate).boundRespected).toBe(false);
  });

  it("fails when anything moved while the gate was refused", () => {
    const before = viewWith(entity(AVATAR, [0, 0, 0]));
    const after = applyActorMotion(before, AVATAR, [0.1, 0, 0]);
    const obs = observeActorMotion(before, after, motionBounds, refused);
    expect(obs.staticWhenRefused).toBe(false);
    expect(obs.ok).toBe(false);
  });
});

describe("bounds validation", () => {
  it("rejects a contract that permits no axis", () => {
    expect(() =>
      validateMotionBounds({
        schema_version: "actor-control/1",
        bound_actor: AVATAR,
        max_speed: 1,
        max_tick_displacement: 1,
        axes: { forward: false, right: false, up: false }
      })
    ).toThrow(/at least one axis/);
  });

  it("rejects a non-positive max_speed rather than defaulting", () => {
    expect(() =>
      validateMotionBounds({
        schema_version: "actor-control/1",
        bound_actor: AVATAR,
        max_speed: 0,
        max_tick_displacement: 1,
        axes: { forward: true, right: false, up: false }
      })
    ).toThrow(/max_speed/);
  });

  it("loads the shipped fixture as a valid, grounded bound", () => {
    expect(motionBounds.boundActor).toBe(AVATAR);
    expect(motionBounds.axes.up).toBe(false); // grounded: the avatar cannot fly
  });
});

describe("control session + scene", () => {
  const bounds: MotionBounds = motionBounds;

  it("grants on request and releases back to observer", () => {
    let session = initialControlSession(AVATAR);
    session = applyControlCommand(session, { kind: "request" });
    expect(session.control.kind).toBe("granted");
    session = applyControlCommand(session, { kind: "release" });
    expect(session.control.kind).toBe("observer");
  });

  it("refuses to move under observer and records the reason", () => {
    const scene: ActorScene = {
      universe: viewWith(entity(AVATAR, [0, 0, 0])),
      session: initialControlSession(AVATAR)
    };
    const next = applySceneAction(scene, { kind: "move", displacement: [0.2, 0, 0] }, bounds);
    expect(next.universe.entities.get(AVATAR)?.position).toEqual([0, 0, 0]);
    expect(next.session.lastRefusedReason).toBe("observer");
  });

  it("moves the avatar once control is granted", () => {
    let scene: ActorScene = {
      universe: viewWith(entity(AVATAR, [0, 0, 0])),
      session: initialControlSession(AVATAR)
    };
    scene = applySceneAction(scene, { kind: "control", command: { kind: "request" } }, bounds);
    scene = applySceneAction(scene, { kind: "move", displacement: [0.2, 0, 0] }, bounds);
    expect(scene.universe.entities.get(AVATAR)?.position).toEqual([0.2, 0, 0]);
  });
});
