import { ControlClient } from "../../packages/control-client/src/client";
import { startWorkerGame } from "../../packages/control-client/src/worker/supervisor";
import workerUrl from "../../packages/control-client/src/worker/host-worker.ts?worker&url";
import { decodeTrace } from "../trace";

const TICKS = 10_000;

async function fetchBytes(url: string): Promise<Uint8Array> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`cannot fetch ${url}: ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

const hex = (hash: bigint) => hash.toString(16).padStart(16, "0");

/** Runs the boxes scenario in a worker and returns its hashes next to the golden ones. */
window.runGolden = async () => {
  const game = startWorkerGame({
    workerUrl: new URL(workerUrl, location.href),
    glueUrl: new URL("pkg/game.js", location.href).href,
  });
  game.onCrashed((crash) => console.error(`the game crashed at tick ${crash.tick}: ${crash.message}`));
  try {
    const client = await ControlClient.connect(game.transport);
    const ron = await (await fetch("/assets/boxes.scene.ron")).text();
    await client.request({ type: "load_scene", ron });
    const log = toBase64(await fetchBytes("/assets/boxes.inputs"));
    await client.request({ type: "load_inputs", log });
    const initial = await client.request({ type: "get_hash" });
    const stepped = await client.request({ type: "step", ticks: TICKS, record_hashes: true });
    const golden = decodeTrace(await fetchBytes("/golden/boxes.trace"));
    return { actual: [initial.hash, ...stepped.hashes], expected: golden.hashes.map(hex) };
  } finally {
    game.terminate();
  }
};
