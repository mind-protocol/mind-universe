// The avatar is now an inhabitant of the DEFAULT city, not only of its solo
// fixture. These tests are the measured evidence that composing it into a
// populated universe (the postgres identity pilot) keeps the piloting loop's
// promise: the avatar is really present, it moves under a granted gate, and the
// city buildings around it never budge. The browser pane runs hidden here (RAF
// frozen), so this — not a screenshot — is where "controllable in the city" is
// actually proven.

import { describe, expect, it } from "vitest";
import {
  AVATAR_ENTITY_ID,
  avatarEntityAt,
  withAvatar
} from "./avatar-fixture";
import {
  applySceneAction,
  initialControlSession,
  motionBounds,
  observeActorMotion,
  type ActorScene
} from "./actor-control";
import { postgresPilotProjection } from "./postgres-pilot-fixture";

const START: readonly [number, number, number] = [0, 1.4, 4];

describe("avatar composed into the default city", () => {
  it("adds exactly the avatar and leaves the base universe untouched", () => {
    const base = postgresPilotProjection.view;
    const baseSize = base.entities.size;
    expect(base.entities.has(AVATAR_ENTITY_ID)).toBe(false);

    const city = withAvatar(base, [...START]);

    // The avatar is present, positioned where we placed it…
    expect(city.entities.get(AVATAR_ENTITY_ID)?.position).toEqual([...START]);
    // …and every original building is still there, count grown by exactly one.
    expect(city.entities.size).toBe(baseSize + 1);
    for (const id of base.entities.keys()) {
      expect(city.entities.has(id)).toBe(true);
    }
    // The base map was not mutated (no avatar leaked back into it).
    expect(base.entities.has(AVATAR_ENTITY_ID)).toBe(false);
    // Relations/transfers carry over unchanged — it is the same city.
    expect(city.relations).toBe(base.relations);
    expect(city.transfers).toBe(base.transfers);
  });

  it("renders the avatar as an embodiment inside the city", () => {
    const avatar = avatarEntityAt([...START]);
    expect(avatar.embodiment).toBeDefined();
    expect(avatar.id).toBe(AVATAR_ENTITY_ID);
  });

  it("moves only the avatar when piloted in the city — buildings hold", () => {
    let scene: ActorScene = {
      universe: withAvatar(postgresPilotProjection.view, [...START]),
      session: initialControlSession(AVATAR_ENTITY_ID)
    };
    const before = scene.universe;

    scene = applySceneAction(
      scene,
      { kind: "control", command: { kind: "request" } },
      motionBounds
    );
    scene = applySceneAction(
      scene,
      { kind: "move", displacement: [0.2, 0, 0] },
      motionBounds
    );

    // Independent observer over before/after: isolation held, within bounds.
    const obs = observeActorMotion(
      before,
      scene.universe,
      motionBounds,
      { kind: "granted", actor: AVATAR_ENTITY_ID }
    );
    expect(obs.ok).toBe(true);
    expect(obs.onlyBoundActorMoved).toBe(true);
    expect(obs.displacement).toBeCloseTo(0.2);

    // And concretely: the avatar advanced, a sampled building did not.
    const avatarBefore = before.entities.get(AVATAR_ENTITY_ID)!.position;
    const avatarAfter = scene.universe.entities.get(AVATAR_ENTITY_ID)!.position;
    expect(avatarAfter[0]).toBeCloseTo(avatarBefore[0] + 0.2);
    const sampleId = [...before.entities.keys()].find(
      (id) => id !== AVATAR_ENTITY_ID
    )!;
    expect(scene.universe.entities.get(sampleId)?.position).toEqual(
      before.entities.get(sampleId)?.position
    );
  });

  it("refuses to move in the city until control is granted", () => {
    let scene: ActorScene = {
      universe: withAvatar(postgresPilotProjection.view, [...START]),
      session: initialControlSession(AVATAR_ENTITY_ID)
    };
    scene = applySceneAction(
      scene,
      { kind: "move", displacement: [0.2, 0, 0] },
      motionBounds
    );
    expect(scene.universe.entities.get(AVATAR_ENTITY_ID)?.position).toEqual([
      ...START
    ]);
    expect(scene.session.lastRefusedReason).toBe("observer");
  });
});
