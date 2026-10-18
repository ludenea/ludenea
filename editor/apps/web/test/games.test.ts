import { ControlClient, ProjectClient, fakeTransport, type GameInfo, type WorkerCrash, type WorkerGame } from "@ludenea/control-client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { followNativeGames, followWasmGames } from "../src/games";

type Message = { type: string; [key: string]: unknown };

const info: GameInfo = { components: [], resources: [], assets: [], events: [], fingerprint: "0000000000000000", input: null, tick_rate: 60, debug_layers: [] };
const timeline = { head: 5, simulated: 5, current: 3, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, scene_dirty: false, resimulating: false };
const hashes = ["1", "2", "3", "4", "5", "6"].map((digit) => digit.padStart(16, "0"));

/** Everything sent to the project service and to every game, in order, as `<peer>:<type>`. */
let log: string[] = [];
/** Every `LoadSession` a game received, in order. */
let loads: Message[] = [];

/** A game host answering like a real one, `resimulating` as told; `drop` ends its connection. */
function fakeHost(name: string, options: { resimulating?: boolean } = {}) {
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [JSON.stringify({ type: "welcome", protocol_version: 3, engine_version: "0", game: info })];
    log.push(`${name}:${message.command.type}`);
    if (message.command.type === "load_session") loads.push(message.command);
    const answers: Record<string, object> = {
      export_session: { type: "session", scene: `${name} scene`, saved_scene: "saved", inputs: "AQEA", edits: "", undo: "", hashes, assets: [], branches: "", document: "Scene", keeps_assets: false },
      get_timeline: { type: "timeline", ...timeline, resimulating: options.resimulating ?? false },
    };
    return [JSON.stringify({ type: "response", id: message.id, result: { ok: answers[message.command.type] ?? { type: "done" } } })];
  });
  return transport;
}

/** A project service answering `answer(request)`; `push` sends an event. */
async function fakeProject(answer: (request: Message) => object) {
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [JSON.stringify({ type: "welcome", protocol_version: 1 })];
    log.push(`project:${message.request.type}`);
    return [JSON.stringify({ type: "response", id: message.id, result: answer(message.request) })];
  });
  const project = await ProjectClient.connect(transport);
  return { project, push: (event: object) => transport.push(JSON.stringify({ type: "event", event })) };
}

const app = () => ({ connectGame: vi.fn(), log: vi.fn() });
const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

afterEach(() => {
  log = [];
  loads = [];
  vi.restoreAllMocks();
});

describe("followWasmGames", () => {
  it("a_restore_whose_game_dies_lets_the_next_build_start", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const builds = ["/game/a/game.js", "/game/b/game.js", "/game/c/game.js"] as const;
    let build = 0;
    const { project, push } = await fakeProject(() => ({ ok: { type: "wasm", glue_url: builds[build] } }));
    const hosts = new Map<string, ReturnType<typeof fakeHost>>();
    const start = vi.fn((glueUrl: string): WorkerGame => {
      // The second build is still resimulating when the restore asks.
      const host = fakeHost(glueUrl, { resimulating: glueUrl === builds[1] });
      hosts.set(glueUrl, host);
      return { transport: host, onCrashed: (_: (crash: WorkerCrash) => void) => () => {}, terminate: () => host.close() };
    });
    followWasmGames(project, app(), start);
    await settle();
    expect(start).toHaveBeenLastCalledWith(builds[0]);

    build = 1;
    push({ type: "game_ready" });
    await settle();
    expect(start).toHaveBeenLastCalledWith(builds[1]);
    expect(log).toContain(`${builds[1]}:load_session`);
    // The rebuilt game dies while resimulating the restored session.
    hosts.get(builds[1])!.drop("the game crashed");
    await settle();

    build = 2;
    push({ type: "game_ready" });
    await settle();
    expect(start).toHaveBeenLastCalledWith(builds[2]);
  });
});

describe("followWasmGames crashes", () => {
  it("a_rebuild_that_crashes_while_restoring_keeps_the_scene_of_the_session", async () => {
    const builds = ["/game/a/game.js", "/game/b/game.js"] as const;
    let build = 0;
    const { project, push } = await fakeProject(() => ({ ok: { type: "wasm", glue_url: builds[build] } }));
    const crashes: ((crash: WorkerCrash) => void)[] = [];
    // Every game started from the second build is still resimulating when asked.
    const start = vi.fn((glueUrl: string): WorkerGame => {
      const host = fakeHost(glueUrl, { resimulating: glueUrl === builds[1] });
      return { transport: host, onCrashed: (listener: (crash: WorkerCrash) => void) => (crashes.push(listener), () => {}), terminate: () => host.close() };
    });
    const editor = app();
    followWasmGames(project, editor, start);
    await settle();
    build = 1;
    push({ type: "game_ready" });
    await settle();
    expect(loads).toHaveLength(1);
    // The new build panics in the recorded future of the session it restores.
    crashes.at(-1)!({ tick: 3, message: "boom" });
    await settle();
    expect(start).toHaveBeenCalledTimes(3);
    expect(loads[1]).toEqual({ type: "load_session", scene: `${builds[0]} scene`, saved_scene: "saved", inputs: "AQEA", edits: "", undo: "", expected: [], assets: [], branches: "", document: "Scene", keeps_assets: false });
    expect(editor.log).toHaveBeenCalledWith("error", "The game crashed at tick 3: boom");
  });
});

describe("followWasmGames crash loops", () => {
  it("a_game_that_keeps_crashing_stops_restarting_until_the_next_build", async () => {
    const builds = ["/game/a/game.js", "/game/b/game.js"] as const;
    let build = 0;
    const { project, push } = await fakeProject(() => ({ ok: { type: "wasm", glue_url: builds[build] } }));
    const crashes: ((crash: WorkerCrash) => void)[] = [];
    const terminated: string[] = [];
    const start = vi.fn((glueUrl: string): WorkerGame => {
      const host = fakeHost(glueUrl);
      const terminate = () => {
        terminated.push(glueUrl);
        host.close();
      };
      return { transport: host, onCrashed: (listener: (crash: WorkerCrash) => void) => (crashes.push(listener), () => {}), terminate };
    });
    const editor = app();
    followWasmGames(project, editor, start);
    await settle();
    expect(start).toHaveBeenCalledTimes(1);

    // The restored scene panics as soon as it loads: three restarts, then none.
    for (let crash = 0; crash < 4; crash++) {
      crashes.at(-1)!({ message: "boom" });
      await settle();
    }
    expect(start).toHaveBeenCalledTimes(4);
    expect(editor.log).toHaveBeenLastCalledWith("error", expect.stringMatching(/^The game crashed 4 times within \d+ seconds/));
    // Every crashed worker is gone, the last one included, and the editor shows no game.
    expect(terminated).toHaveLength(4);
    expect(editor.connectGame).toHaveBeenLastCalledWith(null);

    // The next build starts again, even an identical one, with a fresh allowance.
    push({ type: "game_ready" });
    await settle();
    expect(start).toHaveBeenCalledTimes(5);
    build = 1;
    push({ type: "game_ready" });
    await settle();
    expect(start).toHaveBeenLastCalledWith(builds[1]);
    crashes.at(-1)!({ message: "boom" });
    await settle();
    expect(start).toHaveBeenCalledTimes(7);
  });
});

describe("followWasmGames giving up", () => {
  it("a_game_given_up_on_leaves_no_timer_running", async () => {
    vi.useFakeTimers();
    const { project } = await fakeProject(() => ({ ok: { type: "wasm", glue_url: "/game/a/game.js" } }));
    const crashes: ((crash: WorkerCrash) => void)[] = [];
    const hosts: ReturnType<typeof fakeHost>[] = [];
    const start = (glueUrl: string): WorkerGame => {
      const host = fakeHost(glueUrl);
      hosts.push(host);
      return { transport: host, onCrashed: (listener: (crash: WorkerCrash) => void) => (crashes.push(listener), () => {}), terminate: () => {} };
    };
    followWasmGames(project, app(), start);
    await vi.advanceTimersByTimeAsync(0);
    for (let crash = 0; crash < 4; crash++) {
      // Playing when it crashes: its guard exports every second until disposed.
      hosts.at(-1)!.push(JSON.stringify({ type: "event", event: { type: "ticked", tick: 1, hash: "0000000000000001", pending: false, playing: true, effective_rate: 1 } }));
      crashes.at(-1)!({ message: "boom" });
      await vi.advanceTimersByTimeAsync(0);
    }
    // No guard's export interval or coalescing timer is left behind.
    expect(vi.getTimerCount()).toBe(0);
    vi.useRealTimers();
  });
});

describe("followNativeGames", () => {
  it("desktop_rebuild_exports_before_restart_game", async () => {
    const games = [fakeHost("old"), fakeHost("new")];
    let restarted = false;
    const { project, push } = await fakeProject((request) => {
      if (request.type === "restart_game") restarted = true;
      return { ok: { type: "done" } };
    });
    followNativeGames(project, app(), async () => games[restarted ? 1 : 0]!);
    push({ type: "game_ready" });
    await settle();
    log = [];

    push({ type: "game_rebuilt" });
    await settle();
    // The service announces the swapped game once `RestartGame` is done.
    push({ type: "game_ready" });
    await settle();
    expect(log.slice(0, 3)).toEqual(["old:export_session", "old:get_timeline", "project:restart_game"]);
    expect(log).toContain("new:load_session");
    const loaded = log.indexOf("new:load_session");
    expect(log.slice(loaded)).toEqual(["new:load_session", "new:get_timeline", "new:seek"]);
  });

  it("a_second_rebuild_before_the_swap_keeps_the_first_export", async () => {
    const games = [fakeHost("old"), fakeHost("new")];
    let restarts = 0;
    const { project, push } = await fakeProject((request) => {
      if (request.type !== "restart_game") return { ok: { type: "done" } };
      restarts += 1;
      return restarts === 1 ? { ok: { type: "done" } } : { error: { code: "no_game", message: "no rebuilt game waits to start" } };
    });
    followNativeGames(project, app(), async () => games[restarts > 0 ? 1 : 0]!);
    push({ type: "game_ready" });
    await settle();
    log = [];

    push({ type: "game_rebuilt" });
    push({ type: "game_rebuilt" });
    await settle();
    push({ type: "game_ready" });
    await settle();
    expect(log.filter((entry) => entry === "old:export_session")).toHaveLength(1);
    expect(log).toContain("new:load_session");
  });

  it("a_failed_restart_exports_again_at_the_next_rebuild", async () => {
    const games = [fakeHost("old"), fakeHost("new")];
    let restarts = 0;
    const { project, push } = await fakeProject((request) => {
      if (request.type !== "restart_game") return { ok: { type: "done" } };
      restarts += 1;
      return restarts === 1 ? { error: { code: "io", message: "cannot start the game" } } : { ok: { type: "done" } };
    });
    const editor = app();
    followNativeGames(project, editor, async () => games[restarts > 1 ? 1 : 0]!);
    push({ type: "game_ready" });
    await settle();
    log = [];

    push({ type: "game_rebuilt" });
    await settle();
    push({ type: "game_rebuilt" });
    await settle();
    push({ type: "game_ready" });
    await settle();
    expect(log.filter((entry) => entry === "old:export_session")).toHaveLength(2);
    expect(log).toContain("new:load_session");
    expect(editor.log).toHaveBeenCalledWith("error", "Cannot restart the rebuilt game: cannot start the game");
  });
});
