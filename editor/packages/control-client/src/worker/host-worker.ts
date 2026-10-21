// The module worker that hosts a game's WebAssembly build. The first message is
// `{ type: "init", glueUrl }`; every later one is protocol JSON text for the host.

/** What `ludenea::worker_entry!` exports, as the wasm-bindgen glue exposes it. */
interface WorkerHost {
  handle(message: string): string;
  advance(elapsedMs: number): string;
  /** The frames the last `advance` pushed, once. */
  takeRenderFrames(): { type: "render_frames"; progress: number; frames: ArrayBuffer[] } | undefined;
}

interface Glue {
  default(): Promise<unknown>;
  createHost(): WorkerHost;
}

/** The part of the dedicated worker scope used here (the DOM library types `self` as a window). */
declare const self: {
  postMessage(message: unknown, transfer?: Transferable[]): void;
  addEventListener(type: "message", listener: (event: MessageEvent<unknown>) => void): void;
};

const FRAME_MS = 16;

let host: Promise<WorkerHost | undefined> = Promise.resolve(undefined);
let timer: ReturnType<typeof setInterval> | undefined;
let crashed = false;

/** Stops the host for good and reports why. A panic aborts the instance with a
 * `WebAssembly.RuntimeError`; the panic hook usually reported it first, and the
 * supervisor keeps the first report. */
function crash(error: unknown): undefined {
  clearInterval(timer);
  crashed = true;
  self.postMessage({ type: "crashed", message: String(error) });
  return undefined;
}

/** Runs `call` on the host and posts each message of the JSON array it returns. */
function run(call: () => string): void {
  if (crashed) return;
  try {
    for (const message of JSON.parse(call()) as unknown[]) self.postMessage(JSON.stringify(message));
  } catch (error) {
    crash(error);
  }
}

/** Posts the frames the last advance pushed, after its events, their buffers transferred. */
function postFrames(host: WorkerHost): void {
  if (crashed) return;
  try {
    const pushed = host.takeRenderFrames();
    if (pushed) self.postMessage(pushed, pushed.frames);
  } catch (error) {
    crash(error);
  }
}

async function start(glueUrl: string): Promise<WorkerHost | undefined> {
  try {
    const glue = (await import(/* @vite-ignore */ glueUrl)) as Glue;
    await glue.default();
    const created = glue.createHost();
    let last = performance.now();
    timer = setInterval(() => {
      const now = performance.now();
      run(() => created.advance(now - last));
      postFrames(created);
      last = now;
    }, FRAME_MS);
    return created;
  } catch (error) {
    return crash(error);
  }
}

self.addEventListener("message", (event) => {
  const data = event.data;
  if (typeof data === "string") {
    // Requests wait for the host and keep their order.
    host = host.then((ready) => {
      if (ready) run(() => ready.handle(data));
      return ready;
    });
  } else if ((data as { type?: unknown } | null)?.type === "init") {
    host = start((data as { glueUrl: string }).glueUrl);
  }
});

export {};
