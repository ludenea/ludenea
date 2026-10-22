/** The render frames a worker posts as bytes: each a postcard-encoded `RenderFrame` in a
 * buffer of its own. */
export interface PushedFrames {
  type: "render_frames";
  progress: number;
  frames: ArrayBuffer[];
}

/** Whether `data`, posted by a worker, is `PushedFrames`. */
export function isPushedFrames(data: unknown): data is PushedFrames {
  const value = data as Partial<PushedFrames> | null;
  return value?.type === "render_frames" && typeof value.progress === "number" && Array.isArray(value.frames) && value.frames.every((frame) => frame instanceof ArrayBuffer);
}

/** A bidirectional channel of text messages (one JSON document each). */
export interface MessageTransport {
  send(message: string): void;
  onMessage(listener: (message: string) => void): () => void;
  /** The frames posted as bytes, in order with the text messages; only the transports that
   * carry bytes have it (the text transports carry them in base64, in their `render_frames` events). */
  onFrames?(listener: (frames: PushedFrames) => void): () => void;
  onClose(listener: (reason: string) => void): () => void;
  close(): void;
}

/** Shared listener bookkeeping for the transport implementations. */
export class Listeners<T> {
  private readonly set = new Set<(value: T) => void>();

  add(listener: (value: T) => void): () => void {
    this.set.add(listener);
    return () => this.set.delete(listener);
  }

  emit(value: T): void {
    for (const listener of [...this.set]) listener(value);
  }
}
