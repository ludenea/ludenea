import { afterEach, describe, expect, it, vi } from "vitest";
import { startWorkerGame } from "../src/worker/supervisor";

/** Stands in for a module worker: records what the page posts and lets the test act as the worker. */
class FakeWorker extends EventTarget {
  static last: FakeWorker;
  readonly posted: unknown[] = [];
  terminated = false;

  constructor(
    readonly url: URL,
    readonly options: WorkerOptions,
  ) {
    super();
    FakeWorker.last = this;
  }

  postMessage(message: unknown): void {
    this.posted.push(message);
  }

  terminate(): void {
    this.terminated = true;
  }

  /** The worker posts `data` to the page. */
  emit(data: unknown): void {
    this.dispatchEvent(new MessageEvent("message", { data }));
  }
}

function start() {
  vi.stubGlobal("Worker", FakeWorker);
  const game = startWorkerGame({ workerUrl: new URL("https://editor.test/host-worker.js"), glueUrl: "/pkg/game.js" });
  const crashes: { tick?: number; message: string }[] = [];
  game.onCrashed((crash) => crashes.push(crash));
  return { game, worker: FakeWorker.last, crashes };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("startWorkerGame", () => {
  it("starts_a_module_worker_with_the_glue_and_relays_protocol_text", () => {
    const { game, worker } = start();
    expect(worker.options).toEqual({ type: "module" });
    expect(worker.posted).toEqual([{ type: "init", glueUrl: "/pkg/game.js" }]);
    const received: string[] = [];
    game.transport.onMessage((message) => received.push(message));
    game.transport.send('{"type":"hello","protocol_version":2}');
    worker.emit('{"type":"welcome"}');
    expect(worker.posted.at(-1)).toBe('{"type":"hello","protocol_version":2}');
    expect(received).toEqual(['{"type":"welcome"}']);
  });

  it("crash_message_reaches_listener", () => {
    const { game, worker, crashes } = start();
    const closes: string[] = [];
    game.transport.onClose((reason) => closes.push(reason));
    worker.emit({ type: "crashed", tick: 50, message: "boom at fifty" });
    // The fallback report that follows the hook's is the same crash.
    worker.emit({ type: "crashed", message: "RuntimeError: unreachable" });
    expect(crashes).toEqual([{ tick: 50, message: "boom at fifty" }]);
    expect(closes).toEqual(["the game crashed: boom at fifty"]);
  });

  it("runtime_error_without_hook_reports_crash", () => {
    const { worker, crashes } = start();
    worker.emit({ type: "crashed", message: "RuntimeError: unreachable" });
    expect(crashes).toEqual([{ message: "RuntimeError: unreachable" }]);
  });

  it("uncaught_worker_error_reports_crash", () => {
    const { worker, crashes } = start();
    worker.dispatchEvent(Object.assign(new Event("error"), { message: "cannot load the glue" }));
    expect(crashes).toEqual([{ message: "cannot load the glue" }]);
  });

  it("terminate_stops_the_worker", () => {
    const { game, worker } = start();
    game.terminate();
    expect(worker.terminated).toBe(true);
  });

  it("the_transport_closes_once_whatever_ends_the_game", () => {
    const { game, worker } = start();
    const closes: string[] = [];
    game.transport.onClose((reason) => closes.push(reason));
    worker.emit({ type: "crashed", message: "boom" });
    game.terminate();
    game.transport.close();
    expect(closes).toEqual(["the game crashed: boom"]);

    const other = start();
    const otherCloses: string[] = [];
    other.game.transport.onClose((reason) => otherCloses.push(reason));
    other.game.terminate();
    other.game.terminate();
    expect(otherCloses).toEqual(["terminated"]);
  });
});
