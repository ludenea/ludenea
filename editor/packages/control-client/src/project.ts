import { RequestTracker, handshake, parseFrame, unwrap } from "./client";
import type {
  ProjectClientMessage,
  ProjectEvent,
  ProjectOutcome,
  ProjectReply,
  ProjectRequest,
  ProjectServerMessage,
} from "./generated/project";
import type { MessageTransport } from "./transports/transport";

export const PROJECT_PROTOCOL_VERSION = 2;

/** The reply variant answering a project request of type `R`. */
export type ProjectReplyFor<R extends ProjectRequest> = R extends { type: "info" | "open" | "create" }
  ? Extract<ProjectReply, { type: "info" }>
  : R extends { type: "list_files" }
    ? Extract<ProjectReply, { type: "files" }>
    : R extends { type: "read_text" }
      ? Extract<ProjectReply, { type: "text" }>
      : R extends { type: "list_plugins" }
        ? Extract<ProjectReply, { type: "plugins" }>
        : R extends { type: "game_endpoint" }
          ? Extract<ProjectReply, { type: "native" | "wasm" }>
          : Extract<ProjectReply, { type: "done" }>;

type Listener = (event: never) => void;

/** Client of the project protocol, in the same shape as the control client. */
export class ProjectClient {
  private readonly listeners = new Map<string, Set<Listener>>();
  private closed = false;

  private constructor(
    private readonly transport: MessageTransport,
    private readonly requests: RequestTracker<ProjectOutcome>,
  ) {
    transport.onMessage((raw) => {
      const message = parseFrame<ProjectServerMessage>(raw);
      if (message?.type === "response") requests.resolve(message.id, message.result);
      else if (message?.type === "event") this.dispatch(message.event);
    });
    transport.onClose((reason) => {
      this.closed = true;
      requests.rejectAll(`connection closed: ${reason}`);
    });
  }

  static connect(transport: MessageTransport): Promise<ProjectClient> {
    const hello: ProjectClientMessage = { type: "hello", protocol_version: PROJECT_PROTOCOL_VERSION };
    return handshake<ProjectServerMessage, ProjectClient>(transport, hello, () => new ProjectClient(transport, new RequestTracker<ProjectOutcome>(transport)));
  }

  async request<R extends ProjectRequest>(request: R): Promise<ProjectReplyFor<R>> {
    if (this.closed) throw new Error("connection closed");
    const result = await this.requests.send((id): ProjectClientMessage => ({ type: "request", id, request }));
    return unwrap(result) as ProjectReplyFor<R>;
  }

  on<T extends ProjectEvent["type"]>(type: T, listener: (event: Extract<ProjectEvent, { type: T }>) => void): () => void {
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

  private dispatch(event: ProjectEvent): void {
    for (const listener of [...(this.listeners.get(event.type) ?? [])]) listener(event as never);
  }
}
