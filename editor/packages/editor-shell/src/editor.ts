import type { ControlClient, ProjectClient } from "@ludenea/control-client";
import { createAssetAccess, type AssetAccess } from "./assets";
import { createConsole, type EditorConsole } from "./console";
import { createEntityAccess, type ComponentTypes, type EntityAccess } from "./entities";
import { PluginLoader } from "./loader";
import type {
  CommandDefinition,
  Disposable,
  ExplorerColumn,
  FieldDecoration,
  InspectorComponent,
  PanelDefinition,
  SettingDefinition,
  ViewportOverlay,
} from "./plugin";
import { Registry } from "./registry";
import { normalizeShortcut } from "./shortcuts";
import { createBranchStore, type BranchStore } from "./stores/branches";
import { createDocumentStore, type DocumentSessions, type DocumentStore } from "./stores/document";
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
    decorate(decoration: FieldDecoration): Disposable;
    decorations: Observed<FieldDecoration>;
  };
  explorer: { columns: Observed<ExplorerColumn> & { register(column: ExplorerColumn): Disposable } };
  commands: {
    register(command: CommandDefinition): Disposable;
    run(id: string, ...args: unknown[]): Promise<void>;
    list(): CommandDefinition[];
  };
  /** `key` such as `Mod+S` (Cmd on macOS, Ctrl elsewhere), `Ctrl+Shift+Z`, `F5`, `Delete`, or `.`; runs `command` unless a text field has focus. */
  shortcuts: {
    register(shortcut: { key: string; command: string }): Disposable;
    /** Whether the game captures live input: then only `F5` and `Mod` combinations run shortcuts. */
    readonly captured: boolean;
    /** Captures live input until the returned disposable is disposed; captures may overlap. */
    capture(): Disposable;
  };
  overlays: { register(overlay: ViewportOverlay): Disposable; list(): ViewportOverlay[] };
  settings: {
    register<T>(setting: SettingDefinition<T>): Disposable;
    get<T>(id: string): T;
    set<T>(id: string, value: T): void;
  };
  selection: SelectionStore;
  timeline: TimelineStore;
  /** The document the game's session edits or plays, one at a time. */
  document: DocumentStore;
  branches: BranchStore;
  assets: AssetAccess;
  /** What the Console panel shows: plugins report their failures here, as the shell reports those of commands. */
  console: EditorConsole;
  onGameChanged(listener: (control: ControlClient | null) => void): Disposable;
}

/** What the application holding the editor uses beyond the `Editor` plugins see. */
export interface EditorHost {
  editor: Editor;
  registry: Registry;
  loader: PluginLoader;
  /** Switches to another game (or none): plugins hear it through `onGameChanged`. The same game again changes nothing. */
  setControl(control: ControlClient | null): void;
  /** The documents' sessions, for crash recovery, rebuilds, and close. */
  sessions: DocumentSessions;
}

/** Registrations a panel lists, re-rendering when they change. */
export interface Observed<T> {
  /** In registration order; the same array until the next change. */
  list(): T[];
  subscribe(listener: () => void): () => void;
}

const SETTINGS_PREFIX = "ludenea.settings.";

function readSetting(id: string): string | null {
  try {
    return localStorage.getItem(SETTINGS_PREFIX + id);
  } catch {
    return null;
  }
}

function writeSetting(id: string, value: unknown, report: EditorConsole["report"]): void {
  try {
    localStorage.setItem(SETTINGS_PREFIX + id, JSON.stringify(value));
  } catch (error) {
    report(error, `Saving the setting "${id}"`);
  }
}

export function createEditor(options: {
  project: ProjectClient;
  control?: ControlClient | null;
  /** Imports a plugin bundle; tests replace it. */
  importModule?: (url: string) => Promise<unknown>;
}): EditorHost {
  const registry = new Registry();
  const { commands, inspectors, overlays, settings, decorations, columns } = registry;
  const gameListeners = new Set<(control: ControlClient | null) => void>();
  const setting = (id: string) => {
    const definition = settings.get(id);
    if (!definition) throw new Error(`no setting "${id}" is registered`);
    return definition;
  };

  let control = options.control ?? null;
  let captures = 0;
  const editorConsole = createConsole();
  const timeline = createTimelineStore(() => control, editorConsole.report);
  // Its listener comes first, so a new game loads the document before plugins hear of it.
  const documents = createDocumentStore({
    project: options.project,
    control: () => control,
    onGameChanged: (listener) => gameListeners.add(listener),
    log: editorConsole.log,
    playing: () => timeline.getState().playing,
  });
  const editor: Editor = {
    get control() {
      return control;
    },
    project: options.project,
    entities: createEntityAccess(
      () => control,
      (listener) => gameListeners.add(listener),
      (dispose) => registry.own(dispose),
      editorConsole.report,
    ),
    panels: { register: (panel) => registry.panels.register(panel.id, panel) },
    inspectors: {
      register: (component, inspector) => inspectors.register(component as string, inspector as InspectorComponent<unknown>),
      get: (component) => inspectors.get(component),
      decorate: (decoration) => decorations.register(decoration.id, decoration),
      decorations: { list: decorations.values, subscribe: decorations.subscribe },
    },
    explorer: { columns: { register: (column) => columns.register(column.id, column), list: columns.values, subscribe: columns.subscribe } },
    commands: {
      register: (command) => commands.register(command.id, command),
      async run(id, ...args) {
        const command = commands.get(id);
        if (!command) throw new Error(`no command "${id}" is registered`);
        await command.run(editor, ...args);
      },
      list: () => commands.values(),
    },
    shortcuts: {
      register: ({ key, command }) => registry.shortcuts.register(normalizeShortcut(key), command),
      get captured() {
        return captures > 0;
      },
      capture() {
        captures++;
        return registry.own(() => void captures--);
      },
    },
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
        writeSetting(id, value, editorConsole.report);
      },
    },
    selection: createSelectionStore(),
    timeline,
    document: documents.store,
    branches: createBranchStore(() => control, timeline),
    assets: createAssetAccess(() => control, options.project),
    console: editorConsole,
    onGameChanged(listener) {
      // A fresh entry per call, so registering one function twice needs two disposals.
      const entry = (control: ControlClient | null) => listener(control);
      gameListeners.add(entry);
      return registry.own(() => gameListeners.delete(entry));
    },
  };

  // A divergence the game reported belongs to the document open then.
  documents.store.subscribe((state, previous) => {
    if (state.current?.id !== previous.current?.id) timeline.setState({ divergence: null });
  });
  let stopTimeline = feedTimeline(control, editor.timeline);
  const setControl = (next: ControlClient | null) => {
    if (next === control) return;
    stopTimeline();
    control = next;
    stopTimeline = feedTimeline(next, editor.timeline);
    for (const listener of [...gameListeners]) listener(next);
  };

  return { editor, registry, loader: new PluginLoader(editor, registry, options.importModule), setControl, sessions: documents.sessions };
}
