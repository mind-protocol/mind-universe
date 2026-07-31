# Dev orchestrator

One command to run every local service with fixed ports, auto-restart, and file logs.

```bash
node tools/dev/dev.mjs
```

This is local developer tooling (bootstrap), not a graph runtime capability. It
only launches processes defined in [`services.json`](services.json) — the single
source of truth for what runs and on which port.

## What it does

| Requirement            | How                                                                 |
| ---------------------- | ------------------------------------------------------------------- |
| Defines & keeps ports  | `ports` map in `services.json`; a stale process on a port is killed before (re)start |
| Auto-restart on crash  | Exponential backoff (1s → 15s), reset after a 10s stable run        |
| Auto-restart on change | Per-service `watch` globs + `watchExtensions` (server watches `.rs`/`.toml`) |
| Logs to a file         | `logs/<name>.log` (appended, also teed to the console with a `[name]` prefix) |

Vite (`desktop`) keeps its own HMR, so it restarts only on crash, not on file change.

## Services

- **server** — `apps/universe-server` bound to `127.0.0.1:8787` (was a random port).
- **desktop** — `apps/mind-desktop` Vite dev server on `127.0.0.1:1420`.

## Flags

```bash
node tools/dev/dev.mjs --only server     # run a subset (comma-separated)
node tools/dev/dev.mjs --no-watch        # disable restart-on-change
node tools/dev/dev.mjs --attach          # do not reclaim ports; fail if in use
```

## Secrets

`universe-server` requires `UNIVERSE_STREAM_SECRET`. The manifest carries a
**dev-only** default; put a real one in `.env` (git-ignored) to override — `.env`
and the real environment always win over the manifest default.
