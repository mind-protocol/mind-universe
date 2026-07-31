// Minimal, dependency-free stdio -> Streamable-HTTP bridge for mind-mcp.
//
// Why this exists: supergateway's streamable-HTTP transport throws
// "No connection established for request ID" and dies when a real client
// (ChatGPT via ngrok) drives the stream; mcp-proxy has an incompatible mcp-SDK
// pin. Both are unusable here. This bridge does exactly what mind-mcp needs —
// request/response JSON-RPC — over MCP Streamable HTTP, and never throws when a
// connection closes early (the precise failure that killed supergateway).
//
// Contract: one long-lived mind-mcp child (stdio). POST <path> carries one
// JSON-RPC message; we forward it and reply with the matching response as
// application/json. Notifications get 202. GET opens an (empty) SSE stream.
// A single global session id is minted at initialize and echoed back.
//
// Env: BRIDGE_BIN (required, path to mind-mcp.exe), BRIDGE_PORT (default 8000),
//      BRIDGE_PATH (default /mcp). UNIVERSE_STORE/UNIVERSE_GENESIS pass through.

import http from 'node:http';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';

const PORT = parseInt(process.env.BRIDGE_PORT || '8000', 10);
const MCP_PATH = process.env.BRIDGE_PATH || '/mcp';
const BIN = process.env.BRIDGE_BIN;
if (!BIN) { console.error('[bridge] BRIDGE_BIN is required'); process.exit(2); }

// --- the single stdio child -------------------------------------------------
const child = spawn(BIN, [], { stdio: ['pipe', 'pipe', 'inherit'], env: process.env });
child.on('exit', (code) => { console.error(`[bridge] mind-mcp exited (${code}); shutting down`); process.exit(1); });

const pending = new Map(); // json-rpc id -> http res
let sessionId = null;      // minted at first initialize

// Read newline-delimited JSON-RPC frames from the child's stdout.
let buf = '';
child.stdout.on('data', (chunk) => {
  buf += chunk.toString('utf8');
  let nl;
  while ((nl = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, nl).trim();
    buf = buf.slice(nl + 1);
    if (!line) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; } // ignore non-JSON diagnostics
    if (msg.id === undefined || msg.id === null) continue; // server notification -> drop
    const res = pending.get(msg.id);
    if (!res) continue;                                    // unmatched / client already gone
    pending.delete(msg.id);
    if (res.writableEnded || res.destroyed) continue;      // NEVER throw on a closed conn
    try {
      res.writeHead(200, { 'Content-Type': 'application/json', 'Mcp-Session-Id': sessionId || '' });
      res.end(line);
    } catch { /* connection vanished between checks — swallow */ }
  }
});

// --- HTTP front -------------------------------------------------------------
const server = http.createServer((req, res) => {
  const path = (req.url || '').split('?')[0];

  if (req.method === 'GET' && path === '/healthz') {
    res.writeHead(200, { 'Content-Type': 'text/plain' }); res.end('ok'); return;
  }
  if (path !== MCP_PATH) { res.writeHead(404); res.end('not found'); return; }

  // Streamable-HTTP GET: open a notification stream. We have no server-initiated
  // messages, so hold an empty keep-alive SSE until the client disconnects.
  if (req.method === 'GET') {
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', 'Connection': 'keep-alive', 'Mcp-Session-Id': sessionId || '' });
    const ping = setInterval(() => { try { res.write(': ping\n\n'); } catch {} }, 15000);
    res.on('close', () => { clearInterval(ping); try { res.end(); } catch {} });
    return;
  }

  // Session teardown.
  if (req.method === 'DELETE') { res.writeHead(200); res.end(); return; }

  if (req.method !== 'POST') { res.writeHead(405); res.end(); return; }

  let body = '';
  req.on('data', (d) => { body += d; if (body.length > 8 * 1024 * 1024) req.destroy(); });
  req.on('end', () => {
    let msg;
    try { msg = JSON.parse(body); } catch {
      res.writeHead(400, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'parse error' } }));
      return;
    }
    if (msg && msg.method === 'initialize' && !sessionId) sessionId = randomUUID();

    const isNotification = msg == null || msg.id === undefined || msg.id === null;
    if (isNotification) {
      try { child.stdin.write(JSON.stringify(msg) + '\n'); } catch {}
      res.writeHead(202, { 'Mcp-Session-Id': sessionId || '' }); res.end();
      return;
    }

    pending.set(msg.id, res);
    // Clean up only on a real client disconnect (res 'close'). NB: req 'close'
    // fires as soon as the request body is consumed — using it here would drop
    // the pending entry before the child ever answered.
    res.on('close', () => { if (pending.get(msg.id) === res) pending.delete(msg.id); });
    try { child.stdin.write(JSON.stringify(msg) + '\n'); }
    catch {
      pending.delete(msg.id);
      if (!res.writableEnded) { res.writeHead(500); res.end('child write failed'); }
    }
  });
});

server.listen(PORT, '127.0.0.1', () => {
  console.error(`[bridge] mind-mcp Streamable-HTTP on http://127.0.0.1:${PORT}${MCP_PATH}  (health: /healthz)`);
});
