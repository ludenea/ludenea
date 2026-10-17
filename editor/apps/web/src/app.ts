import "dockview-react/dist/styles/dockview.css";
import type { ControlClient, PluginBundle, ProjectClient } from "@ludenea/control-client";
import { builtinPlugins } from "@ludenea/editor-builtins";
import { createEditor, mountShell, type EditorHost } from "@ludenea/editor-shell";

/** What the web and desktop applications share: the shell, its plugins, and the game switch. */
export interface EditorApp {
  host: EditorHost;
  /** Lists the project's plugin bundles and loads the new or changed ones. */
  loadPlugins(): Promise<void>;
  /** Makes `control` the editor's game, after its handshake. */
  connectGame(control: ControlClient): void;
}

/**
 * Shows the shell with the built-in plugins at once (before any build), then loads every
 * plugin bundle of the project, reloading one whenever it is rebuilt. `pluginUrl` is where the
 * application serves a bundle.
 */
export function startEditor(project: ProjectClient, pluginUrl: (bundle: PluginBundle) => string): EditorApp {
  const host = createEditor({ project });
  for (const plugin of builtinPlugins) host.loader.activate(plugin);
  mountShell(document.getElementById("root")!, host);

  // Bundle name -> the plugin it activated and from which URL: a plugin may name itself
  // differently from its bundle (the crate name).
  const loaded = new Map<string, { plugin: string; url: string }>();
  let queue = Promise.resolve();
  // One load at a time, so a rebuild arriving during the first listing cannot activate twice.
  const load = (bundle: PluginBundle) => {
    queue = queue.then(async () => {
      const url = pluginUrl(bundle);
      const previous = loaded.get(bundle.name);
      if (previous?.url === url) return;
      try {
        const plugin = previous ? await host.loader.reload(previous.plugin, url) : await host.loader.load(url);
        loaded.set(bundle.name, { plugin: plugin.name, url });
      } catch (error) {
        console.error(`the editor plugin ${bundle.name} failed to load:`, error);
      }
    });
    return queue;
  };
  project.on("plugin_rebuilt", (bundle) => void load(bundle));
  const loadPlugins = () =>
    project.request({ type: "list_plugins" }).then(
      ({ plugins }) => Promise.all(plugins.map(load)).then(() => {}),
      (error) => console.error("cannot list the editor plugins:", error),
    );

  return {
    host,
    loadPlugins,
    connectGame(control) {
      host.setControl(control);
      project.request({ type: "write_game_types", info: control.game }).catch((error) => console.error("cannot write the game types:", error));
    },
  };
}
