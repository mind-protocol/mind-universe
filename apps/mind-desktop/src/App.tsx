import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import {
  AVATAR_ENTITY_ID,
  AVATAR_MAPPING_AUTHORITY,
  avatarFixtureUniverse
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

const sceneReducer = (scene: ActorScene, action: SceneAction): ActorScene =>
  applySceneAction(scene, action, motionBounds);

export default function App() {
  const fixtureParam = new URLSearchParams(globalThis.location?.search).get(
    "fixture"
  );
  const avatarFixture = fixtureParam === "avatar";
  const audioFixture = fixtureParam === "audio";
  const [scene, dispatch] = useReducer(sceneReducer, undefined, () => ({
    universe: avatarFixtureUniverse(),
    session: initialControlSession(AVATAR_ENTITY_ID)
  }));

  // Audio loops start muted so the first "unmute" click doubles as the user
  // gesture that browser autoplay policy requires before sound may play.
  const [muted, setMuted] = useState(true);
  const audioUniverse = useMemo(() => audioFixtureUniverse(), []);

  // The request/release handshake. G asks the Universe for control of the bound
  // Actor (granted locally in fixture mode); Esc releases it back to observer.
  useEffect(() => {
    if (!avatarFixture) return;
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
  }, [avatarFixture]);

  const onMove = useCallback((displacement: Vec3) => {
    dispatch({ kind: "move", displacement });
  }, []);

  const universe = avatarFixture
    ? scene.universe
    : audioFixture
      ? audioUniverse
      : postgresPilotProjection.view;
  const gate = gateIntent(scene.session.control, scene.session.boundActor);
  const piloting = avatarFixture && gate.kind === "granted";
  const hasAudio = [...universe.entities.values()].some(
    (entity) => entity.audio !== undefined
  );

  const state = avatarFixture
    ? piloting
      ? `Piloting avatar · ZQSD to move · Esc to release${
          isFixtureGrant(scene.session.control) ? " · fixture grant" : ""
        }`
      : scene.session.lastRefusedReason
        ? `Observer · input refused (${scene.session.lastRefusedReason}) · press G to take control`
        : "Observer · press G to take control of the avatar"
    : !universe.synchronized
      ? "Awaiting a coherent Universe"
      : universe.control.kind === "granted"
        ? `In control of ${universe.control.actor}`
        : "Observer";

  return (
    <main data-fixture={avatarFixture ? "avatar" : undefined}>
      <World
        universe={universe}
        entityPresentation={postgresPilotProjection.entityPresentation}
        relationPresentation={postgresPilotProjection.relationPresentation}
        actorControl={
          avatarFixture ? { bounds: motionBounds, piloting, onMove } : undefined
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
        <p>{avatarFixture ? "Citizen embodiment" : "PostgreSQL identity pilot"}</p>
        <strong>
          {avatarFixture
            ? "Deterministic graph-mapping projection"
            : "Verified offline projection"}
        </strong>
        <span>
          {avatarFixture
            ? AVATAR_MAPPING_AUTHORITY
            : `revision ${postgresPilotProjection.authority.revision} - tick ${postgresPilotProjection.authority.tick}`}
        </span>
        <span>non-authoritative renderer fixture</span>
      </header>
      {!avatarFixture ? (
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
