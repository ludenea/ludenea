import { ControlClient, ProjectClient, tauriTransport } from "@ludenea/control-client";
import { startEditor } from "../../web/src/app";
import { guardSession, restoreSession, type SessionGuard } from "../../web/src/session-guard";

// Where the backend's `ludenea-plugin` scheme answers (Windows webviews reach custom schemes
// over http).
const PLUGIN_ORIGIN = navigator.userAgent.includes("Windows") ? "http://ludenea-plugin.localhost" : "ludenea-plugin://localhost";

type Running = { control: ControlClient; guard: SessionGuard };

async function main() {
  const project = await ProjectClient.connect(await tauriTransport("project"));
  const app = startEditor(project, (bundle) => `${PLUGIN_ORIGIN}/${bundle.url_path}`);
  // A project given on the command line is open already; one opened later brings its plugins
  // with its first game.
  project.request({ type: "info" }).then(() => app.loadPlugins(), () => {});

  let running: Running | null = null;
  /**
   * Each `GameReady` is a new game process (first build or rebuild) with a fresh control
   * channel. The previous process is gone by then: its latest export carries the session over.
   */
  const connect = async () => {
    const saved = running?.guard.latest() ?? null;
    running?.guard.dispose();
    running?.control.close();
    running = null;
    const control = await ControlClient.connect(await tauriTransport("control"));
    // The asset browser loads the open scene into the new game here, before any restore.
    app.connectGame(control);
    // The native host recovers from panics itself, so there is no crash to restart from.
    running = { control, guard: guardSession(control, { onCrashed: () => () => {} }, async () => control) };
    if (saved) await restoreSession(control, saved);
    await app.loadPlugins();
  };
  let updates = Promise.resolve();
  project.on("game_ready", () => {
    updates = updates.then(connect).catch((error) => console.error("cannot connect to the game:", error));
  });
}

main().catch((error) => {
  console.error("the editor cannot start:", error);
  document.body.textContent = `The editor cannot start: ${String(error instanceof Error ? error.message : error)}`;
});
