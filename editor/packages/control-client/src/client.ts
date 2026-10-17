import type {
  ClientMessage,
  Command,
  ErrorCode,
  Event,
  GameInfo,
  Outcome,
  Reply,
  ServerMessage,
} from "./generated";
import type { MessageTransport } from "./transports/transport";

export const PROTOCOL_VERSION = 2;

export class ProtocolFailure extends Error {
  constructor(
    readonly code: ErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "ProtocolFailure";
  }
}

/** The reply variant answering a command of type `C`. */
export type ReplyFor<C extends Command> = C extends { type: "get_hash" }
  ? Extract<Reply, { type: "hash" }>
  : C extends { type: "get_hash_tree" }
    ? Extract<Reply, { type: "hash_tree" }>
    : C extends { type: "step" }
      ? Extract<Reply, { type: "stepped" }>
      : C extends { type: "take_snapshot" }
        ? Extract<Reply, { type: "snapshot" }>
        : C extends { type: "diff_snapshots" }
          ? Extract<Reply, { type: "differences" }>
          : C extends { type: "query_entities" }
            ? Extract<Reply, { type: "entities" }>
            : C extends { type: "get_entity" }
              ? Extract<Reply, { type: "entity" }>
              : C extends { type: "get_resources" }
                ? Extract<Reply, { type: "resources" }>
                : C extends { type: "spawn" }
                  ? Extract<Reply, { type: "spawned" }>
                  : C extends { type: "save_scene" }
                    ? Extract<Reply, { type: "scene" }>
                    : C extends { type: "get_timeline" }
                      ? Extract<Reply, { type: "timeline" }>
                      : C extends { type: "export_session" }
                        ? Extract<Reply, { type: "session" }>
                        : C extends { type: "get_schedule" }
                          ? Extract<Reply, { type: "schedule" }>
                          : C extends { type: "get_tick_events" }
                            ? Extract<Reply, { type: "tick_events" }>
                            : C extends { type: "get_tick_commands" }
                              ? Extract<Reply, { type: "tick_commands" }>
                              : C extends { type: "get_debug_draw" }
                                ? Extract<Reply, { type: "debug_draw" }>
                                : C extends { type: "pick" }
                                  ? Extract<Reply, { type: "picked" }>
                                  : Extract<Reply, { type: "done" }>;

type Pending = { resolve: (result: Outcome) => void; reject: (error: Error) => void };

/** Matches responses to requests by increasing id; shared by the control and project clients. */
export class RequestTracker {
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();

  constructor(private readonly transport: MessageTransport) {}

  send(build: (id: number) => unknown): Promise<Outcome> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.transport.send(JSON.stringify(build(id)));
    });
  }

  resolve(id: number, result: Outcome): void {
    const entry = this.pending.get(id);
    if (!entry) return;
    this.pending.delete(id);
    entry.resolve(result);
  }

  rejectAll(reason: string): void {
    for (const entry of this.pending.values()) entry.reject(new Error(reason));
    this.pending.clear();
  }
}

export function unwrap(result: Outcome): Reply {
  if ("error" in result) throw new ProtocolFailure(result.error.code, result.error.message);
  return result.ok;
}

type Listener = (event: never) => void;

export class ControlClient {
  private readonly listeners = new Map<string, Set<Listener>>();
  private closed = false;

  private constructor(
    private readonly transport: MessageTransport,
    private readonly requests: RequestTracker,
    readonly game: GameInfo,
  ) {
    transport.onMessage((raw) => {
      const message = JSON.parse(raw) as ServerMessage;
      if (message.type === "response") requests.resolve(message.id, message.result);
      else if (message.type === "event") this.dispatch(message.event);
    });
    transport.onClose((reason) => {
      this.closed = true;
      requests.rejectAll(`connection closed: ${reason}`);
    });
  }

  static connect(transport: MessageTransport): Promise<ControlClient> {
    return new Promise((resolve, reject) => {
      const stopMessages = transport.onMessage((raw) => {
        const message = JSON.parse(raw) as ServerMessage;
        if (message.type === "welcome") {
          stopMessages();
          stopClose();
          resolve(new ControlClient(transport, new RequestTracker(transport), message.game));
        } else if (message.type === "refused") {
          stopMessages();
          stopClose();
          reject(new Error(message.error.message));
        }
      });
      const stopClose = transport.onClose((reason) => reject(new Error(`connection closed: ${reason}`)));
      const hello: ClientMessage = { type: "hello", protocol_version: PROTOCOL_VERSION };
      transport.send(JSON.stringify(hello));
    });
  }

  async request<C extends Command>(command: C): Promise<ReplyFor<C>> {
    if (this.closed) throw new Error("connection closed");
    const result = await this.requests.send(
      (id): ClientMessage => ({ type: "request", id, command }),
    );
    return unwrap(result) as ReplyFor<C>;
  }

  on<T extends Event["type"]>(type: T, listener: (event: Extract<Event, { type: T }>) => void): () => void {
    const set = this.listeners.get(type) ?? new Set<Listener>();
    this.listeners.set(type, set);
    set.add(listener as Listener);
    return () => set.delete(listener as Listener);
  }

  close(): void {
    this.closed = true;
    this.requests.rejectAll("client closed");
    this.transport.close();
  }

  private dispatch(event: Event): void {
    for (const listener of [...(this.listeners.get(event.type) ?? [])]) listener(event as never);
  }
}
