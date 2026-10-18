import { Listeners, type MessageTransport } from "./transport";

export function webSocketTransport(url: string): Promise<MessageTransport> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    const messages = new Listeners<string>();
    const closes = new Listeners<string>();
    let open = false;
    socket.onmessage = (event) => messages.emit(String(event.data));
    // A WebSocket error carries no detail and is always followed by `close`, which has the code.
    socket.onclose = (event) => {
      const reason = event.reason || `closed (${event.code})`;
      if (open) closes.emit(reason);
      else reject(new Error(`cannot connect to ${url}: ${reason}`));
    };
    socket.onopen = () => {
      open = true;
      resolve({
        send: (message) => socket.send(message),
        onMessage: (listener) => messages.add(listener),
        onClose: (listener) => closes.add(listener),
        close: () => socket.close(),
      });
    };
  });
}
