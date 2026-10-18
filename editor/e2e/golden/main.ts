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

/** Runs `scenario` on the boxes game in a worker. */
async function withGame<T>(scenario: (client: ControlClient) => Promise<T>): Promise<T> {
  const game = startWorkerGame({
    workerUrl: new URL(workerUrl, location.href),
    glueUrl: new URL("pkg/game.js", location.href).href,
  });
  game.onCrashed((crash) => console.error(`the game crashed at tick ${crash.tick}: ${crash.message}`));
  try {
    return await scenario(await ControlClient.connect(game.transport));
  } finally {
    game.terminate();
  }
}

/** Runs the boxes scenario in a worker and returns its hashes next to the golden ones. */
window.runGolden = () =>
  withGame(async (client) => {
    const ron = await (await fetch("/assets/boxes.scene.ron")).text();
    await client.request({ type: "load_scene", scene: ron, assets: [], document: "Scene" });
    const log = toBase64(await fetchBytes("/assets/boxes.inputs"));
    await client.request({ type: "load_inputs", log });
    const initial = await client.request({ type: "get_hash" });
    const stepped = await client.request({ type: "step", ticks: TICKS, record_hashes: true });
    const golden = decodeTrace(await fetchBytes("/golden/boxes.trace"));
    return { actual: [initial.hash, ...stepped.hashes], expected: golden.hashes.map(hex) };
  });

/** Opens the golden replay in a worker and lets the game resimulate it against its recorded hashes. */
window.runReplay = () =>
  withGame(async (client) => {
    const diverged: number[] = [];
    client.on("diverged", ({ tick }) => diverged.push(tick));
    const replay = toBase64(await fetchBytes("/golden/spawner.replay"));
    await client.request({ type: "load_replay", replay });
    let timeline = await client.request({ type: "get_timeline" });
    while (timeline.resimulating) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      timeline = await client.request({ type: "get_timeline" });
    }
    const { hashes } = await client.request({ type: "export_session" });
    return { simulated: timeline.simulated, recorded: hashes.length, diverged };
  });
