import { useReducer } from "react";
import {
  AVATAR_MAPPING_AUTHORITY,
  avatarFixtureUniverse
} from "./avatar-fixture";
import { World } from "./World";
import { postgresPilotProjection } from "./postgres-pilot-fixture";
import { applyUniverseEvent } from "./universe-state";

export default function App() {
  const avatarFixture =
    new URLSearchParams(globalThis.location?.search).get("fixture") === "avatar";
  const [universe] = useReducer(
    applyUniverseEvent,
    avatarFixture ? avatarFixtureUniverse() : postgresPilotProjection.view
  );
  const state = avatarFixture
    ? "Deterministic graph projection fixture · Observer"
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
      />
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
