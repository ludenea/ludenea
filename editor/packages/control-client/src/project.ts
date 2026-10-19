import { RequestTracker, handshake, parseFrame, unwrap } from "./client";
import {
  PROJECT_PROTOCOL_FINGERPRINT,
  PROJECT_PROTOCOL_VERSION,
  type ProjectClientMessage,
  type ProjectEvent,
  type ProjectOutcome,
  type ProjectReply,
  type ProjectReplyTypes,
  type ProjectRequest,
  type ProjectServerMessage,
} from "./generated/project";
import type { MessageTransport } from "./transports/transport";

/** The reply variant answering a project request of type `R`. */
export type ProjectReplyFor<R extends ProjectRequest> = Extract<ProjectReply, { type: ProjectReplyTypes[R["type"]] }>;

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
      // A message the service cannot read whole (one over its size limit) has no request id:
      // it refuses it, then closes the connection, so no waiting request will be answered.
      else if (message?.type === "refused") requests.rejectAll(message.error.message);
    });
    transport.onClose((reason) => {
      this.closed = true;
      requests.rejectAll(`connection closed: ${reason}`);
    });
  }

  static connect(transport: MessageTransport): Promise<ProjectClient> {
    const hello = {
      type: "hello",
      protocol_version: PROJECT_PROTOCOL_VERSION,
      protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT,
    } satisfies ProjectClientMessage;
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
