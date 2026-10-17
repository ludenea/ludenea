import { RequestTracker, unwrap } from "./client";
import type { MessageTransport } from "./transports/transport";

// Replaced by the generated project protocol types.
export type ProjectRequest = { type: string };

/** Client of the project protocol: same request/response shape as the control client, without a handshake. */
export class ProjectClient {
  private readonly requests: RequestTracker;

  constructor(private readonly transport: MessageTransport) {
    this.requests = new RequestTracker(transport);
    transport.onMessage((raw) => {
      const message = JSON.parse(raw) as { type: string; id?: number; result?: never };
      if (message.type === "response" && message.id !== undefined) this.requests.resolve(message.id, message.result!);
    });
    transport.onClose((reason) => this.requests.rejectAll(`connection closed: ${reason}`));
  }

  async request<R extends ProjectRequest>(request: R): Promise<unknown> {
    return unwrap(await this.requests.send((id) => ({ type: "request", id, request })));
  }

  close(): void {
    this.requests.rejectAll("client closed");
    this.transport.close();
  }
}
