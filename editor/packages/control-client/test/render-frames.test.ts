import { afterEach, describe, expect, it, vi } from "vitest";
import { ControlClient, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, fakeTransport, postMessageTransport, type GameInfo, type MessageTransport, type RenderFramesEvent } from "../src";
import { startWorkerGame } from "../src/worker/supervisor";

const game: GameInfo = { components: [], resources: [], assets: [], events: [], fingerprint: "0000000000000000", input: null, tick_rate: 60, debug_layers: [] };
const welcome = JSON.stringify({ type: "welcome", protocol_version: PROTOCOL_VERSION, protocol_fingerprint: PROTOCOL_FINGERPRINT, engine_version: "0", game });

/** Stands in for a module worker: welcomes the client and lets the test post as the worker. */
class FakeWorker extends EventTarget {
  static last: FakeWorker;

  constructor() {
    super();
    FakeWorker.last = this;
  }

  postMessage(message: unknown): void {
    if (typeof message === "string" && JSON.parse(message).type === "hello") queueMicrotask(() => this.emit(welcome));
  }

  terminate(): void {}

  /** The worker posts `data` to the page. */
  emit(data: unknown): void {
    this.dispatchEvent(new MessageEvent("message", { data }));
  }
}

/** Connects a client over `transport`, then has the worker post a tick and two frames. */
async function framesOver(transport: MessageTransport, worker: { emit(data: unknown): void }) {
  const client = await ControlClient.connect(transport);
  const received: (RenderFramesEvent | { type: "ticked" })[] = [];
  client.on("ticked", () => received.push({ type: "ticked" }));
  client.on("render_frames", (event) => received.push(event));
  const ticked = { type: "ticked", tick: 2, hash: "0000000000000001", pending: false, playing: true, effective_rate: 60 };
  worker.emit(JSON.stringify({ type: "event", event: ticked }));
  worker.emit({ type: "render_frames", progress: 0.25, frames: [new Uint8Array([1, 2, 3]).buffer, new Uint8Array([4]).buffer] });
  return received;
}

const expected = [{ type: "ticked" }, { type: "render_frames", progress: 0.25, frames: [new Uint8Array([1, 2, 3]), new Uint8Array([4])] }];

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("render frames", () => {
  it("worker_frames_arrive_as_bytes", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    const started = startWorkerGame({ workerUrl: new URL("https://editor.test/host-worker.js"), glueUrl: "/pkg/game.js" });
    expect(await framesOver(started.transport, FakeWorker.last)).toEqual(expected);
  });

  it("every_transport_delivers_bytes", async () => {
    // A text transport (the desktop's standard output, a WebSocket) carries the same bytes in base64.
    const text = fakeTransport((message) => (JSON.parse(message).type === "hello" ? [welcome] : []));
    const worker = new FakeWorker();
    const overText = framesOver(text, { emit: (data) => text.push(typeof data === "string" ? data : JSON.stringify({ type: "event", event: { ...(data as object), frames: ["AQID", "BA=="] } })) });
    expect(await overText).toEqual(expected);
    expect(await framesOver(postMessageTransport(worker as unknown as Worker), worker)).toEqual(expected);
  });

  it("a_frame_that_is_not_base64_is_dropped_with_a_warning", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const text = fakeTransport((message) => (JSON.parse(message).type === "hello" ? [welcome] : []));
    const client = await ControlClient.connect(text);
    const received: RenderFramesEvent[] = [];
    client.on("render_frames", (event) => received.push(event));
    text.push(JSON.stringify({ type: "event", event: { type: "render_frames", progress: 0, frames: ["not base64!"] } }));
    text.push(JSON.stringify({ type: "event", event: { type: "render_frames", progress: 0.5, frames: ["BA=="] } }));
    expect(received).toEqual([{ type: "render_frames", progress: 0.5, frames: [new Uint8Array([4])] }]);
    expect(warn).toHaveBeenCalledWith(expect.stringMatching(/^ignoring render frames that are not base64: /));
    warn.mockRestore();
  });

  it("frames_decode_with_the_native_base64_decoder_when_there_is_one", async () => {
    const decoder = Uint8Array as { fromBase64?: (text: string) => Uint8Array };
    const native = decoder.fromBase64;
    const fromBase64 = vi.fn((text: string) => new Uint8Array([text.length]));
    decoder.fromBase64 = fromBase64;
    try {
      const text = fakeTransport((message) => (JSON.parse(message).type === "hello" ? [welcome] : []));
      const client = await ControlClient.connect(text);
      const received: RenderFramesEvent[] = [];
      client.on("render_frames", (event) => received.push(event));
      text.push(JSON.stringify({ type: "event", event: { type: "render_frames", progress: 0, frames: ["AQID"] } }));
      expect(fromBase64).toHaveBeenCalledWith("AQID");
      expect(received).toEqual([{ type: "render_frames", progress: 0, frames: [new Uint8Array([4])] }]);
    } finally {
      if (native) decoder.fromBase64 = native;
      else delete decoder.fromBase64;
    }
  });

  it("post_message_frames_arrive_as_bytes", async () => {
    const worker = new FakeWorker();
    expect(await framesOver(postMessageTransport(worker as unknown as Worker), worker)).toEqual(expected);
  });
});
