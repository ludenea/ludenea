import { Listeners, type MessageTransport } from "./transport";

/** Over a worker or a message port. An uncaught worker error or an unreadable message closes it. */
export function postMessageTransport(port: MessagePort | Worker): MessageTransport {
  const messages = new Listeners<string>();
  const closes = new Listeners<string>();
  let closed = false;
  const close = (reason: string) => {
    if (closed) return;
    closed = true;
    if ("terminate" in port) port.terminate();
    else port.close();
    closes.emit(reason);
  };
  port.addEventListener("message", (event) => messages.emit(String((event as MessageEvent).data)));
  port.addEventListener("error", (event) => close(`the worker failed: ${(event as ErrorEvent).message || "unknown error"}`));
  port.addEventListener("messageerror", () => close("a message could not be read"));
  if ("start" in port) port.start();
  return {
    send: (message) => port.postMessage(message),
    onMessage: (listener) => messages.add(listener),
    onClose: (listener) => closes.add(listener),
    close: () => close("closed"),
  };
}
