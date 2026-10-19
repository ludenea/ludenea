import { ControlClient, type MessageTransport, type ProjectClient, type WorkerGame } from "@ludenea/control-client";
import type { EditorApp } from "./app";
import { guardSession, type SavedSession, type SessionGuard } from "./session-guard";

type App = Pick<EditorApp, "connectGame" | "log" | "sessions">;

/** Runs steps one after the other; a failing step is reported and never blocks the next. */
function sequence(app: App): (what: string, step: () => Promise<void>) => void {
  let queue = Promise.resolve();
  return (what, step) => {
    queue = queue.then(step).catch((error) => app.log("error", `Cannot ${what}: ${errorText(error)}`));
  };
}

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * Restores off the queue: a restore waiting for resimulation never holds up the next game.
 * Through the document sessions, which set aside a stored session the game refuses.
 */
function restoreInBackground(app: App, control: ControlClient, saved: SavedSession): void {
  app.sessions.restore(control, saved).catch((error: unknown) => app.log("error", `Cannot restore the session: ${errorText(error)}`));
}

/** Crashes closer together than this count as one loop. */
const CRASH_WINDOW_MS = 30_000;
/** Restarts allowed within `CRASH_WINDOW_MS`; one more crash stops the game. */
const MAX_CRASH_RESTARTS = 3;

/**
 * Browser: runs each WebAssembly build announced by `GameReady` in a worker from `start`. A
 * rebuild carries the session over (export from the running game, `LoadSession` into the new
 * one, back to its tick); a crash restarts the same build the same way (`guardSession`). A
 * build that keeps crashing (a scene that panics on load) stays stopped until the next build.
 */
export function followWasmGames(project: ProjectClient, app: App, start: (glueUrl: string) => WorkerGame): void {
  type Running = { glueUrl: string; game: WorkerGame; control: ControlClient; guard: SessionGuard };
  let running: Running | null = null;
  let crashes: number[] = [];
  let stopped = false;
  const relaunch = async (glueUrl: string, saved: SavedSession | null): Promise<ControlClient | null> => {
    const now = Date.now();
    crashes = [...crashes.filter((time) => now - time < CRASH_WINDOW_MS), now];
    if (crashes.length > MAX_CRASH_RESTARTS) {
      stopped = true;
      // The crashed worker and its client are dead: the editor shows no game until a build starts.
      running?.game.terminate();
      running?.control.close();
      app.connectGame(null);
      app.log("error", `The game crashed ${crashes.length} times within ${CRASH_WINDOW_MS / 1000} seconds: it restarts with the next build.`);
      return null;
    }
    return launch(glueUrl, saved);
  };
  // `seed` is the session about to be restored into the new game.
  const launch = async (glueUrl: string, seed: SavedSession | null): Promise<ControlClient> => {
    running?.guard.dispose();
    running?.game.terminate();
    running = null;
    const game = start(glueUrl);
    const control = await ControlClient.connect(game.transport);
    // The document store loads the open document into the new game here, before any restore.
    app.connectGame(control);
    const guard = guardSession(control, game, (saved) => relaunch(glueUrl, saved), { seed, log: app.log, sessions: app.sessions });
    running = { glueUrl, game, control, guard };
    return control;
  };
  const update = async () => {
    const endpoint = await project.request({ type: "game_endpoint" }).catch(() => null);
    if (endpoint?.type !== "wasm" || (endpoint.glue_url === running?.glueUrl && !stopped)) return;
    crashes = [];
    stopped = false;
    const previous = running;
    // The last export may predate a document switch: only the open document's session carries over.
    const saved = previous && (await app.sessions.own(await app.sessions.export(previous.control).catch(() => previous.guard.latest())));
    const control = await launch(endpoint.glue_url, saved);
    if (saved) restoreInBackground(app, control, saved);
  };
  const enqueue = sequence(app);
  project.on("game_ready", () => enqueue("start the game", update));
  enqueue("start the game", update);
}

/**
 * Desktop: connects to each native game process `GameReady` announces, over the fresh
 * control transport `connect` opens. On `GameRebuilt` the old game still runs: its session
 * is exported, then `RestartGame` swaps the processes, and the new game loads it, losing
 * nothing. Only when the old process already exited (the native host recovers from panics, so
 * an abort or a kill) does the export fail: the new game then loads the guard's last export,
 * up to one second of play old.
 */
export function followNativeGames(project: ProjectClient, app: App, connect: () => Promise<MessageTransport>): void {
  let running: { control: ControlClient; guard: SessionGuard } | null = null;
  let pending: SavedSession | null = null;
  const enqueue = sequence(app);
  project.on("game_rebuilt", () =>
    enqueue("restart the rebuilt game", async () => {
      const current = running;
      // One export per swap: a second rebuild before the new game is ready keeps the first,
      // taken while the old game still ran.
      const exporting = pending === null;
      if (current && exporting) pending = await app.sessions.export(current.control).catch(() => current.guard.latest());
      try {
        await project.request({ type: "restart_game" });
      } catch (error) {
        // The old game still runs: the next rebuild exports it afresh.
        if (exporting) pending = null;
        throw error;
      }
    }),
  );
  project.on("game_ready", () =>
    enqueue("connect to the game", async () => {
      // The last export may predate a document switch: only the open document's session carries over.
      const saved = await app.sessions.own(pending);
      pending = null;
      running?.guard.dispose();
      running?.control.close();
      running = null;
      const control = await ControlClient.connect(await connect());
      // The document store loads the open document into the new game here, before any restore.
      app.connectGame(control);
      // The native host recovers from panics itself, so there is no crash to restart from.
      const guard = guardSession(control, { onCrashed: () => () => {} }, async () => control, { seed: saved, log: app.log, sessions: app.sessions });
      running = { control, guard };
      if (saved) restoreInBackground(app, control, saved);
    }),
  );
}
