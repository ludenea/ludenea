import { ControlClient, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, fakeTransport, type GameInfo, type WorkerCrash } from "@ludenea/control-client";
import { createEditor } from "@ludenea/editor-shell/app";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EXPORT_COALESCE_MS, exportSession, guardSession, restoreSession, type SavedSession } from "../src/session-guard";
import { documentProject, sessionFile, storedBytes } from "./support";

type Command = { type: string; [key: string]: unknown };

const info: GameInfo = { components: [], resources: [], assets: [], events: [], fingerprint: "0000000000000000", input: null, tick_rate: 60, debug_layers: [] };
const timeline = { head: 5, simulated: 5, current: 3, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, scene_dirty: false, resimulating: false };
// Six recorded hashes: ticks 0 to 5.
const hashes = ["1", "2", "3", "4", "5", "6"].map((digit) => digit.padStart(16, "0"));
const session = (scene: string) => ({ type: "session", scene, saved_scene: null, inputs: "AA==", edits: "", undo: "", hashes, assets: [], saved_assets: [], branches: "", document: "Scene", keeps_assets: false });

/** A game whose host answers with `answer(command)` (`null` fails, as does `{ error }`), recording every command. */
async function fakeGame(answer: (command: Command) => object | null = () => ({ type: "done" })) {
  const sent: Command[] = [];
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [JSON.stringify({ type: "welcome", protocol_version: PROTOCOL_VERSION, protocol_fingerprint: PROTOCOL_FINGERPRINT, engine_version: "0", game: info })];
    sent.push(message.command);
    const reply = answer(message.command);
    const result = reply === null ? { error: { code: "resimulating", message: "still resimulating" } } : "error" in reply ? reply : { ok: reply };
    return [JSON.stringify({ type: "response", id: message.id, result })];
  });
  const control = await ControlClient.connect(transport);
  return { control, sent, push: (event: object) => transport.push(JSON.stringify({ type: "event", event })), drop: transport.drop };
}

/** A worker the test crashes by hand. */
function fakeWorker() {
  const listeners = new Set<(crash: WorkerCrash) => void>();
  return {
    onCrashed(listener: (crash: WorkerCrash) => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    crash: (crash: WorkerCrash) => listeners.forEach((listener) => listener(crash)),
  };
}

const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0));
const exports = (sent: Command[]) => sent.filter((command) => command.type === "export_session").length;

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("guardSession", () => {
  it("crash_restarts_worker_and_loads_latest_export", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    let scene = "first";
    const first = await fakeGame((command) => (command.type === "export_session" ? session(scene) : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const worker = fakeWorker();
    // The second game is still resimulating the loaded session when asked.
    const second = await fakeGame((command) =>
      command.type === "get_timeline" ? { type: "timeline", ...timeline, simulated: 0, resimulating: true } : { type: "done" },
    );
    const restart = vi.fn(async () => {
      // As the asset browser does for every new game.
      void second.control.request({ type: "load_scene", scene: "bare scene", assets: [], document: "Scene", keeps_assets: false });
      return second.control;
    });
    guardSession(first.control, worker, restart);
    await settle();
    scene = "edited";
    first.push({ type: "paused", reason: { type: "user" } });
    await settle();

    worker.crash({ tick: 4, message: "boom" });
    await settle();
    expect(restart).toHaveBeenCalledTimes(1);
    expect(errors.mock.calls.flat().join(" ")).toContain("boom");
    // The seek waits for the resimulation to reach the crash tick.
    expect(second.sent.map((command) => command.type)).toEqual(["load_scene", "load_session", "get_timeline"]);
    expect(second.sent[1]).toEqual({ type: "load_session", scene: "edited", saved_scene: null, inputs: "AA==", edits: "", undo: "", expected: hashes, assets: [], saved_assets: [], branches: "", document: "Scene", keeps_assets: false });
    second.push({ type: "resimulated", up_to: 2 });
    await settle();
    expect(second.sent.at(-1)?.type).toBe("get_timeline");
    second.push({ type: "resimulated", up_to: 5 });
    await settle();
    expect(second.sent.at(-1)).toEqual({ type: "seek", tick: 4 });
  });

  it("seeks_to_the_exported_tick_when_the_crash_has_none", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const first = await fakeGame((command) => (command.type === "export_session" ? session("scene") : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const worker = fakeWorker();
    const second = await fakeGame((command) => (command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    guardSession(first.control, worker, async () => second.control);
    await settle();
    worker.crash({ message: "boom" });
    await settle();
    expect(second.sent.map((command) => command.type)).toEqual(["load_session", "get_timeline", "seek"]);
    expect(second.sent.at(-1)).toEqual({ type: "seek", tick: 3 });
  });

  it("export_taken_on_pause_and_after_edits", async () => {
    vi.useFakeTimers();
    const game = await fakeGame((command) => (command.type === "export_session" ? session("scene") : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const guard = guardSession(game.control, fakeWorker(), async () => game.control);
    await vi.advanceTimersByTimeAsync(0);
    expect(exports(game.sent)).toBe(1);

    game.push({ type: "paused", reason: { type: "user" } });
    await vi.advanceTimersByTimeAsync(0);
    expect(exports(game.sent)).toBe(2);
    // An edit changes the timeline; the export waits for its resimulation.
    game.push({ type: "timeline_changed", timeline: { ...timeline, resimulating: true } });
    await vi.advanceTimersByTimeAsync(0);
    expect(exports(game.sent)).toBe(2);
    game.push({ type: "timeline_changed", timeline: { ...timeline, edit_ticks: [3] } });
    await vi.advanceTimersByTimeAsync(EXPORT_COALESCE_MS);
    expect(exports(game.sent)).toBe(3);

    // While playing: once a second, not on every timeline change.
    game.push({ type: "ticked", tick: 4, hash: "0000000000000003", pending: false, playing: true, effective_rate: 1 });
    game.push({ type: "timeline_changed", timeline: { ...timeline, head: 6 } });
    // The interval started with the guard, `EXPORT_COALESCE_MS` ago.
    await vi.advanceTimersByTimeAsync(999 - EXPORT_COALESCE_MS);
    expect(exports(game.sent)).toBe(3);
    await vi.advanceTimersByTimeAsync(1);
    expect(exports(game.sent)).toBe(4);
    expect(guard.latest()?.load.scene).toBe("scene");

    guard.dispose();
    await vi.advanceTimersByTimeAsync(5000);
    expect(exports(game.sent)).toBe(4);
  });

  it("stores_the_session_on_pause_and_edits_but_not_while_playing", async () => {
    vi.useFakeTimers();
    const game = await fakeGame((command) => (command.type === "export_session" ? session("scene") : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const store = vi.fn(async () => {});
    const sessions = { export: exportSession, store, own: async () => null, restore: restoreSession };
    guardSession(game.control, fakeWorker(), async () => game.control, { sessions });
    await vi.advanceTimersByTimeAsync(0);
    expect(store).toHaveBeenCalledTimes(1);

    // Playing: the export taken every second stays in memory, for a crash.
    game.push({ type: "ticked", tick: 4, hash: "0000000000000003", pending: false, playing: true, effective_rate: 1 });
    await vi.advanceTimersByTimeAsync(3000);
    expect(exports(game.sent)).toBe(4);
    expect(store).toHaveBeenCalledTimes(1);

    game.push({ type: "paused", reason: { type: "user" } });
    await vi.advanceTimersByTimeAsync(0);
    expect(store).toHaveBeenCalledTimes(2);
    game.push({ type: "timeline_changed", timeline: { ...timeline, edit_ticks: [3] } });
    await vi.advanceTimersByTimeAsync(EXPORT_COALESCE_MS);
    expect(store).toHaveBeenCalledTimes(3);
  });

  it("timeline_changes_while_paused_coalesce_into_one_export", async () => {
    vi.useFakeTimers();
    const game = await fakeGame((command) => (command.type === "export_session" ? session("scene") : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const guard = guardSession(game.control, fakeWorker(), async () => game.control);
    await vi.advanceTimersByTimeAsync(0);
    expect(exports(game.sent)).toBe(1);

    // Scrubbing the timeline while paused: one export once the changes stop.
    for (let tick = 0; tick < 5; tick++) {
      game.push({ type: "timeline_changed", timeline: { ...timeline, current: tick } });
      await vi.advanceTimersByTimeAsync(EXPORT_COALESCE_MS / 2);
    }
    expect(exports(game.sent)).toBe(1);
    await vi.advanceTimersByTimeAsync(EXPORT_COALESCE_MS / 2);
    expect(exports(game.sent)).toBe(2);

    // A pause exports at once, taking the pending export with it.
    game.push({ type: "timeline_changed", timeline: { ...timeline, current: 1 } });
    game.push({ type: "paused", reason: { type: "user" } });
    await vi.advanceTimersByTimeAsync(0);
    expect(exports(game.sent)).toBe(3);
    await vi.advanceTimersByTimeAsync(EXPORT_COALESCE_MS);
    expect(exports(game.sent)).toBe(3);

    game.push({ type: "timeline_changed", timeline: { ...timeline, current: 2 } });
    guard.dispose();
    await vi.advanceTimersByTimeAsync(EXPORT_COALESCE_MS);
    expect(exports(game.sent)).toBe(3);
  });

  it("a_continuous_burst_of_timeline_changes_still_exports_every_second", async () => {
    vi.useFakeTimers();
    const game = await fakeGame((command) => (command.type === "export_session" ? session("scene") : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    guardSession(game.control, fakeWorker(), async () => game.control);
    await vi.advanceTimersByTimeAsync(0);
    expect(exports(game.sent)).toBe(1);

    // Changes never stop for `EXPORT_COALESCE_MS`: the first one waits a second at most.
    const step = EXPORT_COALESCE_MS / 2;
    for (let elapsed = 0; elapsed < 1000 - step; elapsed += step) {
      game.push({ type: "timeline_changed", timeline: { ...timeline, current: elapsed % 5 } });
      await vi.advanceTimersByTimeAsync(step);
    }
    expect(exports(game.sent)).toBe(1);
    game.push({ type: "timeline_changed", timeline: { ...timeline, current: 1 } });
    await vi.advanceTimersByTimeAsync(step);
    expect(exports(game.sent)).toBe(2);
  });

  it("restore_fails_when_the_game_dies_or_panics_while_resimulating", async () => {
    const saved = { load: { type: "load_session" as const, scene: "s", saved_scene: null, inputs: "", edits: "", undo: "", expected: hashes, assets: [], saved_assets: [], branches: "", document: "Scene" as const, keeps_assets: false }, tick: 4 };
    const resimulating = (command: Command) => (command.type === "get_timeline" ? { type: "timeline", ...timeline, simulated: 0, resimulating: true } : { type: "done" });
    const dying = await fakeGame(resimulating);
    const died = restoreSession(dying.control, saved);
    await settle();
    dying.drop("the game crashed");
    await expect(died).rejects.toThrow("the game stopped while resimulating");

    const panicking = await fakeGame(resimulating);
    const panicked = restoreSession(panicking.control, saved);
    await settle();
    panicking.push({ type: "paused", reason: { type: "panicked", tick: 2, message: "boom" } });
    await expect(panicked).rejects.toThrow("boom");
  });

  it("a_crash_while_restoring_reloads_only_the_restored_scene", async () => {
    vi.useFakeTimers();
    const seed = {
      // Version 1, two players, one tick.
      load: { type: "load_session" as const, scene: "edited", saved_scene: "saved", inputs: "AQIBAQc=", edits: "AQ==", undo: "Ag==", expected: hashes, assets: [], saved_assets: [], branches: "", document: "Scene" as const, keeps_assets: false },
      tick: 4,
    };
    // Still resimulating the restored session: every export fails.
    const first = await fakeGame((command) => (command.type === "export_session" ? null : { type: "done" }));
    const worker = fakeWorker();
    const second = await fakeGame((command) => (command.type === "get_timeline" ? { type: "timeline", ...timeline, current: 0 } : { type: "done" }));
    const restart = vi.fn(async () => second.control);
    const logged: string[] = [];
    guardSession(first.control, worker, restart, { seed, log: (level, text) => logged.push(`${level}: ${text}`) });
    first.push({ type: "timeline_changed", timeline: { ...timeline, resimulating: false } });
    await vi.advanceTimersByTimeAsync(EXPORT_COALESCE_MS);
    expect(exports(first.sent)).toBe(1);

    worker.crash({ tick: 4, message: "boom" });
    await vi.advanceTimersByTimeAsync(0);
    // The input log keeps its version and two players, without ticks.
    const sceneOnly = { type: "load_session", scene: "edited", saved_scene: "saved", inputs: "AQIA", edits: "", undo: "", expected: [], assets: [], saved_assets: [], branches: "", document: "Scene", keeps_assets: false };
    expect(restart).toHaveBeenCalledWith({ load: sceneOnly, tick: 0 });
    expect(second.sent).toEqual([sceneOnly, { type: "get_timeline" }, { type: "seek", tick: 0 }]);
    expect(logged[0]).toBe("error: The game crashed at tick 4: boom");
    expect(logged[1]).toMatch(/^warning: .*only its scene/);
  });

  it("a_seeded_guard_restores_a_newer_export_in_full", async () => {
    const seed = { load: { type: "load_session" as const, scene: "old", saved_scene: null, inputs: "", edits: "", undo: "", expected: hashes, assets: [], saved_assets: [], branches: "", document: "Scene" as const, keeps_assets: false }, tick: 2 };
    const first = await fakeGame((command) => (command.type === "export_session" ? session("newer") : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const worker = fakeWorker();
    const second = await fakeGame((command) => (command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const guard = guardSession(first.control, worker, async () => second.control, { seed, log: () => {} });
    expect(guard.latest()).toBe(seed);
    first.push({ type: "paused", reason: { type: "user" } });
    await settle();
    expect(guard.latest()?.load.scene).toBe("newer");
    worker.crash({ tick: 4, message: "boom" });
    await settle();
    expect(second.sent.map((command) => command.type)).toEqual(["load_session", "get_timeline", "seek"]);
    expect(second.sent[0]).toMatchObject({ scene: "newer", inputs: "AA==" });
    expect(second.sent.at(-1)).toEqual({ type: "seek", tick: 4 });
  });
});

describe("guardSession with documents", () => {
  it("closing_the_page_stores_the_latest_export_at_once", async () => {
    // The page's events, as a browser fires them.
    const page = new EventTarget();
    vi.stubGlobal("addEventListener", page.addEventListener.bind(page));
    vi.stubGlobal("removeEventListener", page.removeEventListener.bind(page));
    let answering = true;
    const game = await fakeGame((command) =>
      command.type === "export_session" ? (answering ? session("paused") : null) : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" },
    );
    const store = vi.fn(async (_?: SavedSession) => {});
    const sessions = { export: exportSession, store, own: async () => null, restore: restoreSession };
    const guard = guardSession(game.control, fakeWorker(), async () => game.control, { sessions, log: () => {} });
    await settle();
    store.mockClear();
    // A page closing mid-play rarely lives through a fresh export's round trips.
    answering = false;

    page.dispatchEvent(new Event("pagehide"));

    expect(store).toHaveBeenCalledWith(guard.latest());
    expect(store.mock.calls[0]![0]?.load.scene).toBe("paused");
    guard.dispose();
    store.mockClear();
    page.dispatchEvent(new Event("pagehide"));
    expect(store).not.toHaveBeenCalled();
  });

  it("a_crash_restore_keeps_the_unsaved_assets", async () => {
    const asset = "3".padStart(32, "0");
    const unsaved = { ...session("edited"), assets: [[asset, "(edited)"]], saved_assets: [[asset, "(on disk)"]] };
    const first = await fakeGame((command) => (command.type === "export_session" ? unsaved : command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const worker = fakeWorker();
    const second = await fakeGame((command) => (command.type === "get_timeline" ? { type: "timeline", ...timeline } : { type: "done" }));
    const sessions = { export: exportSession, store: async () => {}, own: async (saved: SavedSession | null) => saved, restore: restoreSession };
    guardSession(first.control, worker, async () => second.control, { sessions, log: () => {} });
    await settle();

    worker.crash({ tick: 4, message: "boom" });
    await settle();

    expect(second.sent.find((command) => command.type === "load_session")).toMatchObject({ assets: [[asset, "(edited)"]], saved_assets: [[asset, "(on disk)"]] });
  });

  /**
   * A game whose session is the scene it loaded last. One that `exited` exports nothing; one
   * that `refuses` refuses every stored session.
   */
  const documentGame = (options: { exited?: boolean; refuses?: boolean } = {}) => {
    let scene = "";
    return fakeGame((command) => {
      if (command.type === "load_scene") scene = command.scene as string;
      if (command.type === "load_session" && options.refuses) return { error: { code: "invalid_command", message: "the component demo::Gone is not registered" } };
      if (command.type === "export_session") return options.exited ? null : session(scene);
      if (command.type === "get_timeline") return { type: "timeline", ...timeline, branch: 0, branches: [], compare: null };
      return { type: "done" };
    });
  };

  it("a_crash_after_a_document_switch_restores_only_the_open_document", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { project, ids, requests, sessions } = await documentProject();
    const [a, b] = ids;
    const host = createEditor({ project });
    const first = await documentGame();
    host.setControl(first.control);
    await host.editor.document.getState().open(a);
    const worker = fakeWorker();
    const second = await documentGame();
    const restart = vi.fn(async () => {
      host.setControl(second.control);
      return second.control;
    });
    guardSession(first.control, worker, restart, { sessions: host.sessions });
    await settle();
    expect(exports(first.sent)).toBe(1);

    // The guard's latest export is still A's when the game crashes on B.
    await host.editor.document.getState().open(b);
    worker.crash({ tick: 2, message: "boom" });
    await settle();
    await settle();

    expect(restart).toHaveBeenCalledWith(null);
    expect(second.sent.filter((command) => command.type === "load_session")).toEqual([]);
    expect(second.sent.find((command) => command.type === "load_scene")).toMatchObject({ scene: "(B)" });
    const stored = sessions.get(sessionFile(b));
    expect(stored === undefined ? "(B)" : JSON.parse(atob(stored)).saved.load.scene).toBe("(B)");
    expect(requests.filter((request) => request.type === "write_text")).toEqual([]);
  });

  it("a_stored_session_the_restarted_game_refuses_is_set_aside_before_the_next_store", async () => {
    const { project, ids, requests, sessions } = await documentProject();
    const [a] = ids;
    const bytes = storedBytes({
      load: { type: "load_session", scene: "(A edited)", saved_scene: "(A)", inputs: "AA==", edits: "", undo: "", expected: hashes, assets: [], saved_assets: [], branches: "", document: "Scene", keeps_assets: false },
      tick: 2,
      document: a,
    });
    sessions.set(sessionFile(a), bytes);
    const host = createEditor({ project });
    // The game exits before it exports anything: the crash restores A's stored session.
    const first = await documentGame({ exited: true });
    host.setControl(first.control);
    await host.editor.document.getState().open(a);
    const worker = fakeWorker();
    // It restarts refusing that session, as a game rebuilt without one of its components does.
    const second = await documentGame({ refuses: true });
    const restart = async () => {
      host.setControl(second.control);
      return second.control;
    };
    const logged: string[] = [];
    guardSession(first.control, worker, restart, { sessions: host.sessions, log: (level, text) => logged.push(`${level}: ${text}`) });
    await settle();

    worker.crash({ tick: 2, message: "boom" });
    await vi.waitFor(() => expect(requests.some((request) => request.type === "set_aside_stored_session")).toBe(true));
    await host.sessions.store();

    expect(second.sent.find((command) => command.type === "load_session")).toMatchObject({ scene: "(A edited)" });
    const kept = `.ludenea/sessions/${a}.1.rejected.session`;
    expect(sessions.get(kept)).toBe(bytes);
    expect(requests.map((request) => request.type).filter((type) => type === "set_aside_stored_session" || type === "store_session")).toEqual(["set_aside_stored_session", "store_session"]);
    expect(host.editor.console.list().map(({ level, text }) => `${level}: ${text}`)).toContainEqual(expect.stringMatching(`^warning: The game refused the stored session of scenes/a.scene.ron .*kept as ${kept}`));
    expect(logged).toEqual(["error: The game crashed at tick 2: boom"]);
  });
});
