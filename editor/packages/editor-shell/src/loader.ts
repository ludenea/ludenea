import type { Editor } from "./editor";
import type { PluginDefinition } from "./plugin";
import type { Activation, Registry } from "./registry";

type Active = Activation & { plugin: PluginDefinition; cleanup?: () => void };

function pluginOf(module: unknown, url: string): PluginDefinition {
  const plugin = (module as { default?: Partial<PluginDefinition> } | null)?.default;
  if (typeof plugin?.name !== "string" || typeof plugin.activate !== "function") {
    throw new Error(`${url} does not export a plugin definition as its default export`);
  }
  return plugin as PluginDefinition;
}

/** Activates plugins on the shared `Editor` and disposes everything each one registered. */
export class PluginLoader {
  private readonly active = new Map<string, Active>();

  constructor(
    private readonly editor: Editor,
    private readonly registry: Registry,
    private readonly importModule: (url: string) => Promise<unknown> = (url) => import(/* @vite-ignore */ url),
  ) {}

  /** Activates a plugin already in memory (the built-ins); a failing `activate` leaves nothing registered. */
  activate(plugin: PluginDefinition): void {
    if (this.active.has(plugin.name)) throw new Error(`plugin "${plugin.name}" is already active`);
    const activation: Active = { name: plugin.name, plugin, disposables: [] };
    try {
      const cleanup = this.registry.during(activation, () => plugin.activate(this.editor));
      if (typeof cleanup === "function") activation.cleanup = cleanup;
    } catch (error) {
      try {
        dispose(activation);
      } catch (failure) {
        console.error(`plugin "${plugin.name}" left registrations behind:`, failure);
      }
      throw error;
    }
    this.active.set(plugin.name, activation);
  }

  /**
   * Disposes what `name` registered, then calls its cleanup. One that throws does not stop
   * the others: the plugin is inactive either way, and the failures come as an `AggregateError`.
   */
  deactivate(name: string): void {
    const activation = this.active.get(name);
    if (!activation) return;
    this.active.delete(name);
    dispose(activation);
  }

  /** Imports a bundle and activates its default export. */
  async load(url: string): Promise<PluginDefinition> {
    const plugin = pluginOf(await this.importModule(url), url);
    this.activate(plugin);
    return plugin;
  }

  /**
   * Deactivates `name` then activates the bundle at `url`. The import happens first, so the
   * swap is synchronous: the shell sees the same panel ids and keeps their place in the
   * layout. A bundle that fails to import leaves the old plugin running; one whose `activate`
   * throws gets the old plugin activated again, in the same step, before the error is rethrown.
   */
  async reload(name: string, url: string): Promise<PluginDefinition> {
    const plugin = pluginOf(await this.importModule(url), url);
    const previous = this.active.get(name)?.plugin;
    try {
      this.deactivate(name);
    } catch (failure) {
      console.error(`plugin "${name}" did not deactivate cleanly:`, failure);
    }
    try {
      this.activate(plugin);
    } catch (error) {
      if (previous) {
        try {
          this.activate(previous);
        } catch (failure) {
          console.error(`plugin "${name}" could not be restored:`, failure);
        }
      }
      throw error;
    }
    return plugin;
  }
}

function dispose(activation: Active): void {
  const failures: unknown[] = [];
  const attempt = (run: () => void) => {
    try {
      run();
    } catch (failure) {
      failures.push(failure);
    }
  };
  for (const disposable of activation.disposables.reverse()) attempt(() => disposable.dispose());
  if (activation.cleanup) attempt(activation.cleanup);
  if (failures.length > 0) throw new AggregateError(failures, `plugin "${activation.name}" did not deactivate cleanly`);
}
