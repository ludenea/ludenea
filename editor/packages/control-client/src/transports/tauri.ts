import { Channel, invoke } from "@tauri-apps/api/core";
import { Listeners, type MessageTransport } from "./transport";

/** The desktop backend's channels. It streams batches: protocol messages one per line. */
export async function tauriTransport(channel: "control" | "project"): Promise<MessageTransport> {
  const messages = new Listeners<string>();
  const closes = new Listeners<string>();
  const onMessage = new Channel<string>();
  onMessage.onmessage = (batch) => {
    for (const message of batch.split("\n")) messages.emit(message);
  };
  await invoke("subscribe", { channel, onMessage });
  let closed = false;
  return {
    send: (message) => {
      void invoke("send", { channel, message }).catch((error) => closes.emit(String(error)));
    },
    onMessage: (listener) => messages.add(listener),
    onClose: (listener) => closes.add(listener),
    close: () => {
      if (closed) return;
      closed = true;
      // Names this channel, so it cannot detach a newer subscription that overtook it.
      void invoke("unsubscribe", { channel, id: onMessage.id }).catch((error) => console.warn(`channel ${channel} not detached: ${String(error)}`));
      closes.emit("closed");
    },
  };
}
