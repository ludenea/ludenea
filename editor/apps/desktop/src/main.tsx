import { invoke } from "@tauri-apps/api/core";
import { ProjectClient, tauriTransport } from "@ludenea/control-client";
import { startEditor, type EditorApp } from "@ludenea/web/app";
import { measureFrameTimes } from "@ludenea/web/frame-times";
import { followNativeGames } from "@ludenea/web/games";

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
  // Optional: failing to ask whether a measurement was requested never stops a started editor.
  invoke<boolean>("frame_times_requested").then(
    (requested) => {
      if (requested) void reportFrameTimes(app);
    },
    (error: unknown) => console.error(`cannot tell whether a frame time measurement was requested: ${errorText(error)}`),
  );
}

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** How long a frame time measurement plays. */
const MEASURED_PLAY_MS = 10_000;

/** Measures the frame times, as requested at launch, and hands the backend the report, which exits. */
async function reportFrameTimes(app: EditorApp): Promise<void> {
  const report = await measureFrameTimes(app.host.editor, MEASURED_PLAY_MS).then(
    (times) => times,
    (error: unknown) => ({ error: errorText(error) }),
  );
  // Unsent, the editor keeps running until the measuring command times out; the cause stays in the webview console.
  await invoke("frame_times_measured", { report: JSON.stringify(report) }).catch((error: unknown) => console.error(`cannot hand the frame times to the backend: ${errorText(error)}`));
}

main().catch((error) => {
  console.error("the editor cannot start:", error);
  document.body.textContent = `The editor cannot start: ${String(error instanceof Error ? error.message : error)}`;
});
