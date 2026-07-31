// Vite dev plugin: stream the LIVE ontology-registry store to the browser over SSE.
//
// This is the dev-time transport for "le store, pas un snapshot, en temps réel":
//   GET /universe-stream  ->  text/event-stream
//     - on connect: materialize the store (snapshot + events replay) and send the
//       full frame batch (snapshot + entity/relation frames);
//     - on any change under the store directory: re-materialize and push a fresh
//       batch, so the city updates live as bins append events / rewrite the store.
//
// The frames are the SAME wire shape the native universe-server / Tauri bridge
// emits (see stream-frames.mjs), so the browser consumes them through the exact
// production reducer (protocol-adapter -> stream-client). Graduating to the native
// path is a transport swap, not a reformat. `apply: "serve"` keeps this inert in
// production builds (where the app falls back to the baked fixture).

import { watch } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { materializeCity } from "./materialize-city.mjs";
import { cityToFrames } from "./stream-frames.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, "../../..");
const STORE_DIR = resolve(REPO_ROOT, "artifacts/ontology-registry/current/store");
const ENDPOINT = "/universe-stream";

export default function universeStreamPlugin() {
  return {
    name: "universe-stream",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const path = (req.url ?? "").split("?")[0];
        if (path !== ENDPOINT) return next();

        res.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache, no-transform",
          Connection: "keep-alive",
          // Vite sits behind no proxy in dev, but this is the courteous default.
          "X-Accel-Buffering": "no"
        });
        res.write(": connected\n\n");

        let seq = 1;
        let closed = false;

        const push = () => {
          if (closed) return;
          let city;
          try {
            city = materializeCity(STORE_DIR);
          } catch (error) {
            // The store may be mid-write (a bin appending events); skip this tick
            // rather than emit a half-read city. The next fs event re-tries.
            server.config.logger.warn(
              `[universe-stream] skipped a push: ${error instanceof Error ? error.message : error}`
            );
            return;
          }
          const { frames, nextSeq } = cityToFrames(city, seq);
          seq = nextSeq;
          for (const frame of frames) {
            res.write(`data: ${JSON.stringify(frame)}\n\n`);
          }
        };

        // Initial batch: the current store, immediately.
        push();

        // Coalesce bursts of fs events (a single save fires several) into one push.
        let debounce = null;
        const onChange = () => {
          if (debounce) clearTimeout(debounce);
          debounce = setTimeout(push, 200);
        };
        let watcher = null;
        try {
          watcher = watch(STORE_DIR, { persistent: false }, onChange);
        } catch (error) {
          server.config.logger.warn(
            `[universe-stream] cannot watch ${STORE_DIR}: ${error instanceof Error ? error.message : error}`
          );
        }

        // Keep the connection alive through idle periods.
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

      server.config.logger.info(
        `  \x1b[36m➜\x1b[0m  universe-stream: live store at \x1b[1m${ENDPOINT}\x1b[0m`
      );
    }
  };
}
