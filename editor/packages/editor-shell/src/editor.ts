import type { ControlClient, ProjectClient } from "@ludenea/control-client";
import { createEntityAccess, type ComponentTypes, type EntityAccess } from "./entities";
import { PluginLoader } from "./loader";
import type {
  CommandDefinition,
  Disposable,
  InspectorComponent,
  PanelDefinition,
  SettingDefinition,
  ViewportOverlay,
} from "./plugin";
import { Registry } from "./registry";
import { normalizeShortcut } from "./shortcuts";
import { createSelectionStore, type SelectionStore } from "./stores/selection";
import { feedTimeline, createTimelineStore, type TimelineStore } from "./stores/timeline";

/** The one object every plugin, built-in or not, extends the editor through. */
export interface Editor {
  /** The running game's control client; null while no game is connected. */
  control: ControlClient | null;
  project: ProjectClient;
  entities: EntityAccess;
  panels: { register(panel: PanelDefinition): Disposable };
  inspectors: {
    register<N extends keyof ComponentTypes>(component: N, inspector: InspectorComponent<ComponentTypes[N]>): Disposable;
    get(component: string): InspectorComponent<unknown> | undefined;
  };
  commands: {
    register(command: CommandDefinition): Disposable;
    run(id: string): Promise<void>;
    list(): CommandDefinition[];
  };
  /** `key` such as `Mod+S` (Cmd on macOS, Ctrl elsewhere), `Ctrl+Shift+Z`, `F5`, `Delete`, or `.`; runs `command` unless a text field has focus. */
  shortcuts: { register(shortcut: { key: string; command: string }): Disposable };
  overlays: { register(overlay: ViewportOverlay): Disposable; list(): ViewportOverlay[] };
  settings: {
    register<T>(setting: SettingDefinition<T>): Disposable;
    get<T>(id: string): T;
    set<T>(id: string, value: T): void;
  };
  selection: SelectionStore;
  timeline: TimelineStore;
  onGameChanged(listener: (control: ControlClient | null) => void): Disposable;
}

/** What the application holding the editor uses beyond the `Editor` plugins see. */
export interface EditorHost {
  editor: Editor;
  registry: Registry;
  loader: PluginLoader;
  /** Switches to another game (or none): plugins hear it through `onGameChanged`. */
  setControl(control: ControlClient | null): void;
}

const SETTINGS_PREFIX = "ludenea.settings.";

function readSetting(id: string): string | null {
  try {
    return localStorage.getItem(SETTINGS_PREFIX + id);
  } catch {
    return null;
  }
}

function writeSetting(id: string, value: unknown): void {
  try {
    localStorage.setItem(SETTINGS_PREFIX + id, JSON.stringify(value));
  } catch (error) {
    console.warn(`setting "${id}" not saved: ${String(error)}`);
  }
}

export function createEditor(options: {
  project: ProjectClient;
  control?: ControlClient | null;
  /** Imports a plugin bundle; tests replace it. */
  importModule?: (url: string) => Promise<unknown>;
}): EditorHost {
  const registry = new Registry();
  const { commands, inspectors, overlays, settings } = registry;
  const gameListeners = new Set<(control: ControlClient | null) => void>();
  const setting = (id: string) => {
    const definition = settings.get(id);
    if (!definition) throw new Error(`no setting "${id}" is registered`);
    return definition;
  };

  let control = options.control ?? null;
  const editor: Editor = {
    get control() {
      return control;
    },
    project: options.project,
    entities: createEntityAccess(
      () => control,
      (listener) => gameListeners.add(listener),
      (dispose) => registry.own(dispose),
    ),
    panels: { register: (panel) => registry.panels.register(panel.id, panel) },
    inspectors: {
      register: (component, inspector) => inspectors.register(component as string, inspector as InspectorComponent<unknown>),
      get: (component) => inspectors.get(component),
    },
    commands: {
      register: (command) => commands.register(command.id, command),
      async run(id) {
        const command = commands.get(id);
        if (!command) throw new Error(`no command "${id}" is registered`);
        await command.run(editor);
      },
      list: () => commands.values(),
    },
    shortcuts: { register: ({ key, command }) => registry.shortcuts.register(normalizeShortcut(key), command) },
    overlays: { register: (overlay) => overlays.register(overlay.id, overlay), list: () => overlays.values() },
    settings: {
      register: (definition) => settings.register(definition.id, definition),
      get<T>(id: string): T {
        const fallback = setting(id).default as T;
        const stored = readSetting(id);
        if (stored === null) return fallback;
        try {
          return JSON.parse(stored) as T;
        } catch {
          return fallback;
        }
      },
      set(id, value) {
        setting(id);
        writeSetting(id, value);
      },
    },
    selection: createSelectionStore(),
    timeline: createTimelineStore(),
    onGameChanged(listener) {
      // A fresh entry per call, so registering one function twice needs two disposals.
      const entry = (control: ControlClient | null) => listener(control);
      gameListeners.add(entry);
      return registry.own(() => gameListeners.delete(entry));
    },
  };

  let stopTimeline = control ? feedTimeline(control, editor.timeline) : undefined;
  const setControl = (next: ControlClient | null) => {
    stopTimeline?.();
    control = next;
    stopTimeline = next ? feedTimeline(next, editor.timeline) : undefined;
    for (const listener of [...gameListeners]) listener(next);
  };

  return { editor, registry, loader: new PluginLoader(editor, registry, options.importModule), setControl };
}
