// Vite dev plugin: stream the LIVE city to the browser over SSE.
//
//   GET /universe-stream  ->  text/event-stream
//     - on connect, and on every change under the store directory: run the
//       world-first projector and forward the frames it wrote, verbatim.
//
// THE PLUGIN HOLDS NO LOGIC. It does not read the store, does not decide where a
// node is, does not caption a frame, does not rank or filter anything, and does
// not renumber a sequence. Every judgement about the world — which entities exist,
// where each one stands, whether that place was BUILT by a citizen or merely
// SCAFFOLDED by the layout kernel, what visual authority resolves it — is made by
// the Rust bin `desktop_world_snapshot`, which reads the store and composes the
// observation. This file frames, encodes, and routes. A rule that could change
// what a caller learns about the Universe would be Universe logic, and would
// belong in the projector or in a construct, never here.
//
// Consequences of that, deliberately:
//   - the projector is a PREBUILT executable; if it is absent, this endpoint
//     serves NOTHING and says so. It never falls back to a baked fixture, because
//     a fixture standing in for a missing store is a lie about the world.
//   - each batch is self-contained: the projector always emits `snapshot` at
//     sequence 1, which resets the client view, so no cross-batch counter is kept
//     here (keeping one would be this file deciding something).
//   - `apply: "serve"` keeps it inert in production builds.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, watch } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, "../../..");
const ENDPOINT = "/universe-stream";

// The live city.
const STORE_DIR = resolve(REPO_ROOT, "artifacts/ontology-registry/current/store");
// Where the projector writes; read straight back and forwarded. Under artifacts/,
// which is not tracked — this is a cache of a projection, not a source.
const PROJECTION_DIR = resolve(REPO_ROOT, "artifacts/desktop-world-snapshot/live");
const FRAMES_FILE = resolve(PROJECTION_DIR, "world-snapshot-frames.json");

// The world-first projector. Built, not invoked through cargo: a dev transport
// must not silently rebuild the trusted computing base on an fs event.
const PROJECTOR = resolve(
  REPO_ROOT,
  process.platform === "win32"
    ? "target/release/desktop_world_snapshot.exe"
    : "target/release/desktop_world_snapshot"
);
const BUILD_HINT =
  "cargo build --release -p universe-e2e --bin desktop_world_snapshot";

// The graph-materialized visual authority the PROJECTOR resolves appearance
// against. Passed through as arguments; this file never opens them.
const VISUAL_CATALOG = resolve(
  REPO_ROOT,
  "fixtures/assets/visual-embodiment-catalog.json"
);
// The DECLARED resolution policy — provenance-based, binding by producing
// toolkit. The v0-shaped table was passed here only because the Rust authority
// could not deserialize this document; now that it can, the policy the world
// declares is the policy that runs.
const VISUAL_POLICY = resolve(
  REPO_ROOT,
  "fixtures/assets/visual-resolution-policy-v1.json"
);

/**
 * Runs the projector over the store and returns the frames it wrote, or null when
 * it could not run or did not produce a readable batch. Null means "no observation
 * this time" — the caller sends nothing rather than inventing a city.
 */
function projectFrames(logger) {
  if (!existsSync(PROJECTOR)) {
    logger.warn(
      `[universe-stream] projector missing: ${PROJECTOR}\n` +
        `[universe-stream] build it with: ${BUILD_HINT}`
    );
    return null;
  }
  try {
    execFileSync(
      PROJECTOR,
      [STORE_DIR, PROJECTION_DIR, VISUAL_CATALOG, VISUAL_POLICY],
      { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"] }
    );
  } catch (error) {
    // The store is very likely mid-write (a bin appending events, or the MCP
    // session committing). Skip this push; the next fs event retries. Never emit
    // a half-read city.
    const detail = error?.stderr?.toString().trim() || String(error);
    logger.warn(`[universe-stream] projection skipped: ${detail}`);
    return null;
  }
  try {
    const frames = JSON.parse(readFileSync(FRAMES_FILE, "utf8"));
    return Array.isArray(frames) ? frames : null;
  } catch (error) {
    logger.warn(
      `[universe-stream] unreadable frames at ${FRAMES_FILE}: ${
        error instanceof Error ? error.message : error
      }`
    );
    return null;
  }
}

export default function universeStreamPlugin() {
  return {
    name: "universe-stream",
    apply: "serve",
    configureServer(server) {
      const logger = server.config.logger;

      server.middlewares.use((req, res, next) => {
        const path = (req.url ?? "").split("?")[0];
        if (path !== ENDPOINT) return next();

        res.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache, no-transform",
          Connection: "keep-alive",
          "X-Accel-Buffering": "no"
        });
        res.write(": connected\n\n");

        let closed = false;

        const push = () => {
          if (closed) return;
          const frames = projectFrames(logger);
          if (!frames) return; // nothing observed — the client stays honestly empty
          for (const frame of frames) {
            res.write(`data: ${JSON.stringify(frame)}\n\n`);
          }
        };

        // The current city, immediately.
        push();

        // Coalesce the burst of fs events one write produces into a single push.
        let debounce = null;
        const onChange = () => {
          if (debounce) clearTimeout(debounce);
          debounce = setTimeout(push, 200);
        };
        let watcher = null;
        try {
          watcher = watch(STORE_DIR, { persistent: false }, onChange);
        } catch (error) {
          logger.warn(
            `[universe-stream] cannot watch ${STORE_DIR}: ${
              error instanceof Error ? error.message : error
            }`
          );
        }

        const heartbeat = setInterval(() => {
          if (!closed) res.write(": ping\n\n");
        }, 20_000);

        const stop = () => {
          if (closed) return;
          closed = true;
          if (debounce) clearTimeout(debounce);
          clearInterval(heartbeat);
          if (watcher) watcher.close();
        };
        req.on("close", stop);
        res.on("close", stop);
      });

      logger.info(
        `  \x1b[36m➜\x1b[0m  universe-stream: live city at \x1b[1m${ENDPOINT}\x1b[0m`
      );
      if (!existsSync(PROJECTOR)) {
        logger.warn(
          `  \x1b[33m!\x1b[0m  universe-stream: projector not built — ${BUILD_HINT}`
        );
      }
    }
  };
}
