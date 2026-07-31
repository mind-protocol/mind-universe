#!/usr/bin/env node
// Local dev orchestrator for mind-universe.
//
// One command starts every service defined in tools/dev/services.json with:
//   - fixed, single-source-of-truth ports (reclaimed if a stale process holds them)
//   - auto-restart on crash (exponential backoff, reset after a stable run)
//   - auto-restart on source change (per-service watch globs; Vite keeps its own HMR)
//   - per-service log files under logs/<name>.log (also teed to the console)
//   - clean tree-kill of every child (and its grandchildren) on Ctrl-C
//
// This is developer tooling / local bootstrap code — NOT a graph runtime
// capability. It only launches processes; it never mutates the graph.
//
// Usage:
//   node tools/dev/dev.mjs                 # start everything
//   node tools/dev/dev.mjs --only server   # start a subset (comma-separated)
//   node tools/dev/dev.mjs --no-watch      # disable restart-on-change
//   node tools/dev/dev.mjs --attach        # do not reclaim ports, fail if in use

import { spawn, spawnSync } from "node:child_process";
import { createWriteStream, mkdirSync, readFileSync, existsSync, statSync, rmSync } from "node:fs";
import { watch } from "node:fs";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

// Paths that churn from builds/tooling and must never trigger a restart.
const IGNORE_SEGMENTS = ["target", "node_modules", ".git", "logs", "dist", ".fingerprint"];
const isIgnoredPath = (p) =>
  p.split(/[\\/]/).some((seg) => IGNORE_SEGMENTS.includes(seg));

const IS_WIN = process.platform === "win32";
const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, "..", "..");

// ---- CLI flags ------------------------------------------------------------
const argv = process.argv.slice(2);
const flag = (name) => argv.includes(name);
const flagValue = (name) => {
  const i = argv.indexOf(name);
  return i >= 0 && i + 1 < argv.length ? argv[i + 1] : null;
};
const ONLY = (flagValue("--only") || "")
  .split(",")
  .map((s) => s.trim())
  .filter(Boolean);
const NO_WATCH = flag("--no-watch");
const NO_RECLAIM = flag("--attach");

// ---- colors ---------------------------------------------------------------
const COLORS = [36, 32, 35, 33, 34, 31, 96, 92]; // cyan, green, magenta, ...
const c = (n, s) => (process.stdout.isTTY ? `\x1b[${n}m${s}\x1b[0m` : s);
const dim = (s) => c(90, s);

// ---- config load ----------------------------------------------------------
function loadJsonc(path) {
  // Tolerate our "$comment" keys; strip nothing else (strict JSON otherwise).
  return JSON.parse(readFileSync(path, "utf8"));
}

function loadDotEnv(path) {
  const out = {};
  if (!existsSync(path)) return out;
  for (const raw of readFileSync(path, "utf8").split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const eq = line.indexOf("=");
    if (eq < 0) continue;
    const key = line.slice(0, eq).trim();
    let val = line.slice(eq + 1).trim();
    if (
      (val.startsWith('"') && val.endsWith('"')) ||
      (val.startsWith("'") && val.endsWith("'"))
    ) {
      val = val.slice(1, -1);
    }
    out[key] = val;
  }
  return out;
}

const manifest = loadJsonc(join(__dirname, "services.json"));
const dotEnv = loadDotEnv(join(ROOT, ".env"));

// Resolution scope for ${...} tokens: real env wins over .env wins over manifest defaults.
const manifestEnv = { ...manifest.env };
delete manifestEnv.$comment;
const resolvedEnv = { ...manifestEnv, ...dotEnv, ...process.env };
const ports = manifest.ports || {};

const EXE = IS_WIN ? ".exe" : "";
function interpolate(value) {
  if (typeof value !== "string") return value;
  return value
    .replace(/\$\{exe\}/g, EXE)
    .replace(/\$\{(port|env):([^}]+)\}/g, (_, kind, key) => {
      if (kind === "port") {
        if (ports[key] == null) throw new Error(`unknown port token: ${key}`);
        return String(ports[key]);
      }
      return resolvedEnv[key] ?? "";
    });
}

// ---- port reclaim (best effort) -------------------------------------------
function pidsOnPort(port) {
  const pids = new Set();
  if (IS_WIN) {
    const res = spawnSync("netstat", ["-ano"], { encoding: "utf8" });
    if (res.status !== 0 || !res.stdout) return [];
    for (const line of res.stdout.split(/\r?\n/)) {
      // e.g.  TCP    127.0.0.1:8787   0.0.0.0:0   LISTENING   12345
      if (!/LISTENING/.test(line)) continue;
      if (!new RegExp(`[:.]${port}\\b`).test(line)) continue;
      const pid = line.trim().split(/\s+/).pop();
      if (/^\d+$/.test(pid) && pid !== "0") pids.add(pid);
    }
  } else {
    const res = spawnSync("lsof", ["-ti", `tcp:${port}`, "-sTCP:LISTEN"], {
      encoding: "utf8",
    });
    if (res.stdout) {
      for (const pid of res.stdout.split(/\s+/)) if (/^\d+$/.test(pid)) pids.add(pid);
    }
  }
  return [...pids];
}

function killPidTree(pid) {
  if (IS_WIN) {
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
  } else {
    try {
      process.kill(-pid, "SIGKILL"); // process group
    } catch {
      try {
        process.kill(pid, "SIGKILL");
      } catch {}
    }
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Kill whatever holds the port, then wait until the OS actually releases it
// before returning — closing a socket handle is not instantaneous on Windows,
// and binding too eagerly races into "address already in use" (os error 10048).
async function ensurePortFree(port, label) {
  if (NO_RECLAIM || port == null) return;
  const holders = pidsOnPort(port).filter((p) => String(p) !== String(process.pid));
  for (const pid of holders) {
    process.stdout.write(dim(`[${label}] port ${port} held by pid ${pid} — reclaiming\n`));
    killPidTree(pid);
  }
  for (let i = 0; i < 25; i++) {
    if (pidsOnPort(port).filter((p) => String(p) !== String(process.pid)).length === 0) return;
    await sleep(100);
  }
  process.stdout.write(dim(`[${label}] port ${port} still busy after reclaim; binding anyway\n`));
}

// ---- service supervisor ---------------------------------------------------
let shuttingDown = false;
const services = [];

class Service {
  constructor(def, color) {
    this.def = def;
    this.name = def.name;
    this.color = color;
    this.port = def.port != null ? Number(interpolate(String(def.port))) : null;
    this.cwd = resolve(ROOT, def.cwd || ".");
    this.restarts = 0;
    this.backoff = 1000;
    this.child = null;
    this.stableTimer = null;
    this.restartTimer = null;
    this.watchTimer = null;
    this.watchers = [];
    this.log = null;
  }

  tag(msg) {
    return `${c(this.color, `[${this.name}]`)} ${msg}`;
  }

  openLog() {
    const logsDir = resolve(ROOT, manifest.logsDir || "logs");
    mkdirSync(logsDir, { recursive: true });
    this.logPath = join(logsDir, `${this.name}.log`);
    this.log = createWriteStream(this.logPath, { flags: "a" });
    this.log.write(
      `\n==== ${this.name} session start ${new Date().toISOString()} ====\n`
    );
  }

  writeLine(stream, chunk) {
    const text = chunk.toString();
    if (this.log) this.log.write(text);
    for (const line of text.split(/\r?\n/)) {
      if (!line.length) continue;
      process[stream].write(this.tag(line) + "\n");
      if (!this.ready && this.def.readyLog && line.includes(this.def.readyLog)) {
        this.ready = true;
        const secs = ((Date.now() - (this.startedAt || Date.now())) / 1000).toFixed(1);
        process.stdout.write(this.tag(c(32, `ready (${secs}s)`)) + "\n");
      }
    }
  }

  childEnv() {
    const env = { ...process.env };
    // Layer manifest defaults + .env so services inherit resolved secrets/ports.
    for (const [k, v] of Object.entries(resolvedEnv)) if (env[k] == null) env[k] = v;
    for (const [k, v] of Object.entries(this.def.env || {})) env[k] = interpolate(v);
    return env;
  }

  // Spawn respecting per-service `shell`. Windows shell mode passes ONE quoted
  // command string (no args array) to avoid DEP0190; POSIX and direct-exe mode
  // spawn the executable itself so exit/kill tracking is exact.
  spawnProcess(command, args, useShell, env) {
    const opts = {
      cwd: this.cwd,
      env,
      detached: !IS_WIN,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    };
    if (useShell) {
      if (IS_WIN) {
        const quote = (t) => (/[\s"&|<>^()]/.test(t) ? `"${t.replace(/"/g, '\\"')}"` : t);
        opts.shell = true;
        return spawn([command, ...args].map(quote).join(" "), [], opts);
      }
      opts.shell = true;
    }
    return spawn(command, args, opts);
  }

  // Run the optional `build` step to completion; resolves with its exit code.
  runBuild(env) {
    return new Promise((done) => {
      const b = this.def.build;
      if (!b) return done(0);
      const cmd = interpolate(b.command);
      const args = (b.args || []).map(interpolate);
      process.stdout.write(this.tag(dim(`build ${cmd} ${args.join(" ")}`)) + "\n");
      const proc = this.spawnProcess(cmd, args, !!b.shell, env);
      this.buildChild = proc;
      proc.stdout.on("data", (d) => this.writeLine("stdout", d));
      proc.stderr.on("data", (d) => this.writeLine("stderr", d));
      proc.on("exit", (code) => {
        this.buildChild = null;
        done(code ?? 0);
      });
      proc.on("error", (e) => {
        this.buildChild = null;
        this.writeLine("stderr", `build spawn error: ${e.message}\n`);
        done(1);
      });
    });
  }

  async start() {
    if (shuttingDown) return;
    const env = this.childEnv();

    const buildCode = await this.runBuild(env);
    if (shuttingDown) return;
    if (buildCode !== 0) {
      process.stdout.write(this.tag(c(31, `build failed (code ${buildCode})`)) + "\n");
      return this.scheduleCrashRestart();
    }

    await ensurePortFree(this.port, this.name);
    if (shuttingDown) return;

    const command = interpolate(this.def.command);
    const args = (this.def.args || []).map(interpolate);

    process.stdout.write(
      this.tag(
        dim(
          `start ${command} ${args.join(" ")}` +
            (this.port ? ` (port ${this.port})` : "")
        )
      ) + "\n"
    );

    // Timestamp the (re)start so the watcher can ignore any change whose mtime
    // predates this run (spurious/duplicate/read events) — only a genuine edit
    // made after launch restarts the process.
    this.startedAt = Date.now();
    this.ready = false;
    this.child = this.spawnProcess(command, args, !!this.def.shell, env);

    this.child.stdout.on("data", (d) => this.writeLine("stdout", d));
    this.child.stderr.on("data", (d) => this.writeLine("stderr", d));
    this.child.on("error", (e) => this.writeLine("stderr", `spawn error: ${e.message}\n`));

    // Consider the process "stable" (worth resetting backoff) after 10s alive.
    this.stableTimer = setTimeout(() => {
      this.backoff = 1000;
      this.restarts = 0;
    }, 10_000);

    this.child.on("exit", (code, signal) => {
      clearTimeout(this.stableTimer);
      this.child = null;
      if (shuttingDown || this.stopping) return;
      const why = signal ? `signal ${signal}` : `code ${code}`;
      process.stdout.write(this.tag(c(31, `exited (${why})`)) + "\n");
      this.scheduleCrashRestart();
    });
  }

  scheduleCrashRestart() {
    if (shuttingDown) return;
    this.restarts += 1;
    const delay = this.backoff;
    this.backoff = Math.min(this.backoff * 2, 15_000);
    process.stdout.write(
      this.tag(dim(`restarting in ${Math.round(delay / 1000)}s (#${this.restarts})`)) + "\n"
    );
    this.restartTimer = setTimeout(() => this.start(), delay);
  }

  restart(reason) {
    if (shuttingDown) return;
    clearTimeout(this.restartTimer); // cancel any pending crash-restart
    process.stdout.write(this.tag(c(33, `restart: ${reason}`)) + "\n");
    this.backoff = 1000;

    // Abort an in-flight build so the change is picked up by a fresh build.
    if (this.buildChild) {
      killPidTree(this.buildChild.pid);
      this.buildChild = null;
    }
    if (!this.child) {
      // Not currently running (building, or between backoffs): start fresh.
      this.start();
      return;
    }
    this.stopping = true;
    const pid = this.child.pid;
    this.child.once("exit", () => {
      this.stopping = false;
      this.start();
    });
    killPidTree(pid);
  }

  setupWatch() {
    if (NO_WATCH || !this.def.restartOnChange) return;
    const exts = this.def.watchExtensions || null;
    const fire = (file) => {
      this.pendingFile = file;
      clearTimeout(this.watchTimer);
      this.watchTimer = setTimeout(
        () => this.restart(`changed ${this.pendingFile}`),
        400
      );
    };
    // A change counts only if the file's mtime is newer than the current run —
    // this drops build churn, editor read-backs, and duplicate FS events.
    const changedSinceStart = (full) => {
      try {
        return statSync(full).mtimeMs > (this.startedAt || 0) - 50;
      } catch {
        return false; // deleted/renamed mid-build: ignore
      }
    };
    for (const rel of this.def.watch || []) {
      const target = resolve(ROOT, rel);
      if (!existsSync(target)) continue;
      const base = statSync(target).isDirectory() ? target : dirname(target);
      try {
        const w = watch(target, { recursive: true }, (_evt, file) => {
          if (!file) return;
          const full = join(base, file);
          if (isIgnoredPath(full)) return;
          if (exts && !exts.some((e) => file.endsWith(e))) return;
          if (!changedSinceStart(full)) return;
          fire(file);
        });
        this.watchers.push(w);
      } catch (e) {
        process.stdout.write(
          this.tag(dim(`watch unavailable for ${rel}: ${e.message}`)) + "\n"
        );
      }
    }
    if (this.watchers.length) {
      process.stdout.write(
        this.tag(dim(`watching ${this.def.watch.join(", ")} (${(exts || ["*"]).join(",")})`)) +
          "\n"
      );
    }
  }

  stop() {
    this.stopping = true;
    clearTimeout(this.restartTimer);
    clearTimeout(this.stableTimer);
    clearTimeout(this.watchTimer);
    for (const w of this.watchers) {
      try {
        w.close();
      } catch {}
    }
    if (this.buildChild) killPidTree(this.buildChild.pid);
    if (this.child) killPidTree(this.child.pid);
    if (this.log) this.log.end();
  }
}

// ---- single-instance lock -------------------------------------------------
// A second orchestrator would fight the first over every port (each one's
// port-reclaim kills the other's services). Refuse to start if one is running.
const LOCK_PATH = join(resolve(ROOT, manifest.logsDir || "logs"), ".dev.lock");
function pidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e.code === "EPERM"; // exists but not ours
  }
}
function acquireLock() {
  mkdirSync(dirname(LOCK_PATH), { recursive: true });
  if (existsSync(LOCK_PATH)) {
    const prev = parseInt(readFileSync(LOCK_PATH, "utf8").trim(), 10);
    if (prev && prev !== process.pid && pidAlive(prev)) {
      console.error(
        c(31, `another orchestrator is already running (pid ${prev}).`) +
          `\nStop it first, or remove ${LOCK_PATH} if it is stale.`
      );
      process.exit(1);
    }
  }
  createWriteStream(LOCK_PATH).end(String(process.pid));
}
function releaseLock() {
  try {
    if (existsSync(LOCK_PATH)) {
      const owner = parseInt(readFileSync(LOCK_PATH, "utf8").trim(), 10);
      if (owner === process.pid) rmSync(LOCK_PATH, { force: true });
    }
  } catch {}
}

// ---- boot -----------------------------------------------------------------
function main() {
  acquireLock();
  let defs = manifest.services || [];
  if (ONLY.length) defs = defs.filter((d) => ONLY.includes(d.name));
  if (!defs.length) {
    console.error("no services selected (check --only / services.json)");
    process.exit(1);
  }

  process.stdout.write(
    c(1, `mind-universe dev orchestrator`) +
      dim(` — ${defs.map((d) => d.name).join(", ")}\n`)
  );
  if (!resolvedEnv.UNIVERSE_STREAM_SECRET) {
    process.stdout.write(dim("warning: UNIVERSE_STREAM_SECRET unset\n"));
  } else if (!dotEnv.UNIVERSE_STREAM_SECRET && !process.env.UNIVERSE_STREAM_SECRET) {
    process.stdout.write(
      dim("note: using dev-only UNIVERSE_STREAM_SECRET from manifest (set one in .env to override)\n")
    );
  }

  defs.forEach((def, i) => {
    const svc = new Service(def, COLORS[i % COLORS.length]);
    services.push(svc);
    svc.openLog();
    svc.start();
    svc.setupWatch();
  });

  process.stdout.write(dim(`logs -> ${resolve(ROOT, manifest.logsDir || "logs")}\n`));
}

let stopped = false;
function shutdown() {
  if (stopped) return;
  stopped = true;
  shuttingDown = true;
  process.stdout.write("\n" + dim("shutting down...\n"));
  for (const s of services) s.stop();
  releaseLock();
  setTimeout(() => process.exit(0), 600);
}

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
if (IS_WIN) {
  // Ctrl-C on Windows sometimes arrives only via readline.
  process.on("SIGBREAK", shutdown);
}

main();
