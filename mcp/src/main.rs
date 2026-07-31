//! `mind-mcp` — the minimal two-verb MCP adapter into the Universe.
//!
//! Transport: JSON-RPC 2.0 over stdio, one message per line (the MCP stdio
//! convention). The adapter boots a real [`World`] from the environment and
//! answers `initialize`, `tools/list`, and `tools/call` for the two verbs
//! `sense` (perceive) and `act` (transform).
//!
//! It is a HEADLESS adapter: it reaches the kernel through the same Supervisor
//! the 3D world uses and never maintains a second ontology (CLAUDE.md,
//! "Headless adapters").

mod act;
mod frame;
mod jsonrpc;
mod mcp;
mod pov;
mod raster;
mod sense;
mod session;
mod world;

use std::io::{self, BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use jsonrpc::{code, Request, Response};
use serde_json::Value;
use session::SessionRegistry;
use world::World;

/// Wall-clock unix seconds, for session admission/expiry.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn main() {
    let mut world = World::from_env();
    let mut registry = SessionRegistry::default();
    if let Some(reason) = world.unmounted_reason() {
        // Diagnostics go to stderr so they never corrupt the stdout wire.
        eprintln!("mind-mcp: {reason}");
    } else {
        eprintln!("mind-mcp: Universe mounted; serving `sense` and `act` over stdio");
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("mind-mcp: stdin read error: {error}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => mcp::handle(&mut world, &mut registry, now_unix(), &request),
            Err(error) => Some(Response::failure(
                Value::Null,
                code::PARSE_ERROR,
                format!("parse error: {error}"),
            )),
        };

        if let Some(response) = response {
            if let Err(error) = write_frame(&mut out, &response) {
                eprintln!("mind-mcp: stdout write error: {error}");
                break;
            }
        }
    }
}

/// Writes one JSON-RPC response as a single newline-terminated line, then
/// flushes so the host sees it immediately.
fn write_frame(out: &mut impl Write, response: &Response) -> io::Result<()> {
    let encoded = serde_json::to_string(response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    out.write_all(encoded.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}
