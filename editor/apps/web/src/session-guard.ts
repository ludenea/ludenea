import type { Command, ControlClient, WorkerGame } from "@ludenea/control-client";
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

export async function exportSession(control: ControlClient): Promise<SavedSession> {
  const [{ scene, inputs, edits, undo, hashes }, timeline] = await Promise.all([
    control.request({ type: "export_session" }),
    control.request({ type: "get_timeline" }),
  ]);
  return { load: { type: "load_session", scene, inputs, edits, undo, expected: hashes }, tick: timeline.current };
}

/** Resolves once the game has simulated `tick`; a seek further than the resimulation fails. */
function simulated(control: ControlClient, tick: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const stop = control.on("resimulated", ({ up_to }) => {
      if (up_to < tick) return;
      stop();
      resolve();
    });
    control.request({ type: "get_timeline" }).then((timeline) => {
      if (timeline.resimulating && timeline.simulated < tick) return;
      stop();
      resolve();
    }, (error) => {
      stop();
      reject(error);
    });
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
  const target = Math.min(tick, saved.load.expected.length - 1);
  await simulated(control, target);
  await control.request({ type: "seek", tick: target });
}

/**
 * Keeps the latest export of the session (on every pause, after every edit once resimulated,
 * and every second while playing). When the worker crashes, restarts it, loads that export,
 * and seeks back to the crash tick when known (else the exported tick).
 */
export function guardSession(
  control: ControlClient,
  game: Pick<WorkerGame, "onCrashed">,
  restart: () => Promise<ControlClient>,
): SessionGuard {
  let latest: SavedSession | null = null;
  let playing = false;
  let disposed = false;
  const take = () => {
    exportSession(control).then(
      (saved) => {
        if (!disposed) latest = saved;
      },
      // Nothing loaded yet, or the game is gone: the previous export stays.
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
      console.error(`the game crashed${crash.tick === undefined ? "" : ` at tick ${crash.tick}`}: ${crash.message}`);
      const saved = latest;
      dispose();
      try {
        const next = await restart();
        if (saved) await restoreSession(next, saved, crash.tick ?? saved.tick);
      } catch (error) {
        console.error("cannot restore the session after the crash:", error);
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
  take();
  return { latest: () => latest, dispose };
}
