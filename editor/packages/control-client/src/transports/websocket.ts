import { Listeners, type MessageTransport } from "./transport";

export function webSocketTransport(url: string): Promise<MessageTransport> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    const messages = new Listeners<string>();
    const closes = new Listeners<string>();
    socket.onmessage = (event) => messages.emit(String(event.data));
    socket.onclose = (event) => closes.emit(event.reason || `closed (${event.code})`);
    socket.onerror = () => reject(new Error(`cannot connect to ${url}`));
    socket.onopen = () =>
      resolve({
        send: (message) => socket.send(message),
        onMessage: (listener) => messages.add(listener),
        onClose: (listener) => closes.add(listener),
        close: () => socket.close(),
      });
  });
}
