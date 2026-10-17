/** A recorded run, as `ludenea::cli::Trace` encodes it with postcard. */
export interface Trace {
  version: number;
  sceneHash: bigint;
  inputsHash: bigint;
  /** `hashes[k]` is the state hash after `k` ticks. */
  hashes: bigint[];
}

/** Decodes a trace file: postcard writes every integer as an unsigned LEB128 varint. */
export function decodeTrace(bytes: Uint8Array): Trace {
  let offset = 0;
  const varint = (): bigint => {
    let value = 0n;
    for (let shift = 0n; ; shift += 7n) {
      const byte = bytes[offset++];
      if (byte === undefined) throw new Error("trace ends inside a number");
      value |= BigInt(byte & 0x7f) << shift;
      if (byte < 0x80) return value;
    }
  };
  const version = Number(varint());
  const sceneHash = varint();
  const inputsHash = varint();
  const hashes = Array.from({ length: Number(varint()) }, varint);
  if (offset !== bytes.length) throw new Error("trailing bytes after the trace");
  return { version, sceneHash, inputsHash, hashes };
}
