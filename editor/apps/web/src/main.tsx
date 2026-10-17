import { ProjectClient, startWorkerGame, webSocketTransport } from "@ludenea/control-client";
import workerUrl from "@ludenea/control-client/src/worker/host-worker.ts?worker&url";
import { startEditor } from "./app";
import { followWasmGames } from "./games";

async function main() {
  const project = await ProjectClient.connect(await webSocketTransport("/ws/project"));
  const app = startEditor(project, (bundle) => `/${bundle.url_path}`);
  void app.loadPlugins();
  followWasmGames(project, app, (glueUrl) =>
    startWorkerGame({ workerUrl: new URL(workerUrl, location.href), glueUrl: new URL(glueUrl, location.href).href }),
  );
}

main().catch((error) => {
  console.error("the editor cannot start:", error);
  document.body.textContent = `The editor cannot reach ludenea serve: ${String(error instanceof Error ? error.message : error)}`;
});
