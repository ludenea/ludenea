import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ControlClient, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, postMessageTransport, tauriTransport, webSocketTransport, type RenderFramesEvent } from "../src";

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    id = 7;
    onmessage: (batch: string) => void = () => {};
  },
  invoke: vi.fn(async () => {}),
}));

/** Stands in for a browser WebSocket: the test drives its events. */
class FakeSocket {
  static last: FakeSocket;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;

  constructor(readonly url: string) {
    FakeSocket.last = this;
  }

  send(): void {}
  close(): void {}
}

/** Stands in for a worker: records `terminate` and lets the test fire its events. */
class FakePort extends EventTarget {
  terminated = 0;
  postMessage(): void {}
  terminate(): void {
    this.terminated++;
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.mocked(invoke).mockClear();
});

describe("transports", () => {
  it("websocket_that_cannot_connect_rejects_with_the_close_detail", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const connecting = webSocketTransport("ws://editor.test/ws/project");
    FakeSocket.last.onerror?.();
    FakeSocket.last.onclose?.({ code: 1006, reason: "" });
    await expect(connecting).rejects.toThrow("cannot connect to ws://editor.test/ws/project: closed (1006)");
  });

  it("websocket_close_after_open_reaches_listeners", async () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const connecting = webSocketTransport("ws://editor.test/ws/project");
    FakeSocket.last.onopen?.();
    const transport = await connecting;
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    FakeSocket.last.onerror?.();
    FakeSocket.last.onclose?.({ code: 1011, reason: "server error" });
    expect(closes).toEqual(["server error"]);
  });

  it("post_message_error_closes_the_transport_once", () => {
    const port = new FakePort();
    const transport = postMessageTransport(port as unknown as Worker);
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    port.dispatchEvent(Object.assign(new Event("error"), { message: "boom" }));
    port.dispatchEvent(new Event("messageerror"));
    transport.close();
    expect(closes).toEqual(["the worker failed: boom"]);
    expect(port.terminated).toBe(1);
  });

  it("post_message_messageerror_closes_the_transport", () => {
    const port = new FakePort();
    const transport = postMessageTransport(port as unknown as Worker);
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    port.dispatchEvent(new Event("messageerror"));
    expect(closes).toEqual(["a message could not be read"]);
  });

  it("tauri_render_frames_arrive_as_bytes", async () => {
    const transport = await tauriTransport("control");
    const [, subscribed] = vi.mocked(invoke).mock.calls.find(([command]) => command === "subscribe")!;
    const backend = (subscribed as { onMessage: { onmessage(batch: string): void } }).onMessage;
    const connecting = ControlClient.connect(transport);
    const game = { components: [], resources: [], assets: [], events: [], fingerprint: "0000000000000000", input: null, tick_rate: 60, debug_layers: [] };
    backend.onmessage(JSON.stringify({ type: "welcome", protocol_version: PROTOCOL_VERSION, protocol_fingerprint: PROTOCOL_FINGERPRINT, engine_version: "0", game }));
    const client = await connecting;
    const received: RenderFramesEvent[] = [];
    client.on("render_frames", (event) => received.push(event));
    // One batch: what the game printed in one advance, a line each.
    const ticked = { type: "ticked", tick: 2, hash: "0000000000000001", pending: false, playing: true, effective_rate: 60 };
    const frames = { type: "render_frames", frames: ["AQID", "BA=="], progress: 0.25 };
    backend.onmessage([ticked, frames].map((event) => JSON.stringify({ type: "event", event })).join("\n"));
    expect(received).toEqual([{ type: "render_frames", progress: 0.25, frames: [new Uint8Array([1, 2, 3]), new Uint8Array([4])] }]);
    expect(received[0]!.frames[0]).toBeInstanceOf(Uint8Array);
  });

  it("tauri_close_detaches_its_own_backend_channel_once", async () => {
    const transport = await tauriTransport("control");
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    transport.close();
    transport.close();
    expect(vi.mocked(invoke).mock.calls.map(([command, args]) => [command, args])).toEqual([
      ["subscribe", { channel: "control", onMessage: expect.anything() }],
      ["unsubscribe", { channel: "control", id: 7 }],
    ]);
    expect(closes).toEqual(["closed"]);
  });
});
