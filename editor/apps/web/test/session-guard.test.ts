import { ControlClient, fakeTransport, type GameInfo, type WorkerCrash } from "@ludenea/control-client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { guardSession, restoreSession } from "../src/session-guard";

type Command = { type: string; [key: string]: unknown };

const info: GameInfo = { components: [], resources: [], input: null, tick_rate: 60, debug_layers: [] };
const timeline = { head: 5, simulated: 5, current: 3, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, scene_dirty: false, resimulating: false };
// Six recorded hashes: ticks 0 to 5.
const hashes = ["1", "2", "3", "4", "5", "6"].map((digit) => digit.padStart(16, "0"));
const session = (scene: string) => ({ type: "session", scene, inputs: "AA==", edits: "", undo: "", hashes });

/** A game whose host answers with `answer(command)`, recording every command. */
async function fakeGame(answer: (command: Command) => object = () => ({ type: "done" })) {
  const sent: Command[] = [];
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [JSON.stringify({ type: "welcome", protocol_version: 2, engine_version: "0", game: info })];
    sent.push(message.command);
    return [JSON.stringify({ type: "response", id: message.id, result: { ok: answer(message.command) } })];
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
      void second.control.request({ type: "load_scene", ron: "bare scene" });
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
    expect(second.sent[1]).toEqual({ type: "load_session", scene: "edited", inputs: "AA==", edits: "", undo: "", expected: hashes });
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
    await vi.advanceTimersByTimeAsync(0);
    expect(exports(game.sent)).toBe(3);

    // While playing: once a second, not on every timeline change.
    game.push({ type: "ticked", tick: 4, hash: "0000000000000003", playing: true, effective_rate: 1 });
    game.push({ type: "timeline_changed", timeline: { ...timeline, head: 6 } });
    await vi.advanceTimersByTimeAsync(999);
    expect(exports(game.sent)).toBe(3);
    await vi.advanceTimersByTimeAsync(1);
    expect(exports(game.sent)).toBe(4);
    expect(guard.latest()?.load.scene).toBe("scene");

    guard.dispose();
    await vi.advanceTimersByTimeAsync(5000);
    expect(exports(game.sent)).toBe(4);
  });

  it("restore_fails_when_the_game_dies_or_panics_while_resimulating", async () => {
    const saved = { load: { type: "load_session" as const, scene: "s", inputs: "", edits: "", undo: "", expected: hashes }, tick: 4 };
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
});
