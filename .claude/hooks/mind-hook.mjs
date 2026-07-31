#!/usr/bin/env node
// mind-hook.mjs — put THIS Claude session's body into the Universe.
//
// Two Claude Code hooks share this one script (the verb is argv[2]):
//
//   SessionStart      -> `arrive` : the city Gate mints a traceable, durable
//                                    ActorSession for this session. Sponsored, so
//                                    it persists as an inhabitant (visible on the
//                                    desktop); re-running the same id re-admits.
//   UserPromptSubmit  -> `sense`  : INJECT the submitted prompt as a situated,
//                                    attributed perturbation (an `act`), then
//                                    return the world AFTER it — the same
//                                    first-person frame `sense` builds, now of
//                                    the post-commit revision.
//
// It is NOT a second ontology, and it does NOT re-derive readable prose: it
// drives the real `mind-mcp` binary one-shot over stdio (the exact adapter the
// live MCP server uses) and RELAYS the Observation's own `text` frame verbatim.
// Every node label and action verb is owned by the Rust renderer; this hook
// holds NO label/verb dictionaries. An unmounted store is reported honestly (the
// Rust `text` frame says so itself), never fabricated as "empty".
//
// HONEST LIMIT: the UserPromptSubmit `act` commits a WRITTEN perturbation — an
// inert `construct`/proposal node with provenance and a construction moment,
// committed atomically and read back independently. It is NOT a live firing of a
// construct's mechanism; the world-after reflects a committed write, not a run.
//
// Everything resolves repo-relative (with env overrides), so the script is not
// pinned to one machine. It always exits 0: a perception hook must never block
// the session, so any failure degrades to "no context injected".

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url)); // <repo>/.claude/hooks
const REPO = process.env.CLAUDE_PROJECT_DIR
  ? resolve(process.env.CLAUDE_PROJECT_DIR)
  : resolve(HERE, "..", ".."); // .claude/hooks -> repo root

const BIN =
  process.env.MIND_MCP_BIN ||
  join(
    REPO,
    "mcp",
    "target",
    "release",
    process.platform === "win32" ? "mind-mcp.exe" : "mind-mcp",
  );
const STORE =
  process.env.UNIVERSE_STORE ||
  join(REPO, "artifacts", "ontology-registry", "current", "store");
const GENESIS =
  process.env.UNIVERSE_GENESIS ||
  join(REPO, "fixtures", "genesis", "minimal-genesis.json");

const TIMEOUT_MS = Number(process.env.MIND_HOOK_TIMEOUT_MS || 15000);
const ORIGIN = "Claude Code/Anthropic";
// Default vantage for the situated perturbation and its readback: the Lumina
// Prime orientation beacon ("Balise Zéro") — a civic origin with a rich
// neighbourhood (BFS reach ~74), vs the severed `protocol` root the world falls
// back to otherwise (reach 1). It is the 0xB000 deterministic key of
// inject_orientation_beacon, stable across reseeds. If it fails to resolve, the
// adapter degrades to its own fallback vantage (no error).
const SENSE_ORIGIN =
  process.env.MIND_SENSE_ORIGIN || "0000000000000000000000000000b000";
// A sponsor unlocks the Propose (write) capability. It makes `arrive` materialise
// a DURABLE inhabitant, and it lets the UserPromptSubmit `act` actually commit
// its perturbation. The child's session registry persists across the lines of a
// single spawn, so an `arrive` that sponsors a session id is in force for an
// `act` on that same id in the same process — fail-closed authority is honoured
// (an unsponsored walk-in still cannot write), never bypassed.
const SPONSOR = process.env.MIND_SPONSOR || "nlr_ai";

/** Read all of stdin (the hook payload JSON). */
function readStdin() {
  return new Promise((res) => {
    let buf = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (d) => (buf += d));
    process.stdin.on("end", () => res(buf));
    process.stdin.on("error", () => res(buf));
  });
}

/** Emit a hook result and leave. Always exit 0 — never block the session. */
function emitAndExit(obj) {
  if (obj) process.stdout.write(JSON.stringify(obj));
  process.exit(0);
}

/**
 * Drive mind-mcp one-shot: feed each call in `calls` as one JSON-RPC line
 * (ids 1..n). The child processes the lines in order and its session registry
 * persists across them within the single spawn, so a sponsoring `arrive` is in
 * force for a later `act` on the same session id. Resolves `{ result }` with the
 * reply of the LAST request, or `{ error }` on any failure.
 */
function callMcp(calls) {
  return new Promise((res) => {
    if (!existsSync(BIN)) {
      return res({ error: `mind-mcp binary not found at ${BIN}` });
    }
    const child = spawn(BIN, [], {
      env: { ...process.env, UNIVERSE_STORE: STORE, UNIVERSE_GENESIS: GENESIS },
      stdio: ["pipe", "pipe", "pipe"],
    });
    const wantId = calls.length; // the reply we relay is the last request's
    let out = "";
    const timer = setTimeout(() => {
      try { child.kill(); } catch {}
      res({ error: `mind-mcp timed out after ${TIMEOUT_MS}ms` });
    }, TIMEOUT_MS);

    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (d) => (out += d));
    child.on("error", (e) => {
      clearTimeout(timer);
      res({ error: `spawn failed: ${e.message}` });
    });
    child.on("close", () => {
      clearTimeout(timer);
      const line = out
        .split(/\r?\n/)
        .filter(Boolean)
        .map((l) => { try { return JSON.parse(l); } catch { return null; } })
        .find((m) => m && m.id === wantId);
      if (!line) return res({ error: "no JSON-RPC reply from mind-mcp" });
      if (line.error) return res({ error: line.error.message || "mcp error" });
      res({ result: line.result });
    });

    let body = "";
    for (let i = 0; i < calls.length; i++) {
      body += JSON.stringify({
        jsonrpc: "2.0",
        id: i + 1,
        method: "tools/call",
        params: { name: calls[i].name, arguments: calls[i].args },
      }) + "\n";
    }
    child.stdin.write(body);
    child.stdin.end();
  });
}

/**
 * The city Gate mints/refreshes this session's body. `arrive` returns an
 * admission receipt, NOT an Observation, so it carries no readable `text` frame.
 * This is therefore the one place the hook composes prose — and it does so from
 * SessionPassport fields ALONE (session_id, origin, status, density,
 * capabilities). No node names, no verb translations, no dictionaries.
 */
function summariseArrive(sc) {
  const p = sc.passport || {};
  const caps = Array.isArray(p.capabilities) && p.capabilities.length
    ? p.capabilities.join(", ")
    : "aucune";
  return (
    `Tu es incarné dans l'Univers (mind). La session « ${p.session_id} » ` +
    `(origine ${p.origin}) a été admise.\n` +
    `Statut : ${p.status} · densité d'incarnation ${p.density} · capacités : ${caps}.\n` +
    `Être présent est gratuit ; agir demande une portée. Perçois avant d'agir — ` +
    `lis les reçus, ne devine pas.`
  );
}

async function main() {
  const verb = process.argv[2]; // "arrive" | "sense"
  const raw = await readStdin();
  let hook = {};
  try { hook = JSON.parse(raw || "{}"); } catch {}

  const hookEventName =
    hook.hook_event_name || (verb === "arrive" ? "SessionStart" : "UserPromptSubmit");
  const sessionId = `claude:${hook.session_id || "session"}`;

  let call;
  let relayFrame = false;
  if (verb === "arrive") {
    // Sponsored arrival materialises a DURABLE actor node — the session stops
    // being ephemeral and persists as an L1 inhabitant.
    call = await callMcp([
      { name: "arrive", args: { session_id: sessionId, origin: ORIGIN, sponsor: SPONSOR } },
    ]);
  } else {
    // UserPromptSubmit: INJECT then PERCEIVE. Sponsor-arrive first (same id, same
    // spawn) so the session holds Propose and the `act` genuinely commits; then
    // `act` commits the prompt as a situated perturbation and returns the actor's
    // POV of the world AFTER. `where` sets both the perturbation's situation and
    // the readback vantage to the civic beacon; `intent` is the submitted prompt,
    // so the perturbation is attributed to this session and to what it asked.
    call = await callMcp([
      { name: "arrive", args: { session_id: sessionId, origin: ORIGIN, sponsor: SPONSOR } },
      {
        name: "act",
        args: { actor_id: sessionId, where: SENSE_ORIGIN, intent: hook.prompt || "" },
      },
    ]);
    relayFrame = true;
  }

  if (call.error) {
    // Never block the session; surface the reason only via stderr (--debug).
    process.stderr.write(`mind-hook(${verb}): ${call.error}\n`);
    return emitAndExit({ suppressOutput: true });
  }

  const sc = call.result && call.result.structuredContent;
  if (!sc) return emitAndExit({ suppressOutput: true });

  // The perceive path relays the Rust-owned readable frame (Observation.text)
  // VERBATIM — including the honest "no Universe is mounted" frame the adapter
  // itself emits when the store is absent. The arrive path composes passport
  // prose (arrive has no `text` frame).
  const additionalContext = relayFrame
    ? (typeof sc.text === "string" && sc.text.length ? sc.text : null)
    : summariseArrive(sc);

  if (!additionalContext) return emitAndExit({ suppressOutput: true });
  emitAndExit({ hookSpecificOutput: { hookEventName, additionalContext } });
}

main().catch((e) => {
  process.stderr.write(`mind-hook: ${e && e.message}\n`);
  emitAndExit({ suppressOutput: true });
});
