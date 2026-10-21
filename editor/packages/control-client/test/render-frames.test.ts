import { afterEach, describe, expect, it, vi } from "vitest";
import { ControlClient, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, postMessageTransport, type GameInfo, type MessageTransport, type RenderFramesEvent } from "../src";
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
async function framesOver(transport: MessageTransport, worker: FakeWorker) {
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

  it("post_message_frames_arrive_as_bytes", async () => {
    const worker = new FakeWorker();
    expect(await framesOver(postMessageTransport(worker as unknown as Worker), worker)).toEqual(expected);
  });
});
