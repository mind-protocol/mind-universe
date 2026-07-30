import { beforeEach, describe, expect, it } from "vitest";
import citizenFrames from "../../../fixtures/desktop-world-snapshot/citizen/world-snapshot-frames.json";
import { UniverseTransport, type Socket, type TransportConfig } from "./transport-client";

class FakeSocket implements Socket {
  readonly sent: unknown[] = [];
  onOpen?: () => void;
  onMessage?: (data: string) => void;
  onClose?: () => void;
  closed = false;

  send(data: string): void {
    this.sent.push(JSON.parse(data));
  }
  close(): void {
    this.closed = true;
    this.onClose?.();
  }
  open(): void {
    this.onOpen?.();
  }
  deliver(message: unknown): void {
    this.onMessage?.(JSON.stringify(message));
  }
  sentOfKind(kind: string): Record<string, unknown>[] {
    return (this.sent as Record<string, unknown>[]).filter((m) => m.kind === kind);
  }
  clientPayloads(messageType: string): Record<string, unknown>[] {
    return this.sentOfKind("client_frame")
      .map((m) => m.payload as Record<string, unknown>)
      .filter((p) => p.message_type === messageType);
  }
}

const WELCOME = {
  kind: "welcome",
  selected_version: 0,
  stream_id: "stream-1",
  earliest_available: 0,
  latest_published: 5,
  resynchronization_required: false
};

let now = 1_000;
const config = (): TransportConfig => ({
  clientId: "desktop-test",
  minVersion: 0,
  maxVersion: 0,
  resync: { origin: "0000000000000000000000000000b001", maxEntities: 100, maxRelations: 100, timeoutTicks: 32 },
  ackEveryFrames: 2,
  nowMs: () => now
});

describe("live transport connector", () => {
  let socket: FakeSocket;
  let transport: UniverseTransport;

  beforeEach(() => {
    now = 1_000;
    socket = new FakeSocket();
    transport = new UniverseTransport(config());
    transport.attach(socket);
  });

  it("sends a hello on open and enters streaming on welcome", () => {
    socket.open();
    expect(transport.getPhase()).toBe("handshaking");
    const hellos = socket.sentOfKind("hello");
    expect(hellos).toHaveLength(1);
    expect(hellos[0].client_id).toBe("desktop-test");

    socket.deliver(WELCOME);
    expect(transport.getPhase()).toBe("streaming");
  });

  it("applies server frames and acknowledges periodically", () => {
    socket.open();
    socket.deliver(WELCOME);
    for (const frame of citizenFrames) socket.deliver(frame);

    expect(transport.getStream().health).toBe("synchronized");
    expect(transport.getStream().view.entities.size).toBe(3);
    // ackEveryFrames = 2 over 5 applied frames → acknowledgements at 2 and 4.
    expect(socket.clientPayloads("acknowledge").length).toBeGreaterThanOrEqual(2);
  });

  it("requests resync on a sequence gap", () => {
    socket.open();
    socket.deliver(WELCOME);
    socket.deliver(citizenFrames[0]); // snapshot, seq 1
    socket.deliver(citizenFrames[2]); // seq 3 — gap

    expect(transport.getStream().health).toBe("degraded");
    const resyncs = socket.clientPayloads("resynchronize");
    expect(resyncs.length).toBeGreaterThanOrEqual(1);
    expect(resyncs[0].origin).toBe("0000000000000000000000000000b001");
  });

  it("resyncs immediately when the welcome demands it", () => {
    socket.open();
    socket.deliver({ ...WELCOME, resynchronization_required: true });
    expect(socket.clientPayloads("resynchronize").length).toBeGreaterThanOrEqual(1);
  });

  it("marks the stream disconnected on close", () => {
    socket.open();
    socket.deliver(WELCOME);
    socket.deliver(citizenFrames[0]);
    socket.close();
    expect(transport.getPhase()).toBe("closed");
    expect(transport.getStream().health).toBe("disconnected");
  });
});
