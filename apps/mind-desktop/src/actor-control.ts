// Actor-control loop (piloting the avatar) — the renderer-free, deterministic
// core beneath ActorControls.tsx. Pattern: *gated bounded-intent projection*.
//
//   input (ZQSD) -> ActorIntent -> IntentGate(control, boundActor)
//     -> refused  : no entity moves, refusal is surfaced (never a silent zero)
//     -> granted  : camera-relative displacement, clamped to bounds, applied to
//                   the bound Actor's entity position ONLY.
//
// The bounds are graph-declared authority loaded from
// fixtures/desktop-control/actor-control-bounds.json (consumed, never hard-coded
// here) — the same shape the visual embodiment authority uses. A follow-up Rust
// loop can materialize this contract as a content-addressed Asset with readback
// (mirroring crates/universe-assets/src/visual.rs); the runtime already consumes
// the fixture the materialization will emit.

import type { ControlState, EntityId, Vector3 } from "./contracts";
import type { UniverseView } from "./universe-state";
import type { ObserverMotion } from "./observer-controls";
import boundsFixture from "../../../fixtures/desktop-control/actor-control-bounds.json";

export interface MotionAxes {
  readonly forward: boolean;
  readonly right: boolean;
  readonly up: boolean;
}

export interface MotionBounds {
  readonly boundsId: string;
  readonly boundActor: EntityId;
  /** World units per second at full intent (before speed multiplier). */
  readonly maxSpeed: number;
  /** Hard clamp on the magnitude of any single applied displacement. */
  readonly maxTickDisplacement: number;
  readonly axes: MotionAxes;
}

// An ActorIntent is axis-aligned input in [-1, 1] with a deliberate speed mode —
// identical shape to ObserverMotion, so ZQSD maps the same way it does for the
// camera. The gate, not the intent, decides whether it becomes motion.
export type ActorIntent = ObserverMotion;

export type IntentGate =
  | { readonly kind: "granted"; readonly actor: EntityId }
  | { readonly kind: "refused"; readonly reason: string };

/** A right-handed world basis the displacement is projected onto. */
export interface MotionBasis {
  readonly forward: Vector3;
  readonly right: Vector3;
  readonly up: Vector3;
}

const ZERO: Vector3 = [0, 0, 0];
// Longest plausible frame we trust; a stalled tab must not integrate a huge dt
// into a teleport. Beyond this we still clamp by maxTickDisplacement, but we cap
// dt first so speed stays meaningful.
const MAX_TRUSTED_DT = 0.05;

export const ACTOR_CONTROL_AUTHORITY = boundsFixture.authority_id;

/**
 * Validates the graph-declared bounds and returns them, or throws — never a
 * convenient default. An unbound or non-positive contract is a refusal to
 * operate, not a silently-corrected value.
 */
export function validateMotionBounds(raw: unknown): MotionBounds {
  const record = raw as Record<string, unknown>;
  if (record?.schema_version !== "actor-control/1") {
    throw new Error("actor-control bounds: schema_version must be actor-control/1");
  }
  const boundActor = record.bound_actor;
  if (typeof boundActor !== "string" || boundActor.length === 0) {
    throw new Error("actor-control bounds: bound_actor must be a non-empty id");
  }
  const maxSpeed = record.max_speed;
  if (typeof maxSpeed !== "number" || !Number.isFinite(maxSpeed) || maxSpeed <= 0) {
    throw new Error("actor-control bounds: max_speed must be a positive number");
  }
  const maxTickDisplacement = record.max_tick_displacement;
  if (
    typeof maxTickDisplacement !== "number" ||
    !Number.isFinite(maxTickDisplacement) ||
    maxTickDisplacement <= 0
  ) {
    throw new Error("actor-control bounds: max_tick_displacement must be positive");
  }
  const axesRaw = record.axes as Record<string, unknown> | undefined;
  const axes: MotionAxes = {
    forward: axesRaw?.forward === true,
    right: axesRaw?.right === true,
    up: axesRaw?.up === true
  };
  if (!axes.forward && !axes.right && !axes.up) {
    throw new Error("actor-control bounds: at least one axis must be permitted");
  }
  return {
    boundsId: String(record.bounds_id ?? "unknown"),
    boundActor,
    maxSpeed,
    maxTickDisplacement,
    axes
  };
}

export const motionBounds: MotionBounds = validateMotionBounds(boundsFixture);

/**
 * The gate: motion is authorized only when control is `granted` over the same
 * Actor the bounds bind. Every other state — observer, requested, refused, or a
 * grant over a different Actor — is a refusal carrying the reason, so the UI can
 * surface it instead of hiding an ungranted input as a no-op.
 */
export function gateIntent(control: ControlState, boundActor: EntityId): IntentGate {
  if (control.kind === "granted") {
    return control.actor === boundActor
      ? { kind: "granted", actor: boundActor }
      : { kind: "refused", reason: `granted to ${control.actor}, not the avatar` };
  }
  return { kind: "refused", reason: control.kind };
}

function vlength([x, y, z]: Vector3): number {
  return Math.hypot(x, y, z);
}

/**
 * Projects an intent onto the world basis, integrates over a trusted dt, and
 * clamps the result to the bounds. Withheld axes contribute nothing; a NaN or
 * non-positive dt yields no motion; the final magnitude never exceeds
 * maxTickDisplacement. Pure and renderer-free.
 */
export function actorDisplacement(
  intent: ActorIntent,
  dtSeconds: number,
  basis: MotionBasis,
  bounds: MotionBounds
): Vector3 {
  if (!Number.isFinite(dtSeconds) || dtSeconds <= 0) return ZERO;
  const dt = Math.min(dtSeconds, MAX_TRUSTED_DT);
  const forward = bounds.axes.forward ? intent.forward : 0;
  const right = bounds.axes.right ? intent.right : 0;
  const up = bounds.axes.up ? intent.up : 0;
  if (forward === 0 && right === 0 && up === 0) return ZERO;

  const speed = bounds.maxSpeed * intent.speedMultiplier * dt;
  let x = 0;
  let y = 0;
  let z = 0;
  x += basis.forward[0] * forward + basis.right[0] * right + basis.up[0] * up;
  y += basis.forward[1] * forward + basis.right[1] * right + basis.up[1] * up;
  z += basis.forward[2] * forward + basis.right[2] * right + basis.up[2] * up;

  let displacement: Vector3 = [x * speed, y * speed, z * speed];
  const magnitude = vlength(displacement);
  if (magnitude > bounds.maxTickDisplacement) {
    const scale = bounds.maxTickDisplacement / magnitude;
    displacement = [
      displacement[0] * scale,
      displacement[1] * scale,
      displacement[2] * scale
    ];
  }
  return displacement;
}

/**
 * Applies a world displacement to the bound Actor's position and returns a new
 * view. If the Actor is absent it returns the SAME view reference — it never
 * invents an entity to move.
 */
export function applyActorMotion(
  view: UniverseView,
  actor: EntityId,
  displacement: Vector3
): UniverseView {
  const entity = view.entities.get(actor);
  if (!entity) return view;
  if (displacement[0] === 0 && displacement[1] === 0 && displacement[2] === 0) {
    return view;
  }
  const entities = new Map(view.entities);
  entities.set(actor, {
    ...entity,
    position: [
      entity.position[0] + displacement[0],
      entity.position[1] + displacement[1],
      entity.position[2] + displacement[2]
    ]
  });
  return { ...view, entities };
}

export interface MotionObservation {
  /** Only the bound Actor's position differs between before and after. */
  readonly onlyBoundActorMoved: boolean;
  /** Magnitude actually applied to the bound Actor. */
  readonly displacement: number;
  /** The applied magnitude did not exceed maxTickDisplacement. */
  readonly boundRespected: boolean;
  /** Under a refused gate, nothing moved at all. */
  readonly staticWhenRefused: boolean;
  readonly ok: boolean;
}

const EPSILON = 1e-9;

/**
 * Independent observer: reads two views and the gate, and reports whether the
 * loop held its promise — only the bound Actor moved, within bounds, and nothing
 * moved when the gate refused. It trusts the views, not the applier's claims.
 */
export function observeActorMotion(
  before: UniverseView,
  after: UniverseView,
  bounds: MotionBounds,
  gate: IntentGate
): MotionObservation {
  let onlyBoundActorMoved = true;
  let displacement = 0;

  const ids = new Set<EntityId>([
    ...before.entities.keys(),
    ...after.entities.keys()
  ]);
  for (const id of ids) {
    const from = before.entities.get(id);
    const to = after.entities.get(id);
    if (!from || !to) {
      // An entity appearing or vanishing is not this loop's business — treat it
      // as a violation of isolation rather than silently ignoring it.
      if (id !== bounds.boundActor) onlyBoundActorMoved = false;
      continue;
    }
    const delta = vlength([
      to.position[0] - from.position[0],
      to.position[1] - from.position[1],
      to.position[2] - from.position[2]
    ]);
    if (id === bounds.boundActor) {
      displacement = delta;
    } else if (delta > EPSILON) {
      onlyBoundActorMoved = false;
    }
  }

  const boundRespected = displacement <= bounds.maxTickDisplacement + EPSILON;
  const staticWhenRefused = gate.kind === "refused" ? displacement <= EPSILON : true;
  return {
    onlyBoundActorMoved,
    displacement,
    boundRespected,
    staticWhenRefused,
    ok: onlyBoundActorMoved && boundRespected && staticWhenRefused
  };
}

// ---------------------------------------------------------------------------
// Control session — the request/grant/release handshake over the bound Actor.
// ---------------------------------------------------------------------------

export interface ControlSession {
  readonly control: ControlState;
  readonly boundActor: EntityId;
  /** The reason the most recent intent was refused, for the HUD. */
  readonly lastRefusedReason: string | null;
}

export const initialControlSession = (boundActor: EntityId): ControlSession => ({
  control: { kind: "observer" },
  boundActor,
  lastRefusedReason: null
});

export type ControlCommand =
  | { readonly kind: "request" }
  | { readonly kind: "release" };

// In fixture mode there is no authority server; a request is granted locally
// with a receipt that is HONEST about its provenance — it is a fixture grant,
// not a Universe-issued capability. The HUD shows this distinction.
const FIXTURE_RECEIPT_PREFIX = "fixture:capability:actor-control:";

export function applyControlCommand(
  session: ControlSession,
  command: ControlCommand
): ControlSession {
  switch (command.kind) {
    case "request":
      if (session.control.kind === "granted") return session;
      return {
        ...session,
        control: {
          kind: "granted",
          actor: session.boundActor,
          capabilityReceipt: `${FIXTURE_RECEIPT_PREFIX}${session.boundActor}`
        },
        lastRefusedReason: null
      };
    case "release":
      if (session.control.kind === "observer") return session;
      return { ...session, control: { kind: "observer" }, lastRefusedReason: null };
  }
}

export const isFixtureGrant = (control: ControlState): boolean =>
  control.kind === "granted" &&
  control.capabilityReceipt.startsWith(FIXTURE_RECEIPT_PREFIX);

// ---------------------------------------------------------------------------
// Scene reducer — composes the piloted universe with the control session. The
// gate is enforced here too (defense in depth): a `move` under a refused gate
// records the reason and leaves the universe untouched.
// ---------------------------------------------------------------------------

export interface ActorScene {
  readonly universe: UniverseView;
  readonly session: ControlSession;
}

export type SceneAction =
  | { readonly kind: "control"; readonly command: ControlCommand }
  | { readonly kind: "move"; readonly displacement: Vector3 };

export function applySceneAction(
  scene: ActorScene,
  action: SceneAction,
  bounds: MotionBounds
): ActorScene {
  switch (action.kind) {
    case "control":
      return { ...scene, session: applyControlCommand(scene.session, action.command) };
    case "move": {
      const gate = gateIntent(scene.session.control, scene.session.boundActor);
      if (gate.kind === "refused") {
        if (scene.session.lastRefusedReason === gate.reason) return scene;
        return {
          ...scene,
          session: { ...scene.session, lastRefusedReason: gate.reason }
        };
      }
      const universe = applyActorMotion(
        scene.universe,
        scene.session.boundActor,
        action.displacement
      );
      if (universe === scene.universe) return scene;
      return { ...scene, universe };
    }
  }
}
