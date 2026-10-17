/** A bidirectional channel of text messages (one JSON document each). */
export interface MessageTransport {
  send(message: string): void;
  onMessage(listener: (message: string) => void): () => void;
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
