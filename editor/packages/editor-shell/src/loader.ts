import type { Editor } from "./editor";
import type { PluginDefinition } from "./plugin";
import type { Activation, Registry } from "./registry";

type Active = Activation & { cleanup?: () => void };

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
    const activation: Active = { name: plugin.name, disposables: [] };
    try {
      const cleanup = this.registry.during(activation, () => plugin.activate(this.editor));
      if (typeof cleanup === "function") activation.cleanup = cleanup;
    } catch (error) {
      dispose(activation);
      throw error;
    }
    this.active.set(plugin.name, activation);
  }

  deactivate(name: string): void {
    const activation = this.active.get(name);
    if (!activation) return;
    this.active.delete(name);
    dispose(activation);
    activation.cleanup?.();
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
   * layout, and a bundle that fails to import leaves the old plugin running.
   */
  async reload(name: string, url: string): Promise<PluginDefinition> {
    const plugin = pluginOf(await this.importModule(url), url);
    this.deactivate(name);
    this.activate(plugin);
    return plugin;
  }
}

function dispose(activation: Activation): void {
  for (const disposable of activation.disposables.reverse()) disposable.dispose();
}
