import {
  PROTOCOL_FINGERPRINT,
  PROTOCOL_VERSION,
  type ClientMessage,
  type Command,
  type ErrorCode,
  type Event,
  type GameInfo,
  type Outcome,
  type Reply,
  type ReplyTypes,
  type ServerMessage,
  type Topic,
} from "./generated";
import type { MessageTransport } from "./transports/transport";

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
export type ReplyFor<C extends Command> = Extract<Reply, { type: ReplyTypes[C["type"]] }>;

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

const isObject = (value: unknown): value is { [key: string]: unknown } => typeof value === "object" && value !== null && !Array.isArray(value);

/** Whether `value` has the envelope of a server message: a `type`, and what responses and events carry. */
function isEnvelope(value: unknown): boolean {
  if (!isObject(value) || typeof value.type !== "string") return false;
  if (value.type === "response") return typeof value.id === "number" && isObject(value.result);
  if (value.type === "event") return isObject(value.event) && typeof value.event.type === "string";
  if (value.type === "refused") return isObject(value.error);
  return true;
}

/**
 * Parses an incoming frame; one that is not JSON or lacks the envelope of a server message is
 * ignored with a warning, so it cannot break delivery or hang requests.
 */
export function parseFrame<T>(raw: string): T | undefined {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch (error) {
    console.warn(`ignoring malformed message: ${String(error)}`);
    return undefined;
  }
  if (isEnvelope(value)) return value as T;
  console.warn(`ignoring malformed message: ${raw.slice(0, 200)}`);
  return undefined;
}

/** What a hello and a welcome both carry: the two must be equal. */
type ProtocolIdentity = { protocol_version: number; protocol_fingerprint: string };

const REBUILD = "rebuild the editor (pnpm -C editor build) and the engine from the same checkout";

/** Why a welcome from a peer built from another revision of the protocol is refused, if it is. */
function mismatch(hello: ProtocolIdentity, welcome: Partial<ProtocolIdentity>): ProtocolFailure<"version_mismatch"> | undefined {
  if (welcome.protocol_version !== hello.protocol_version) {
    return new ProtocolFailure(
      "version_mismatch",
      `the engine speaks protocol ${String(welcome.protocol_version)}, this client speaks ${hello.protocol_version}; ${REBUILD}`,
    );
  }
  if (welcome.protocol_fingerprint !== hello.protocol_fingerprint) {
    return new ProtocolFailure(
      "version_mismatch",
      `the engine speaks another revision of protocol ${hello.protocol_version} (fingerprint ${String(welcome.protocol_fingerprint)}, this client ${hello.protocol_fingerprint}); ${REBUILD}`,
    );
  }
  return undefined;
}

/**
 * Sends `hello` and resolves with what `accept` makes of the peer's welcome, called as the
 * welcome arrives so the client it builds misses nothing right behind it (a close). A refusal, or
 * a welcome of another protocol version or fingerprint than `hello`'s, closes the transport and
 * rejects with a `ProtocolFailure` carrying its code; a closed transport rejects too.
 */
export function handshake<M extends { type: string }, T>(
  transport: MessageTransport,
  hello: ProtocolIdentity & { type: "hello" },
  accept: (welcome: Extract<M, { type: "welcome" }>) => T,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const stop = () => {
      stopMessages();
      stopClose();
    };
    const fail = (failure: ProtocolFailure<string>) => {
      transport.close();
      reject(failure);
    };
    const stopMessages = transport.onMessage((raw) => {
      const message = parseFrame<M>(raw);
      if (message?.type === "welcome") {
        stop();
        const failure = mismatch(hello, message as Partial<ProtocolIdentity>);
        if (failure) fail(failure);
        else resolve(accept(message as Extract<M, { type: "welcome" }>));
      } else if (message?.type === "refused") {
        stop();
        const { code, message: text } = (message as unknown as { error: { code: string; message: string } }).error;
        fail(new ProtocolFailure(code, text));
      }
    });
    const stopClose = transport.onClose((reason) => reject(new Error(`connection closed: ${reason}`)));
    transport.send(JSON.stringify(hello));
  });
}

type Listener = (event: never) => void;

/** A topic's identity whatever its key order. */
const topicKey = (topic: Topic): string =>
  JSON.stringify(topic, (_, value) =>
    value && typeof value === "object" && !Array.isArray(value) ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b))) : value,
  );

/**
 * A client of the control protocol.
 *
 * Component and resource values travel as JSON in serde's externally tagged form. An
 * integer whose magnitude exceeds `Number.MAX_SAFE_INTEGER` (2^53 - 1) travels as a string
 * of decimal digits, in both directions: send such an integer as a string, and read
 * `u64`/`i64` fields as `number | string`. Hashes are 16-digit hex strings.
 */
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
    const hello = { type: "hello", protocol_version: PROTOCOL_VERSION, protocol_fingerprint: PROTOCOL_FINGERPRINT } satisfies ClientMessage;
    return handshake<ServerMessage, ControlClient>(transport, hello, (welcome) => new ControlClient(transport, new RequestTracker(transport), welcome.game));
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

  /** Calls `listener` when the connection to the game ends (it exited, crashed, or was closed). */
  onClose(listener: (reason: string) => void): () => void {
    return this.transport.onClose(listener);
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
