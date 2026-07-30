// Byte-exact client half of the authenticated transport wire (G3 item 1),
// mirroring `crates/universe-protocol/src/transport.rs`. A webview cannot open a
// raw TCP socket, so in production the app↔server bridge is NATIVE (a Tauri Rust
// command using `ProtocolClient`). These primitives exist so that native bridge —
// or any exact reimplementation — has a verified, drift-checked reference:
//
// - framing: each message is a 4-byte big-endian length prefix + compact JSON;
// - envelopes: internally tagged by `transport_type`
//   (hello | authenticate | frame ; server: challenge | ready | frame | ...);
// - auth proof: HMAC-SHA256(secret, DOMAIN ‖ nonce ‖ serde_json(hello)).

export const AUTH_DOMAIN = "mind-universe-protocol-v0-auth";

export interface ProtocolHelloWire {
  readonly minimum_version: number;
  readonly maximum_version: number;
  readonly client_id: string;
  readonly resume_after: number | null;
}

/**
 * Encodes a ProtocolHello to the exact bytes `serde_json::to_vec` produces:
 * compact, fields in declaration order, `resume_after` as a number or `null`.
 * The auth proof signs THESE bytes, so any drift here breaks authentication.
 */
export function encodeHello(hello: ProtocolHelloWire): Uint8Array {
  const ordered = {
    minimum_version: hello.minimum_version,
    maximum_version: hello.maximum_version,
    client_id: hello.client_id,
    resume_after: hello.resume_after ?? null
  };
  return new TextEncoder().encode(JSON.stringify(ordered));
}

function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

/**
 * The authentication proof the server expects: HMAC-SHA256 over
 * DOMAIN ‖ challenge-nonce ‖ hello-json, using the shared secret as the key.
 */
export async function authenticationProof(
  secret: Uint8Array,
  nonce: Uint8Array,
  hello: ProtocolHelloWire
): Promise<Uint8Array> {
  const message = concatBytes(
    new TextEncoder().encode(AUTH_DOMAIN),
    nonce,
    encodeHello(hello)
  );
  const key = await crypto.subtle.importKey(
    "raw",
    secret as unknown as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  const signature = await crypto.subtle.sign(
    "HMAC",
    key,
    message as unknown as BufferSource
  );
  return new Uint8Array(signature);
}

// --- transport envelopes (internally tagged by `transport_type`) --------------

export function clientHello(hello: ProtocolHelloWire): Record<string, unknown> {
  return { transport_type: "hello", ...hello, resume_after: hello.resume_after ?? null };
}

export function clientAuthenticate(proof: Uint8Array): Record<string, unknown> {
  return { transport_type: "authenticate", proof: Array.from(proof) };
}

export function clientFrame(frame: Record<string, unknown>): Record<string, unknown> {
  return { transport_type: "frame", ...frame };
}

// --- length-prefixed framing --------------------------------------------------

/** Encodes one message as a 4-byte big-endian length prefix + compact JSON. */
export function frameMessage(message: unknown): Uint8Array {
  const json = new TextEncoder().encode(JSON.stringify(message));
  const framed = new Uint8Array(4 + json.length);
  new DataView(framed.buffer).setUint32(0, json.length, false); // big-endian
  framed.set(json, 4);
  return framed;
}

export interface DecodedFrame {
  readonly message: unknown;
  readonly rest: Uint8Array;
}

/**
 * Decodes one length-prefixed message, returning it plus the remaining bytes.
 * Returns null when fewer than a full frame is buffered (caller waits for more).
 */
export function decodeFrame(buffer: Uint8Array): DecodedFrame | null {
  if (buffer.length < 4) return null;
  const length = new DataView(buffer.buffer, buffer.byteOffset, 4).getUint32(0, false);
  if (buffer.length < 4 + length) return null;
  const json = new TextDecoder().decode(buffer.subarray(4, 4 + length));
  return { message: JSON.parse(json), rest: buffer.subarray(4 + length) };
}
