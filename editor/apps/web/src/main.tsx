import { ControlClient, ProjectClient, startWorkerGame, webSocketTransport, type WorkerGame } from "@ludenea/control-client";
import workerUrl from "@ludenea/control-client/src/worker/host-worker.ts?worker&url";
import { startEditor } from "./app";
import { exportSession, guardSession, restoreSession, type SessionGuard } from "./session-guard";

type Running = { glueUrl: string; game: WorkerGame; control: ControlClient; guard: SessionGuard };

async function main() {
  const project = await ProjectClient.connect(await webSocketTransport("/ws/project"));
  const app = startEditor(project, (bundle) => `/${bundle.url_path}`);
  void app.loadPlugins();

  let running: Running | null = null;
  /** Starts the build at `glueUrl` in a new worker, replacing the running game. */
  const launch = async (glueUrl: string): Promise<ControlClient> => {
    running?.guard.dispose();
    running?.game.terminate();
    running = null;
    const game = startWorkerGame({ workerUrl: new URL(workerUrl, location.href), glueUrl: new URL(glueUrl, location.href).href });
    const control = await ControlClient.connect(game.transport);
    // The asset browser loads the open scene into the new game here, before any restore.
    app.connectGame(control);
    const guard = guardSession(control, game, () => launch(glueUrl));
    running = { glueUrl, game, control, guard };
    return control;
  };

  /** Runs the latest build; a rebuild carries the session over and returns to its tick. */
  const update = async () => {
    const endpoint = await project.request({ type: "game_endpoint" }).catch(() => null);
    if (endpoint?.type !== "wasm" || endpoint.glue_url === running?.glueUrl) return;
    const previous = running;
    const saved = previous && (await exportSession(previous.control).catch(() => previous.guard.latest()));
    const control = await launch(endpoint.glue_url);
    if (saved) await restoreSession(control, saved);
  };
  let updates = Promise.resolve();
  const queueUpdate = () => {
    updates = updates.then(update).catch((error) => console.error("cannot start the game:", error));
  };
  project.on("game_ready", queueUpdate);
  queueUpdate();
}

main().catch((error) => {
  console.error("the editor cannot start:", error);
  document.body.textContent = `The editor cannot reach ludenea serve: ${String(error instanceof Error ? error.message : error)}`;
});
