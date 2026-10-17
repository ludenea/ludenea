import { Listeners, type MessageTransport } from "./transport";

export function postMessageTransport(port: MessagePort | Worker): MessageTransport {
  const messages = new Listeners<string>();
  const closes = new Listeners<string>();
  port.addEventListener("message", (event) => messages.emit(String((event as MessageEvent).data)));
  if ("start" in port) port.start();
  return {
    send: (message) => port.postMessage(message),
    onMessage: (listener) => messages.add(listener),
    onClose: (listener) => closes.add(listener),
    close: () => {
      if ("terminate" in port) port.terminate();
      else port.close();
      closes.emit("closed");
    },
  };
}
