import type { ConsoleLevel, ControlClient, WorkerGame } from "@ludenea/control-client";
import type { Disposable } from "@ludenea/editor-shell";
import { exportSession, restoreSession, type DocumentSessions, type SavedSession } from "@ludenea/editor-shell/app";

export { exportSession, restoreSession, type SavedSession };

const EXPORT_INTERVAL_MS = 1000;
/**
 * Timeline changes while paused (scrubbing, edits) closer together than this export once,
 * at most `EXPORT_INTERVAL_MS` after the first of them.
 */
export const EXPORT_COALESCE_MS = 200;

export interface SessionGuard extends Disposable {
  /** The last export taken, if any. */
  latest(): SavedSession | null;
}

/** Shows a message in the editor's Console panel. */
export type Log = (level: ConsoleLevel, text: string) => void;

const devtools: Log = (level, text) => (level === "error" ? console.error(text) : console.warn(text));

/** `saved` without its recorded future: its scene (unsaved edits included) at tick 0. */
function sceneOf(saved: SavedSession): SavedSession {
  return { ...saved, load: { ...saved.load, inputs: withoutTicks(saved.load.inputs), edits: "", undo: "", branches: "", expected: [] }, tick: 0 };
}

/**
 * An input log (base64 postcard bytes: version, player count, ticks) without its ticks, so
 * a fresh game keeps the session's player count. An empty log stays empty. The layout is
 * `InputLog` in `crates/ludenea-app/src/input_log.rs`, pinned by its test
 * `input_log_prefix_is_version_then_players`.
 */
function withoutTicks(inputs: string): string {
  if (inputs === "") return "";
  const bytes = Uint8Array.from(atob(inputs), (char) => char.charCodeAt(0));
  let end = 0;
  // Two varints: the version, then the player count.
  for (let varint = 0; varint < 2; varint++) {
    while ((bytes[end++] ?? 0) & 0x80);
  }
  return btoa(String.fromCharCode(...bytes.subarray(0, end), 0));
}

/**
 * Keeps the latest export of the session (on every pause, after every edit once resimulated,
 * and every second while playing). Timeline changes while paused export once they stop for
 * `EXPORT_COALESCE_MS`, or a second after the first of them if they do not stop, so a crash
 * in that window loses at most the last second of edits. When the worker crashes, reports it
 * in the console, restarts it, loads that export, and seeks back to the crash tick when known
 * (else the exported tick). `seed` is the session being restored into this game: it stays the
 * latest until a newer export exists, and a crash before then (the restored session crashes
 * this build) reloads only its scene, so the game does not crash again at once. `restart`
 * resolves to `null` when it gives up restarting. With `sessions`, exports are tagged with
 * their document and stored as its session (except those taken while playing, kept in memory), and a crash restores only a session of the open
 * document: an export taken before switching documents gives way to the open one's stored session.
 */
export function guardSession(
  control: ControlClient,
  game: Pick<WorkerGame, "onCrashed">,
  restart: (saved: SavedSession | null) => Promise<ControlClient | null>,
  options: { seed?: SavedSession | null; log?: Log; sessions?: DocumentSessions } = {},
): SessionGuard {
  const log = options.log ?? devtools;
  let latest: SavedSession | null = options.seed ?? null;
  let restoring = latest !== null;
  let playing = false;
  let disposed = false;
  let coalescing: ReturnType<typeof setTimeout> | undefined;
  // The longest a burst of timeline changes waits for its export.
  let deadline: ReturnType<typeof setTimeout> | undefined;
  // While playing, the export every second stays in memory: storing a large session that
  // often stalls the interface. Pauses and edits store it.
  const take = (store = true) => {
    clearTimeout(coalescing);
    clearTimeout(deadline);
    deadline = undefined;
    (options.sessions?.export ?? exportSession)(control).then(
      (saved) => {
        if (disposed) return;
        latest = saved;
        restoring = false;
        if (store) options.sessions?.store(saved).catch((error) => log("warning", `Cannot store the session: ${error instanceof Error ? error.message : String(error)}`));
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
      if (playing || timeline.resimulating) return;
      clearTimeout(coalescing);
      coalescing = setTimeout(take, EXPORT_COALESCE_MS);
      deadline ??= setTimeout(take, EXPORT_INTERVAL_MS);
    }),
    game.onCrashed(async (crash) => {
      log("error", `The game crashed${crash.tick === undefined ? "" : ` at tick ${crash.tick}`}: ${crash.message}`);
      dispose();
      try {
        let saved = latest;
        let tick = crash.tick ?? saved?.tick;
        const own = options.sessions ? await options.sessions.own(saved) : saved;
        if (own !== saved) {
          saved = own;
          tick = own?.tick;
        } else if (saved && restoring) {
          saved = sceneOf(saved);
          tick = 0;
          log("warning", "The game crashed while restoring the session: only its scene was reloaded, without its recorded inputs and edits.");
        }
        const next = await restart(saved);
        if (next && saved) await (options.sessions?.restore ?? restoreSession)(next, saved, tick);
      } catch (error) {
        log("error", `Cannot restore the session after the crash: ${error instanceof Error ? error.message : String(error)}`);
      }
    }),
  ];
  const timer = setInterval(() => {
    if (playing) take(false);
  }, EXPORT_INTERVAL_MS);
  const dispose = () => {
    disposed = true;
    clearInterval(timer);
    clearTimeout(coalescing);
    clearTimeout(deadline);
    stops.forEach((stop) => stop());
  };
  if (!restoring) take();
  return { latest: () => latest, dispose };
}
