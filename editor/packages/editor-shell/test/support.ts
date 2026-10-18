import { ControlClient, ProjectClient, fakeTransport, type GameInfo, type Reply } from "@ludenea/control-client";
import { createEditor, type DocumentLog } from "../src/app";

const game: GameInfo = { components: [], resources: [], assets: [], events: [], fingerprint: "0000000000000000", input: null, tick_rate: 60, debug_layers: [] };

type Message = { type: string; [key: string]: unknown };
/** What a fake peer answers: a reply, `{ error }` to fail, or nothing when undefined. */
type Answer = (message: Message) => object | undefined;

const respond = (id: number, reply: object | undefined) =>
  reply ? [JSON.stringify({ type: "response", id, result: "error" in reply ? reply : { ok: reply } })] : [];

/** An editor on a project service that welcomes it and answers with `answer` (nothing by default). */
export async function testHost(importModule?: (url: string) => Promise<unknown>, answer: Answer = () => undefined, log?: DocumentLog) {
  const project = await ProjectClient.connect(
    fakeTransport((raw) => {
      const message = JSON.parse(raw);
      if (message.type === "hello") return [JSON.stringify({ type: "welcome", protocol_version: 2 })];
      return respond(message.id, answer(message.request));
    }),
  );
  return createEditor({ project, importModule, log });
}

/** A game whose host answers each command with `answer(command)` (nothing when undefined). */
export async function testGame(answer: Answer = () => undefined, info: Partial<GameInfo> = {}) {
  const sent: Message[] = [];
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") {
      return [JSON.stringify({ type: "welcome", protocol_version: 3, engine_version: "0", game: { ...game, ...info } })];
    }
    sent.push(message.command);
    return respond(message.id, answer(message.command));
  });
  const control = await ControlClient.connect(transport);
  return { control, sent, push: (event: object) => transport.push(JSON.stringify({ type: "event", event })) };
}

export const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A session as the game exports it, and the tick its timeline shows. */
export type Session = Omit<Extract<Reply, { type: "session" }>, "type"> & { current: number };

export const timeline = (current = 0) => ({
  type: "timeline",
  head: 2,
  simulated: 2,
  current,
  checkpoints: [0],
  edit_ticks: [],
  can_undo: false,
  can_redo: false,
  undo_label: null,
  redo_label: null,
  scene_dirty: false,
  resimulating: false,
  branch: 0,
  branches: [{ id: 0, name: "Branch 1", parent: null, fork_tick: 0, head: 2, stale: false }],
  compare: null,
});

/** A game that exports `session` and answers everything else with `Done`. */
export function gameWithSession(session: Session) {
  return testGame((command) => {
    const { current, ...exported } = session;
    if (command.type === "export_session") return { type: "session", ...exported };
    if (command.type === "get_timeline") return timeline(current);
    if (command.type === "load_scene") return { type: "loaded", warnings: [], instances: [], load_addresses: [] };
    return { type: "done" };
  });
}

export const ids = {
  main: "00000000000000000000000000000001",
  ball: "00000000000000000000000000000002",
  settings: "00000000000000000000000000000003",
  created: "00000000000000000000000000000004",
  first: "00000000000000000000000000000005",
  second: "00000000000000000000000000000006",
};

/** Stored sessions are the JSON of a `SavedSession`, base64-encoded. */
const encode = (value: unknown) => btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(value))));
const decode = (bytes: string) => JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(bytes), (char) => char.charCodeAt(0))));

/** A project service with a scene, a prefab, a data asset, and two replays on its disk, which stores sessions. */
export function fakeProject() {
  const disk: { [path: string]: string } = {
    "scenes/main.scene.ron": "(main)",
    "assets/ball.prefab.ron": "(ball)",
    "assets/settings.ron": "(gravity_scale: 0.5)",
  };
  const entries = [
    { path: "scenes/main.scene.ron", id: ids.main, kind: "Scene" },
    { path: "assets/ball.prefab.ron", id: ids.ball, kind: "Prefab" },
    { path: "assets/settings.ron", id: ids.settings, kind: { Data: { type: "demo::Settings" } } },
    { path: "replays/first.replay", id: ids.first, kind: "Replay" },
    { path: "replays/second.replay", id: ids.second, kind: "Replay" },
  ];
  const sessions = new Map<string, string>();
  const sent: Message[] = [];
  const assets = () => [
    [ids.ball, disk["assets/ball.prefab.ron"]],
    [ids.settings, `AssetFile(type: "demo::Settings", value: ${disk["assets/settings.ron"]})`],
  ];
  const answer: Answer = (request) => {
    sent.push(request);
    switch (request.type) {
      case "list_files":
        return { type: "files", entries };
      case "read_text":
        return { type: "text", text: disk[request.path as string] };
      case "read_asset_texts":
        return { type: "asset_texts", assets: assets(), paths: entries.slice(1, 3).map(({ id, path }) => [id, path]) };
      case "read_binary":
        return { type: "bytes", bytes: btoa(request.path as string) };
      case "store_session":
        sessions.set(request.document as string, request.bytes as string);
        return { type: "done" };
      case "load_stored_session":
        return { type: "stored_session", bytes: sessions.get(request.document as string) ?? null };
      case "create_asset":
        return { type: "file", path: request.path, id: ids.created, kind: request.kind };
      default:
        return { type: "done" };
    }
  };
  return {
    disk,
    sent,
    answer,
    assets,
    stored: (id: string) => (sessions.has(id) ? decode(sessions.get(id)!) : undefined),
    store: (id: string, saved: unknown) => sessions.set(id, encode(saved)),
  };
}
