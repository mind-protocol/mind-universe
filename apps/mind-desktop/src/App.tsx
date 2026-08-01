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
import { magicCupFixtureUniverse } from "./appearance-fixture";
import { emptyUniverseView } from "./universe-state";
import { startSseStream, type LiveStore } from "./sse-stream";
import type { EntityPresentation, RelationPresentation } from "./postgres-pilot-fixture";

const sceneReducer = (scene: ActorScene, action: SceneAction): ActorScene =>
  applySceneAction(scene, action, motionBounds);

// The live-store stream carries geometry only (no label/predicate presentation),
// so a streamed view renders with empty presentation maps — nodes and streets
// still draw; the Bond renderer falls back to its own visual channels.
const NO_ENTITY_PRESENTATION: ReadonlyMap<string, EntityPresentation> = new Map();
const NO_RELATION_PRESENTATION: ReadonlyMap<string, RelationPresentation> = new Map();

// The city view has NO baked offline source: the JS sunflower materializer
// (materialize-city + companions) was deleted as an anti-world-first projection
// (position = f(kind, arrival-index), ignoring topology). Position is a
// construction — a construct is where the citizen built it — so the view is fed
// ONLY by the live store stream, projected by the Rust bin `desktop_world_snapshot`
// (built placements win; the layout kernel only scaffolds what nothing has placed).
// When that stream serves nothing, the fallback is honestly EMPTY. It is NEVER a
// fixture city: a stand-in for a store we could not read would be a lie about the
// world, not a graceful degradation.
const NO_CITY = {
  title: "La ville — registre canonique en direct",
  subtitle: "aucune observation : le flux ne sert rien (pas de projection hors-ligne)",
  source: "store en direct — aucune substitution",
  view: emptyUniverseView(),
  entityPresentation: NO_ENTITY_PRESENTATION,
  relationPresentation: NO_RELATION_PRESENTATION
};

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
  // `?fixture=magic-cup` shows the cup ALONE (a clean one-construct demo). The
  // DEFAULT root view (below) instead places the cup INTO the populated pilot city.
  const magicCupFixture = fixtureParam === "magic-cup";
  // The DEFAULT view is THE CITY, live: the desktop is branched to the store the
  // citizens actually inhabit, streamed as it changes. Every fixture (the postgres
  // identity pilot, the cup, the avatar, the audio and neighborhood slices) is now
  // an explicit `?fixture=` detour. `?fixture=ontology` stays as an alias for the
  // default, since that is what the city view used to be called.
  const cityFixture = fixtureParam === "ontology";
  const liveCity =
    cityFixture ||
    !(
      avatarFixture ||
      audioFixture ||
      neighborhoodFixture ||
      pilotFixture ||
      magicCupFixture
    );
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
  const magicCupUniverse = useMemo(() => magicCupFixtureUniverse(), []);
  // The LIVE city, in real time (dev): subscribe to the SSE endpoint
  // /universe-stream, served by the dev transport that forwards what the Rust
  // projector reads from the store. The tested stream reducer folds each frame;
  // when it is synchronized and non-empty, its view IS the city. Frames stop, or
  // never start, and the view stays empty — the app never substitutes a fixture.
  const [live, setLive] = useState<LiveStore | null>(null);
  useEffect(() => {
    if (!liveCity || !IS_DEV) return;
    return startSseStream("/universe-stream", setLive);
  }, [liveCity]);
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

  // Which materialized projection supplies entity/relation presentation. The live
  // city carries its own labels on the wire (the projector emits each node's graph
  // symbol), so it needs no offline registry; the postgres pilot supplies labels
  // for the avatar/audio/neighborhood fixtures (whose ids simply miss, harmlessly).
  const registry = liveCity ? NO_CITY : postgresPilotProjection;
  // Piloted views (avatar fixture, pilot city) read the scene reducer's universe
  // so the avatar's moves persist; the other fixtures are static.
  // The live city wins whenever the stream is synchronized; when it is not, the
  // city view is empty rather than a fixture wearing the city's name.
  const universe =
    liveView ??
    (piloted
      ? scene.universe
      : audioFixture
        ? audioUniverse
        : neighborhoodFixture
          ? neighborhoodUniverse
          : magicCupFixture
            ? magicCupUniverse
            : NO_CITY.view);
  // How much of what is on screen is a CONSTRUCTION and how much is a proposal.
  // Counted, not assumed: a node whose frame declared no placement provenance is
  // in neither bucket (it is `unknown`), so the two numbers may sum to less than
  // the node count rather than silently absorbing the undeclared into "scaffold".
  const placementCensus = useMemo(() => {
    let built = 0;
    let scaffold = 0;
    for (const entity of universe.entities.values()) {
      if (entity.placement === "built") built += 1;
      else if (entity.placement === "scaffold") scaffold += 1;
    }
    return { built, scaffold };
  }, [universe]);

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
        avatarFixture ? "avatar" : liveCity ? "ontology" : undefined
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
          {magicCupFixture
            ? "Appearance toolkit — affordance materialization"
            : avatarFixture
              ? "Citizen embodiment"
              : neighborhoodFixture
                ? "Board neighborhood glide"
                : pilotFixture
                  ? "PostgreSQL identity pilot"
                  : NO_CITY.title}
        </p>
        <strong>
          {magicCupFixture
            ? "magic cup — form = union of its affordances (contain · sip · fill · enchant)"
            : avatarFixture
              ? "Deterministic graph-mapping projection"
              : neighborhoodFixture
                ? "Executed neighborhood arc — measured:semantic energy"
                : pilotFixture
                  ? "Verified offline projection"
                  : liveView
                    ? `${liveView.entities.size} nœuds · ${liveView.relations.size} relations · rev ${liveView.revision ?? "?"} · store en direct`
                    : NO_CITY.subtitle}
        </strong>
        <span>
          {avatarFixture
            ? AVATAR_MAPPING_AUTHORITY
            : neighborhoodFixture
              ? "energy measured & executed · positions from layout engine"
              : pilotFixture
                ? `revision ${postgresPilotProjection.authority.revision} - tick ${postgresPilotProjection.authority.tick}`
                : liveView
                  ? `flux SSE · ${live?.state.health ?? "connecting"} · ${placementCensus.built} bâti · ${placementCensus.scaffold} échafaudé`
                  : NO_CITY.source}
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
