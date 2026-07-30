import { createHmac } from "node:crypto";
import { describe, expect, it } from "vitest";
import {
  authenticationProof,
  clientAuthenticate,
  clientHello,
  decodeFrame,
  encodeHello,
  frameMessage,
  type ProtocolHelloWire
} from "./protocol-auth";

const HELLO: ProtocolHelloWire = {
  minimum_version: 0,
  maximum_version: 0,
  client_id: "test-client",
  resume_after: null
};

describe("authenticated transport wire", () => {
  it("encodes hello byte-exactly like serde_json (compact, field order)", () => {
    expect(new TextDecoder().decode(encodeHello(HELLO))).toBe(
      '{"minimum_version":0,"maximum_version":0,"client_id":"test-client","resume_after":null}'
    );
    // A numeric resume_after serialises as a bare number, matching the newtype.
    expect(new TextDecoder().decode(encodeHello({ ...HELLO, resume_after: 7 }))).toBe(
      '{"minimum_version":0,"maximum_version":0,"client_id":"test-client","resume_after":7}'
    );
  });

  it("computes the HMAC-SHA256 proof the server expects", async () => {
    const secret = new TextEncoder().encode("a-sufficiently-long-shared-secret-value");
    const nonce = new Uint8Array(32).map((_, index) => index); // deterministic 0..31

    // Independent golden: HMAC-SHA256 over DOMAIN ‖ nonce ‖ hello-json, computed
    // with Node's own crypto (a different implementation from Web Crypto).
    const message = Buffer.concat([
      Buffer.from("mind-universe-protocol-v0-auth"),
      Buffer.from(nonce),
      Buffer.from(encodeHello(HELLO))
    ]);
    const golden = createHmac("sha256", Buffer.from(secret)).update(message).digest();

    const proof = await authenticationProof(secret, nonce, HELLO);
    expect(proof).toHaveLength(32);
    expect(Buffer.from(proof).equals(golden)).toBe(true);
  });

  it("tags client envelopes by transport_type", () => {
    expect(clientHello(HELLO).transport_type).toBe("hello");
    const auth = clientAuthenticate(new Uint8Array([1, 2, 3]));
    expect(auth.transport_type).toBe("authenticate");
    expect(auth.proof).toEqual([1, 2, 3]);
  });

  it("frames with a big-endian length prefix and round-trips", () => {
    const framed = frameMessage({ transport_type: "hello", client_id: "x" });
    // First 4 bytes are the big-endian JSON length.
    const declared = new DataView(framed.buffer).getUint32(0, false);
    expect(declared).toBe(framed.length - 4);

    const decoded = decodeFrame(framed);
    expect(decoded).not.toBeNull();
    expect(decoded!.message).toEqual({ transport_type: "hello", client_id: "x" });
    expect(decoded!.rest).toHaveLength(0);

    // A partial buffer yields null until the full frame is present.
    expect(decodeFrame(framed.subarray(0, 3))).toBeNull();
    expect(decodeFrame(framed.subarray(0, framed.length - 1))).toBeNull();
  });
});
