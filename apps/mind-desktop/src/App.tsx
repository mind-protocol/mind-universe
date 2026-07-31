import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import {
  AVATAR_ENTITY_ID,
  AVATAR_MAPPING_AUTHORITY,
  avatarFixtureUniverse,
  withAvatar
} from "./avatar-fixture";
import { audioFixtureUniverse } from "./audio-fixture";
import { AudioLoops } from "./AudioLoops";
import {
  applySceneAction,
  gateIntent,
  initialControlSession,
  isFixtureGrant,
  motionBounds,
  type ActorScene,
  type SceneAction
} from "./actor-control";
import type { Vector3 as Vec3 } from "./contracts";
import { World } from "./World";
import { postgresPilotProjection } from "./postgres-pilot-fixture";
import { neighborhoodFixtureUniverse } from "./neighborhood-fixture";
import { ontologyRegistryProjection } from "./ontology-registry-fixture";
import { startSseStream, type LiveStore } from "./sse-stream";
import type { EntityPresentation, RelationPresentation } from "./postgres-pilot-fixture";

const sceneReducer = (scene: ActorScene, action: SceneAction): ActorScene =>
  applySceneAction(scene, action, motionBounds);

// The live-store stream carries geometry only (no label/predicate presentation),
// so a streamed view renders with empty presentation maps — nodes and streets
// still draw; the Bond renderer falls back to its own visual channels.
const NO_ENTITY_PRESENTATION: ReadonlyMap<string, EntityPresentation> = new Map();
const NO_RELATION_PRESENTATION: ReadonlyMap<string, RelationPresentation> = new Map();

// Vite sets `import.meta.env.DEV` in dev; the SSE bridge only exists then. Read it
// without depending on vite/client's ambient types.
const IS_DEV = Boolean(
  (import.meta as { env?: { DEV?: boolean } }).env?.DEV
);

// Where the avatar stands when it is composed into the pilot city projection —
// on the plaza in front of the buildings (which sit around z ≈ -1), lifted clear
// of the terrain so it reads as an inhabitant, not a foundation.
const DEFAULT_CITY_AVATAR_START: Vec3 = [0, 1.4, 4];

export default function App() {
  const fixtureParam = new URLSearchParams(globalThis.location?.search).get(
    "fixture"
  );
  const avatarFixture = fixtureParam === "avatar";
  const audioFixture = fixtureParam === "audio";
  const neighborhoodFixture = fixtureParam === "neighborhood";
  const pilotFixture = fixtureParam === "pilot";
  // The DEFAULT view is the WHOLE city, materialized from the graph store
  // (the ontology-registry projection — 379 nodes / 149 relations). Postgres is
  // dead: the store is the source of truth, so the former postgres-derived
  // defaults are gone. The postgres identity pilot survives only behind
  // ?fixture=pilot for the avatar-piloting demo.
  const ontologyFixture =
    !avatarFixture && !audioFixture && !neighborhoodFixture && !pilotFixture;
  // Piloting (the request/grant handshake + ActorControls) applies to the avatar
  // fixture and the pilot city; every other view is a static observer projection.
  const piloted = avatarFixture || pilotFixture;
  const [scene, dispatch] = useReducer(sceneReducer, undefined, () => ({
    // The avatar fixture is the avatar alone; the pilot city carries the avatar
    // composed into the postgres pilot universe so the piloting loop can move it.
    universe: avatarFixture
      ? avatarFixtureUniverse()
      : withAvatar(postgresPilotProjection.view, DEFAULT_CITY_AVATAR_START),
    session: initialControlSession(AVATAR_ENTITY_ID)
  }));

  // Audio loops start muted so the first "unmute" click doubles as the user
  // gesture that browser autoplay policy requires before sound may play.
  const [muted, setMuted] = useState(true);
  const audioUniverse = useMemo(() => audioFixtureUniverse(), []);
  const neighborhoodUniverse = useMemo(() => neighborhoodFixtureUniverse(), []);

  // The LIVE store, in real time (dev). For the default ontology view, subscribe
  // to the SSE bridge (vite-plugin-universe-stream), which materializes the store
  // (snapshot + events replay) and pushes a fresh frame batch on every store
  // change. The tested stream reducer folds each frame; when it is synchronized
  // and non-empty, its view supersedes the baked fixture below. If the bridge is
  // absent (production build) the fixture simply stands in.
  const [live, setLive] = useState<LiveStore | null>(null);
  useEffect(() => {
    if (!ontologyFixture || !IS_DEV) return;
    return startSseStream("/universe-stream", setLive);
  }, [ontologyFixture]);
  const liveView =
    live && live.state.view.synchronized && live.state.view.entities.size > 0
      ? live.state.view
      : null;

  // The request/release handshake. G asks the Universe for control of the bound
  // Actor (granted locally in fixture mode); Esc releases it back to observer.
  useEffect(() => {
    if (!piloted) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.repeat) return;
      if (event.code === "KeyG") {
        dispatch({ kind: "control", command: { kind: "request" } });
      } else if (event.code === "Escape") {
        dispatch({ kind: "control", command: { kind: "release" } });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [piloted]);

  const onMove = useCallback((displacement: Vec3) => {
    dispatch({ kind: "move", displacement });
  }, []);

  // Which materialized projection supplies entity/relation presentation. The
  // ontology-registry city (default) carries its own labels; the postgres pilot
  // supplies labels for the avatar/audio/neighborhood fixtures (whose ids simply
  // miss, which is harmless).
  const registry = ontologyFixture
    ? ontologyRegistryProjection
    : postgresPilotProjection;
  // Piloted views (avatar fixture, pilot city) read the scene reducer's universe
  // so the avatar's moves persist; the other fixtures are static.
  // The live store wins whenever the stream is synchronized (ontology view only);
  // otherwise the per-fixture baked universe stands in.
  const universe =
    liveView ??
    (piloted
      ? scene.universe
      : audioFixture
        ? audioUniverse
        : neighborhoodFixture
          ? neighborhoodUniverse
          : ontologyRegistryProjection.view);
  const gate = gateIntent(scene.session.control, scene.session.boundActor);
  const piloting = piloted && gate.kind === "granted";
  const hasAudio = [...universe.entities.values()].some(
    (entity) => entity.audio !== undefined
  );

  const state = piloted
    ? piloting
      ? `Piloting avatar · ZQSD to move · Esc to release${
          isFixtureGrant(scene.session.control) ? " · fixture grant" : ""
        }`
      : scene.session.lastRefusedReason
        ? `Observer · input refused (${scene.session.lastRefusedReason}) · press G to take control`
        : "Observer · press G to take control of the avatar"
    : !universe.synchronized
      ? "Awaiting a coherent Universe"
      : "Observer";

  return (
    <main
      data-fixture={
        avatarFixture ? "avatar" : ontologyFixture ? "ontology" : undefined
      }
    >
      <World
        universe={universe}
        entityPresentation={
          liveView
            ? live?.entityPresentation ?? NO_ENTITY_PRESENTATION
            : registry.entityPresentation
        }
        relationPresentation={
          liveView
            ? live?.relationPresentation ?? NO_RELATION_PRESENTATION
            : registry.relationPresentation
        }
        actorControl={
          piloted ? { bounds: motionBounds, piloting, onMove } : undefined
        }
      />
      <AudioLoops entities={universe.entities} muted={muted} />
      {hasAudio && (
        <button
          type="button"
          className="audio-toggle"
          aria-pressed={!muted}
          onClick={() => setMuted((value) => !value)}
        >
          {muted ? "🔇 Sound off" : "🔊 Sound on"}
        </button>
      )}
      <header className="projection-heading">
        <p>
          {avatarFixture
            ? "Citizen embodiment"
            : neighborhoodFixture
              ? "Board neighborhood glide"
              : ontologyFixture
                ? ontologyRegistryProjection.title
                : "PostgreSQL identity pilot"}
        </p>
        <strong>
          {avatarFixture
            ? "Deterministic graph-mapping projection"
            : neighborhoodFixture
              ? "Executed neighborhood arc — measured:semantic energy"
              : ontologyFixture
                ? liveView
                  ? `${liveView.entities.size} nodes · ${liveView.relations.size} relations · rev ${liveView.revision ?? "?"} · store en direct`
                  : ontologyRegistryProjection.subtitle
                : "Verified offline projection"}
        </strong>
        <span>
          {avatarFixture
            ? AVATAR_MAPPING_AUTHORITY
            : neighborhoodFixture
              ? "energy measured & executed · positions from layout engine"
              : ontologyFixture
                ? liveView
                  ? `flux SSE · ${live?.state.health ?? "connecting"} · maj temps réel`
                  : `materialized from ${ontologyRegistryProjection.source}`
                : `revision ${postgresPilotProjection.authority.revision} - tick ${postgresPilotProjection.authority.tick}`}
        </span>
        <span>
          {liveView ? "live store — non-authoritative renderer" : "non-authoritative renderer fixture"}
        </span>
      </header>
      {pilotFixture ? (
        <section className="projection-evidence" aria-label="Projection evidence">
        <div className="coverage">
          <span data-state={postgresPilotProjection.projection.boundedSituation}>
            bounded situation {postgresPilotProjection.projection.boundedSituation}
          </span>
          <span data-state={postgresPilotProjection.projection.universeCoverage}>
            universe {postgresPilotProjection.projection.universeCoverage}
          </span>
          <span data-state={postgresPilotProjection.projection.streamFreshness}>
            stream {postgresPilotProjection.projection.streamFreshness}
          </span>
        </div>
        <ul>
          {postgresPilotProjection.measurements.map((measurement) => (
            <li key={measurement.label} data-state={measurement.state}>
              <span>{measurement.label}</span>
              <strong>{measurement.state.replaceAll("_", " ")}</strong>
              <small>{measurement.detail}</small>
            </li>
          ))}
        </ul>
        </section>
      ) : null}
      <div className="world-status" role="status" aria-live="polite">
        {state}
      </div>
    </main>
  );
}
