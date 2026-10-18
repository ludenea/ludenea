import { ControlClient, type MessageTransport, type ProjectClient, type WorkerGame } from "@ludenea/control-client";
import type { EditorApp } from "./app";
import { exportSession, guardSession, restoreSession, type SavedSession, type SessionGuard } from "./session-guard";

type App = Pick<EditorApp, "connectGame" | "log">;

/** Runs steps one after the other; a failing step is reported and never blocks the next. */
function sequence(app: App): (what: string, step: () => Promise<void>) => void {
  let queue = Promise.resolve();
  return (what, step) => {
    queue = queue.then(step).catch((error) => app.log("error", `Cannot ${what}: ${errorText(error)}`));
  };
}

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Restores off the queue: a restore waiting for resimulation never holds up the next game. */
function restoreInBackground(app: App, control: ControlClient, saved: SavedSession): void {
  restoreSession(control, saved).catch((error) => app.log("error", `Cannot restore the session: ${errorText(error)}`));
}

/**
 * Browser: runs each WebAssembly build announced by `GameReady` in a worker from `start`. A
 * rebuild carries the session over (export from the running game, `LoadSession` into the new
 * one, back to its tick); a crash restarts the same build the same way (`guardSession`).
 */
export function followWasmGames(project: ProjectClient, app: App, start: (glueUrl: string) => WorkerGame): void {
  type Running = { glueUrl: string; game: WorkerGame; control: ControlClient; guard: SessionGuard };
  let running: Running | null = null;
  // `seed` is the session about to be restored into the new game.
  const launch = async (glueUrl: string, seed: SavedSession | null): Promise<ControlClient> => {
    running?.guard.dispose();
    running?.game.terminate();
    running = null;
    const game = start(glueUrl);
    const control = await ControlClient.connect(game.transport);
    // The asset browser loads the open scene into the new game here, before any restore.
    app.connectGame(control);
    const guard = guardSession(control, game, (saved) => launch(glueUrl, saved), { seed, log: app.log });
    running = { glueUrl, game, control, guard };
    return control;
  };
  const update = async () => {
    const endpoint = await project.request({ type: "game_endpoint" }).catch(() => null);
    if (endpoint?.type !== "wasm" || endpoint.glue_url === running?.glueUrl) return;
    const previous = running;
    const saved = previous && (await exportSession(previous.control).catch(() => previous.guard.latest()));
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
 * is exported, then `RestartGame` swaps the processes, and the new game loads it.
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
      if (current && exporting) pending = await exportSession(current.control).catch(() => current.guard.latest());
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
      const saved = pending;
      pending = null;
      running?.guard.dispose();
      running?.control.close();
      running = null;
      const control = await ControlClient.connect(await connect());
      // The asset browser loads the open scene into the new game here, before any restore.
      app.connectGame(control);
      // The native host recovers from panics itself, so there is no crash to restart from.
      const guard = guardSession(control, { onCrashed: () => () => {} }, async () => control, { seed: saved, log: app.log });
      running = { control, guard };
      if (saved) restoreInBackground(app, control, saved);
    }),
  );
}
