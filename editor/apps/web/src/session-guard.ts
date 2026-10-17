import type { Command, ConsoleLevel, ControlClient, WorkerGame } from "@ludenea/control-client";
import type { Disposable } from "@ludenea/editor-shell";

const EXPORT_INTERVAL_MS = 1000;

/** A session as `ExportSession` gave it, ready to load, and the tick the timeline showed then. */
export interface SavedSession {
  load: Extract<Command, { type: "load_session" }>;
  tick: number;
}

export interface SessionGuard extends Disposable {
  /** The last export taken, if any. */
  latest(): SavedSession | null;
}

/** Shows a message in the editor's Console panel. */
export type Log = (level: ConsoleLevel, text: string) => void;

const devtools: Log = (level, text) => (level === "error" ? console.error(text) : console.warn(text));

export async function exportSession(control: ControlClient): Promise<SavedSession> {
  const [{ scene, saved_scene, inputs, edits, undo, hashes }, timeline] = await Promise.all([
    control.request({ type: "export_session" }),
    control.request({ type: "get_timeline" }),
  ]);
  return { load: { type: "load_session", scene, saved_scene, inputs, edits, undo, expected: hashes }, tick: timeline.current };
}

/** `saved` without its recorded future: its scene (unsaved edits included) at tick 0. */
function sceneOf(saved: SavedSession): SavedSession {
  return { load: { ...saved.load, inputs: "", edits: "", undo: "", expected: [] }, tick: 0 };
}

/** How long a restore may wait for resimulation before it gives up. */
const RESIMULATION_TIMEOUT_MS = 5 * 60 * 1000;

/**
 * Resolves once the game has simulated `tick` (a seek further than the resimulation fails);
 * rejects if the game goes away, panics, or takes longer than `RESIMULATION_TIMEOUT_MS`.
 */
function simulated(control: ControlClient, tick: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const stops: (() => void)[] = [];
    const settle = (error?: Error) => {
      stops.forEach((stop) => stop());
      clearTimeout(timer);
      if (error) reject(error);
      else resolve();
    };
    const timer = setTimeout(() => settle(new Error(`the game did not resimulate to tick ${tick} in time`)), RESIMULATION_TIMEOUT_MS);
    stops.push(
      control.on("resimulated", ({ up_to }) => {
        if (up_to >= tick) settle();
      }),
      control.on("paused", ({ reason }) => {
        if (reason.type === "panicked") settle(new Error(`the game panicked at tick ${reason.tick} while resimulating: ${reason.message}`));
      }),
      control.onClose((reason) => settle(new Error(`the game stopped while resimulating: ${reason}`))),
    );
    control.request({ type: "get_timeline" }).then((timeline) => {
      if (!timeline.resimulating || timeline.simulated >= tick) settle();
    }, settle);
  });
}

/**
 * Loads `saved` into a new game and goes back to `tick` (the saved one by default), within
 * what the session recorded. A game whose code changed reports `Diverged` while it resimulates.
 * Callers send this after the game's `LoadScene` (which the asset browser sends to every new
 * game), so the restored session is what stays loaded.
 */
export async function restoreSession(control: ControlClient, saved: SavedSession, tick = saved.tick): Promise<void> {
  await control.request(saved.load);
  const target = Math.max(0, Math.min(tick, saved.load.expected.length - 1));
  await simulated(control, target);
  await control.request({ type: "seek", tick: target });
}

/**
 * Keeps the latest export of the session (on every pause, after every edit once resimulated,
 * and every second while playing). When the worker crashes, reports it in the console,
 * restarts it, loads that export, and seeks back to the crash tick when known (else the
 * exported tick). `seed` is the session being restored into this game: it stays the latest
 * until a newer export exists, and a crash before then (the restored session crashes this
 * build) reloads only its scene, so the game does not crash again at once.
 */
export function guardSession(
  control: ControlClient,
  game: Pick<WorkerGame, "onCrashed">,
  restart: (saved: SavedSession | null) => Promise<ControlClient>,
  options: { seed?: SavedSession | null; log?: Log } = {},
): SessionGuard {
  const log = options.log ?? devtools;
  let latest: SavedSession | null = options.seed ?? null;
  let restoring = latest !== null;
  let playing = false;
  let disposed = false;
  const take = () => {
    exportSession(control).then(
      (saved) => {
        if (disposed) return;
        latest = saved;
        restoring = false;
      },
      // Nothing loaded yet, still resimulating, or the game is gone: the previous export stays.
      () => {},
    );
  };
  const stops = [
    control.on("ticked", (event) => {
      playing = event.playing;
    }),
    control.on("paused", () => {
      playing = false;
      take();
    }),
    control.on("timeline_changed", ({ timeline }) => {
      if (!playing && !timeline.resimulating) take();
    }),
    game.onCrashed(async (crash) => {
      log("error", `The game crashed${crash.tick === undefined ? "" : ` at tick ${crash.tick}`}: ${crash.message}`);
      let saved = latest;
      let tick = crash.tick ?? saved?.tick;
      if (saved && restoring) {
        saved = sceneOf(saved);
        tick = 0;
        log("warning", "The game crashed while restoring the session: only its scene was reloaded, without its recorded inputs and edits.");
      }
      dispose();
      try {
        const next = await restart(saved);
        if (saved) await restoreSession(next, saved, tick);
      } catch (error) {
        log("error", `Cannot restore the session after the crash: ${error instanceof Error ? error.message : String(error)}`);
      }
    }),
  ];
  const timer = setInterval(() => {
    if (playing) take();
  }, EXPORT_INTERVAL_MS);
  const dispose = () => {
    disposed = true;
    clearInterval(timer);
    stops.forEach((stop) => stop());
  };
  if (!restoring) take();
  return { latest: () => latest, dispose };
}
