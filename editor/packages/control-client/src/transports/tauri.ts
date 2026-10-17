import { Channel, invoke } from "@tauri-apps/api/core";
import { Listeners, type MessageTransport } from "./transport";

export async function tauriTransport(channel: "control" | "project"): Promise<MessageTransport> {
  const messages = new Listeners<string>();
  const closes = new Listeners<string>();
  const onMessage = new Channel<string>();
  onMessage.onmessage = (message) => messages.emit(message);
  await invoke("subscribe", { channel, onMessage });
  return {
    send: (message) => {
      void invoke("send", { channel, message }).catch((error) => closes.emit(String(error)));
    },
    onMessage: (listener) => messages.add(listener),
    onClose: (listener) => closes.add(listener),
    close: () => closes.emit("closed"),
  };
}
