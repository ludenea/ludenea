import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { postMessageTransport, tauriTransport, webSocketTransport } from "../src";

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
