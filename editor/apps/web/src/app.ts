import "dockview-react/dist/styles/dockview.css";
import "../../../packages/editor-shell/src/theme.css";
import "../../../packages/editor-builtins/src/panels.css";
import type { ConsoleLevel, ControlClient, PluginBundle, ProjectClient } from "@ludenea/control-client";
import { builtinPlugins } from "@ludenea/editor-builtins";
import { textureFrames } from "@ludenea/editor-builtins/app";
import { createEditor, mountShell, type DocumentSessions, type EditorHost } from "@ludenea/editor-shell/app";

/** What the web and desktop applications share: the shell, its plugins, and the game switch. */
export interface EditorApp {
  host: EditorHost;
  /** Lists the project's plugin bundles and loads the new or changed ones. */
  loadPlugins(): Promise<void>;
  /** Makes `control` the editor's game, after its handshake; `null` leaves the editor without one. */
  connectGame(control: ControlClient | null): void;
  /** Shows a message in the Console panel, through the editor console. */
  log(level: ConsoleLevel, text: string): void;
  /** The documents' sessions, stored under `.ludenea/`. */
  sessions: DocumentSessions;
}

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * Shows the shell with the built-in plugins at once (before any build), then loads every
 * plugin bundle of the project, reloading one whenever it is rebuilt and swapping them all
 * when another project opens. Failures go to the Console panel. `pluginUrl` is where the
 * application serves a bundle.
 */
export function startEditor(project: ProjectClient, pluginUrl: (bundle: PluginBundle) => string): EditorApp {
  const host = createEditor({ project, textureFrames });
  for (const plugin of builtinPlugins) host.loader.activate(plugin);
  mountShell(document.getElementById("root")!, host);
  const log = (level: ConsoleLevel, text: string) => host.editor.console.log(level, text);

  // Bundle name -> the plugin it activated and from which URL: a plugin may name itself
  // differently from its bundle (the crate name).
  const loaded = new Map<string, { plugin: string; url: string }>();
  let queue = Promise.resolve();
  // One load at a time, so a rebuild arriving during the first listing cannot activate twice.
  const enqueue = (step: () => void | Promise<void>) => {
    queue = queue.then(step).catch((error) => log("error", `Cannot switch the editor plugins: ${errorText(error)}`));
    return queue;
  };
  const load = (bundle: PluginBundle) =>
    enqueue(async () => {
      const url = pluginUrl(bundle);
      const previous = loaded.get(bundle.name);
      if (previous?.url === url) return;
      try {
        const plugin = previous ? await host.loader.reload(previous.plugin, url) : await host.loader.load(url);
        loaded.set(bundle.name, { plugin: plugin.name, url });
      } catch (error) {
        log("error", `The editor plugin ${bundle.name} failed to load: ${errorText(error)}`);
      }
    });
  project.on("plugin_rebuilt", (bundle) => void load(bundle));
  const loadPlugins = () =>
    project.request({ type: "list_plugins" }).then(
      ({ plugins }) => Promise.all(plugins.map(load)).then(() => {}),
      (error) => log("error", `Cannot list the editor plugins: ${errorText(error)}`),
    );
  // Another project: its plugins replace those of the previous one.
  project.on("project_opened", () => {
    void enqueue(() => {
      for (const { plugin } of loaded.values()) host.loader.deactivate(plugin);
      loaded.clear();
    });
    void loadPlugins();
  });

  // Closing the editor stores the session: the guard of the running game does (`guardSession`).

  return {
    host,
    loadPlugins,
    log,
    sessions: host.sessions,
    connectGame(control) {
      host.setControl(control);
      if (!control) return;
      project.request({ type: "write_game_types", info: control.game }).catch((error) => log("error", `Cannot write the game types: ${errorText(error)}`));
    },
  };
}
