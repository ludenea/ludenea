import {
  ControlClient,
  PROJECT_PROTOCOL_FINGERPRINT,
  PROJECT_PROTOCOL_VERSION,
  PROTOCOL_FINGERPRINT,
  PROTOCOL_VERSION,
  ProjectClient,
  fakeTransport,
  type GameInfo,
  type Reply,
} from "@ludenea/control-client";
import { createEditor, type EditorHost } from "../src/app";

const game: GameInfo = { components: [], resources: [], assets: [], events: [], fingerprint: "0000000000000000", input: null, tick_rate: 60, debug_layers: [] };

type Message = { type: string; [key: string]: unknown };
/** What a fake peer answers: a reply, `{ error }` to fail, or nothing when undefined. */
type Answer = (message: Message) => object | undefined;

const respond = (id: number, reply: object | undefined) =>
  reply ? [JSON.stringify({ type: "response", id, result: "error" in reply ? reply : { ok: reply } })] : [];

/** An editor on a project service that welcomes it and answers with `answer` (nothing by default). */
export async function testHost(importModule?: (url: string) => Promise<unknown>, answer: Answer = () => undefined) {
  const project = await ProjectClient.connect(
    fakeTransport((raw) => {
      const message = JSON.parse(raw);
      if (message.type === "hello") {
        return [JSON.stringify({ type: "welcome", protocol_version: PROJECT_PROTOCOL_VERSION, protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT })];
      }
      return respond(message.id, answer(message.request));
    }),
  );
  return createEditor({ project, importModule });
}

/** What the editor console of `host` holds, as `[level, text, location]`. */
export const logged = (host: EditorHost) => host.editor.console.list().map(({ level, text, location }) => [level, text, location]);

/** A game whose host answers each command with `answer(command)` (nothing when undefined). */
export async function testGame(answer: Answer = () => undefined, info: Partial<GameInfo> = {}) {
  const sent: Message[] = [];
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") {
      const welcome = { type: "welcome", protocol_version: PROTOCOL_VERSION, protocol_fingerprint: PROTOCOL_FINGERPRINT, engine_version: "0" };
      return [JSON.stringify({ ...welcome, game: { ...game, ...info } })];
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
  unsaved_assets: [],
  resimulating: false,
  branch: 0,
  branches: [{ id: 0, name: "Branch 1", parent: null, fork_tick: 0, head: 2, stale: false, edit_ticks: [] }],
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

/** `value` as JSON, base64-encoded: a stored session is `{ version: 1, saved }`. */
export const storedBytes = (value: unknown) => btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(value))));
const decode = (bytes: string) => JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(bytes), (char) => char.charCodeAt(0))));

/** Where the project keeps the stored session of `document`. */
export const sessionFile = (document: string) => `.ludenea/sessions/${document}.session`;

/** A project service with a scene, a prefab, a data asset, and two replays on its disk, which stores sessions and sets them aside. */
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
  // The files of `.ludenea/sessions/`, by path.
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
        sessions.set(sessionFile(request.document as string), request.bytes as string);
        return { type: "done" };
      case "load_stored_session":
        return { type: "stored_session", bytes: sessions.get(sessionFile(request.document as string)) ?? null };
      case "set_aside_stored_session": {
        const file = sessionFile(request.document as string);
        const bytes = sessions.get(file);
        if (bytes === undefined) return { type: "set_aside", path: null };
        let n = 1;
        while (sessions.has(`.ludenea/sessions/${request.document}.${n}.rejected.session`)) n++;
        const path = `.ludenea/sessions/${request.document}.${n}.rejected.session`;
        sessions.set(path, bytes);
        sessions.delete(file);
        return { type: "set_aside", path };
      }
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
    sessions,
    /** The stored session of `id` as the editor wrote it: `{ version, saved }`. */
    stored: (id: string) => (sessions.has(sessionFile(id)) ? decode(sessions.get(sessionFile(id))!) : undefined),
    /** Stores `saved` as the session of `id`, as the editor does. */
    store: (id: string, saved: unknown) => sessions.set(sessionFile(id), storedBytes({ version: 1, saved })),
  };
}
