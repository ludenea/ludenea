import type {
  ClientMessage,
  Command,
  ErrorCode,
  Event,
  GameInfo,
  Outcome,
  Reply,
  ServerMessage,
  Topic,
} from "./generated";
import type { MessageTransport } from "./transports/transport";

export const PROTOCOL_VERSION = 2;

export class ProtocolFailure<C extends string = ErrorCode> extends Error {
  constructor(
    readonly code: C,
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

type Pending<O> = { resolve: (result: O) => void; reject: (error: Error) => void };

/** Matches responses to requests by increasing id; shared by the control and project clients. */
export class RequestTracker<O = Outcome> {
  private nextId = 1;
  private readonly pending = new Map<number, Pending<O>>();

  constructor(private readonly transport: MessageTransport) {}

  send(build: (id: number) => unknown): Promise<O> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.transport.send(JSON.stringify(build(id)));
    });
  }

  resolve(id: number, result: O): void {
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

export function unwrap<R, C extends string>(result: { ok: R } | { error: { code: C; message: string } }): R {
  if ("error" in result) throw new ProtocolFailure(result.error.code, result.error.message);
  return result.ok;
}

/** Parses an incoming frame; a malformed one is ignored with a warning so it cannot break delivery or hang requests. */
export function parseFrame<T>(raw: string): T | undefined {
  try {
    return JSON.parse(raw) as T;
  } catch (error) {
    console.warn(`ignoring malformed message: ${String(error)}`);
    return undefined;
  }
}

type Listener = (event: never) => void;

/** A topic's identity whatever its key order. */
const topicKey = (topic: Topic): string =>
  JSON.stringify(topic, (_, value) =>
    value && typeof value === "object" && !Array.isArray(value) ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b))) : value,
  );

export class ControlClient {
  private readonly listeners = new Map<string, Set<Listener>>();
  /** Subscribers per topic: the host keeps one flag per client, so only the first and last reach it. */
  private readonly subscriptions = new Map<string, { count: number; subscribed: Promise<unknown> }>();
  private closed = false;

  private constructor(
    private readonly transport: MessageTransport,
    private readonly requests: RequestTracker,
    readonly game: GameInfo,
  ) {
    transport.onMessage((raw) => {
      const message = parseFrame<ServerMessage>(raw);
      if (message?.type === "response") requests.resolve(message.id, message.result);
      else if (message?.type === "event") this.dispatch(message.event);
    });
    transport.onClose((reason) => {
      this.closed = true;
      requests.rejectAll(`connection closed: ${reason}`);
    });
  }

  static connect(transport: MessageTransport): Promise<ControlClient> {
    return new Promise((resolve, reject) => {
      const stopMessages = transport.onMessage((raw) => {
        const message = parseFrame<ServerMessage>(raw);
        if (message?.type === "welcome") {
          stopMessages();
          stopClose();
          resolve(new ControlClient(transport, new RequestTracker(transport), message.game));
        } else if (message?.type === "refused") {
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

  /**
   * Sends `command` and resolves with its reply. `Subscribe` and `Unsubscribe` are counted per
   * topic: only the first subscription and the last unsubscription are sent, so panels sharing a
   * topic do not cancel each other.
   */
  async request<C extends Command>(command: C): Promise<ReplyFor<C>> {
    if (command.type === "subscribe" || command.type === "unsubscribe") {
      return (await this.counted(command.type, command.topic)) as ReplyFor<C>;
    }
    return this.send(command);
  }

  private async counted(type: "subscribe" | "unsubscribe", topic: Topic): Promise<Reply> {
    const key = topicKey(topic);
    const entry = this.subscriptions.get(key);
    if (type === "subscribe") {
      if (entry) {
        entry.count++;
        await entry.subscribed;
        return { type: "done" };
      }
      const subscribed = this.send({ type, topic });
      this.subscriptions.set(key, { count: 1, subscribed });
      // A refused subscription (an invalid filter) counts for nothing.
      return subscribed.catch((error) => {
        this.subscriptions.delete(key);
        throw error;
      });
    }
    if (!entry) return { type: "done" };
    if (--entry.count > 0) return { type: "done" };
    this.subscriptions.delete(key);
    return this.send({ type, topic });
  }

  private async send<C extends Command>(command: C): Promise<ReplyFor<C>> {
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
