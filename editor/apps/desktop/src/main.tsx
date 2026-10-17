import { ProjectClient, tauriTransport } from "@ludenea/control-client";
import { startEditor } from "../../web/src/app";
import { followNativeGames } from "../../web/src/games";

// Where the backend's `ludenea-plugin` scheme answers (Windows webviews reach custom schemes
// over http).
const PLUGIN_ORIGIN = navigator.userAgent.includes("Windows") ? "http://ludenea-plugin.localhost" : "ludenea-plugin://localhost";

async function main() {
  const project = await ProjectClient.connect(await tauriTransport("project"));
  const app = startEditor(project, (bundle) => `${PLUGIN_ORIGIN}/${bundle.url_path}`);
  // A project given on the command line is open already; one opened later announces
  // `ProjectOpened`, which loads its plugins. The backend repeats `GameReady` after the hello
  // when a game runs.
  project.request({ type: "info" }).then(() => app.loadPlugins(), () => {});
  followNativeGames(project, app, () => tauriTransport("control"));
}

main().catch((error) => {
  console.error("the editor cannot start:", error);
  document.body.textContent = `The editor cannot start: ${String(error instanceof Error ? error.message : error)}`;
});
