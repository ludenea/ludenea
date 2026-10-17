import { Listeners, type MessageTransport } from "./transport";

/** In-memory transport for tests: `handler` maps each sent message to the messages the peer answers with. */
export function fakeTransport(handler: (message: string) => string[]): MessageTransport & {
  push(message: string): void;
  drop(reason: string): void;
} {
  const messages = new Listeners<string>();
  const closes = new Listeners<string>();
  return {
    send: (message) => {
      for (const answer of handler(message)) messages.emit(answer);
    },
    onMessage: (listener) => messages.add(listener),
    onClose: (listener) => closes.add(listener),
    close: () => closes.emit("closed"),
    push: (message) => messages.emit(message),
    drop: (reason) => closes.emit(reason),
  };
}
