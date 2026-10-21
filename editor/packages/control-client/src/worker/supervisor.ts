import { Listeners, isPushedFrames, type MessageTransport, type PushedFrames } from "../transports/transport";

export interface WorkerCrash {
  /** The tick the host was at when the failing call began, when the panic hook reported it. */
  tick?: number;
  message: string;
}

export interface WorkerGame {
  transport: MessageTransport;
  onCrashed(listener: (crash: WorkerCrash) => void): () => void;
  terminate(): void;
}

/** Starts a game's WebAssembly build in `host-worker.ts`. Protocol text travels as strings,
 * pushed render frames as transferred buffers; a crash (posted as an object, or an uncaught
 * worker error) is reported once and closes the transport. Recovering is up to the caller. */
export function startWorkerGame(options: { workerUrl: URL; glueUrl: string }): WorkerGame {
  const worker = new Worker(options.workerUrl, { type: "module" });
  const messages = new Listeners<string>();
  const frames = new Listeners<PushedFrames>();
  const closes = new Listeners<string>();
  const crashes = new Listeners<WorkerCrash>();
  // A crash or a termination ends the game: whichever comes first closes the transport, once.
  let ended = false;
  const crash = (report: WorkerCrash) => {
    if (ended) return;
    ended = true;
    crashes.emit(report);
    closes.emit(`the game crashed: ${report.message}`);
  };
  worker.addEventListener("message", (event) => {
    const data: unknown = event.data;
    if (typeof data === "string") messages.emit(data);
    else if (isPushedFrames(data)) frames.emit(data);
    else if ((data as { type?: unknown } | null)?.type === "crashed") {
      const { tick, message } = data as Partial<WorkerCrash>;
      crash({ ...(tick === undefined ? {} : { tick }), message: message ?? "the game crashed" });
    }
  });
  worker.addEventListener("error", (event) => crash({ message: event.message || "the game worker failed" }));
  worker.postMessage({ type: "init", glueUrl: options.glueUrl });
  const terminate = () => {
    worker.terminate();
    if (ended) return;
    ended = true;
    closes.emit("terminated");
  };
  return {
    transport: {
      send: (message) => worker.postMessage(message),
      onMessage: (listener) => messages.add(listener),
      onFrames: (listener) => frames.add(listener),
      onClose: (listener) => closes.add(listener),
      close: terminate,
    },
    onCrashed: (listener) => crashes.add(listener),
    terminate,
  };
}
