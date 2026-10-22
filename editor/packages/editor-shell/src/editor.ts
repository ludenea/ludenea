import type { Command, ControlClient, ProjectClient, ReplyFor } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";
import { createAssetAccess, type AssetAccess } from "./assets";
import { createConsole, type EditorConsole } from "./console";
import { createEntityAccess, type ComponentTypes, type EntityAccess } from "./entities";
import { PluginLoader } from "./loader";
import type {
  AssetInspector,
  CommandDefinition,
  Disposable,
  ExplorerColumn,
  FieldDecoration,
  InspectorComponent,
  PanelDefinition,
  SettingDefinition,
  ViewportOverlay,
  ViewportTool,
} from "./plugin";
import { Registry } from "./registry";
import { normalizeShortcut } from "./shortcuts";
import { createBranchStore, type BranchStore } from "./stores/branches";
import { createDocumentStore, type DocumentSessions, type DocumentStore } from "./stores/document";
import { createSelectionStore, type SelectionStore } from "./stores/selection";
import { feedTimeline, createTimelineStore, type TimelineStore } from "./stores/timeline";
import { createTextureAccess, type TextureAccess, type TextureFrameCutter } from "./textures";

/**
 * A command that edits the game's session: one undo step, which the timeline records. A
 * `batch` applies its commands as one edit, or none of them when one fails.
 */
export type EditCommand = Extract<
  Command,
  {
    type:
      | "spawn"
      | "despawn"
      | "insert_component"
      | "remove_component"
      | "set_field"
      | "paint_tiles"
      | "batch"
      | "apply_to_scene"
      | "set_asset_field"
      | "apply_to_asset"
      | "place_instance"
      | "replace_with_instance"
      | "apply_override"
      | "revert_override"
      | "apply_to_prefab"
      | "undo"
      | "redo";
  }
>;

type BatchCommand = Extract<Command, { type: "batch" }>;

/** The active viewport tool's id, null for the built-in select and move: a Zustand store read as a hook or with `getState`/`subscribe`; `editor.viewport.setTool` changes it. */
export type ActiveToolStore = UseBoundStore<Pick<StoreApi<string | null>, "getState" | "getInitialState" | "subscribe">>;

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
    /** Adds an inspector for the data assets of the type `name` (below their generic fields), or for textures as `"Texture"`. */
    registerAsset(name: string, inspector: AssetInspector): Disposable;
    getAsset(name: string): AssetInspector | undefined;
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
  viewport: {
    tools: Observed<ViewportTool> & { register(tool: ViewportTool): Disposable };
    activeTool: ActiveToolStore;
    /** Activates the tool `id`, or the built-in select and move for null; throws when it is not registered or not available. */
    setTool(id: string | null): void;
  };
  /**
   * Sends an editing command to the running game, then refreshes the timeline and hash it
   * changed; resolves with the game's reply (a `spawn` gives the entity it made). Throws
   * without a game and while the game resimulates; the game's refusal rejects.
   */
  edit<C extends EditCommand>(command: C): Promise<ReplyFor<C>>;
  /**
   * Sends `commands` as one edit (one undo step, all of them or none): one alone as itself,
   * with its own reply, several as a `batch`. Empty, it sends nothing and resolves with the
   * reply an empty batch gets.
   */
  batched<C extends EditCommand>(commands: C[]): Promise<ReplyFor<C> | ReplyFor<BatchCommand>>;
  /** Calls `listener` with each command `edit` and `batched` send, once the game accepted it. */
  onEdited(listener: (command: EditCommand) => void): Disposable;
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
  /** The project's textures, their PNGs, and the frames their metas cut them into; no game needed. */
  textures: TextureAccess;
  /** What the Console panel shows: plugins report their failures here, as the shell reports those of commands. */
  console: EditorConsole;
  onGameChanged(listener: (control: ControlClient | null) => void): Disposable;
  /**
   * The name of the plugin that registered `registration` (a panel, command, inspector, asset
   * inspector, overlay, viewport tool, setting, decoration, or column, as given to `register`),
   * `"editor"` for one registered outside an `activate`; undefined when it is not registered.
   */
  pluginOf(registration: object): string | undefined;
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
  /** Cuts a texture into frames for `editor.textures.frames`: the renderer's; without it, `frames` rejects saying so. */
  textureFrames?: TextureFrameCutter;
}): EditorHost {
  const registry = new Registry();
  const { commands, inspectors, assetInspectors, overlays, tools, settings, decorations, columns } = registry;
  const gameListeners = new Set<(control: ControlClient | null) => void>();
  const editListeners = new Set<(command: EditCommand) => void>();
  const activeTool = create<string | null>()(() => null);
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
  const edit = async <C extends EditCommand>(command: C): Promise<ReplyFor<C>> => {
    const game = control;
    if (!game) throw new Error("no game is running");
    if (timeline.getState().timeline?.resimulating) throw new Error("the game is resimulating; edit once it finishes");
    try {
      const reply = await game.request(command);
      // The game took the edit: a listener that throws costs only its own reaction.
      for (const listener of [...editListeners]) {
        try {
          listener(command);
        } catch (error) {
          editorConsole.report(error, `Reacting to the edit "${command.type}"`);
        }
      }
      return reply;
    } finally {
      void timeline.getState().refresh();
    }
  };
  /** Whether `tool` can be used now; one that throws cannot, and its failure is reported. */
  const available = (tool: ViewportTool) => {
    try {
      return tool.available(editor);
    } catch (error) {
      editorConsole.report(error, `Asking whether the viewport tool "${tool.id}" is available`);
      return false;
    }
  };
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
      registerAsset: (name, inspector) => assetInspectors.register(name, inspector),
      getAsset: (name) => assetInspectors.get(name),
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
    viewport: {
      tools: { register: (tool) => tools.register(tool.id, tool), list: tools.values, subscribe: tools.subscribe },
      activeTool,
      setTool(id) {
        if (id !== null) {
          const tool = tools.get(id);
          if (!tool) throw new Error(`no viewport tool "${id}" is registered`);
          if (!available(tool)) throw new Error(`the viewport tool "${id}" is not available now`);
        }
        activeTool.setState(id, true);
      },
    },
    edit,
    batched(commands) {
      const [only] = commands;
      if (!only) return Promise.resolve({ type: "done" });
      return commands.length === 1 ? edit(only) : edit({ type: "batch", commands });
    },
    onEdited(listener) {
      // A fresh entry per call, as `onGameChanged`.
      const entry = (command: EditCommand) => listener(command);
      editListeners.add(entry);
      return registry.own(() => editListeners.delete(entry));
    },
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
    textures: createTextureAccess(options.project, editorConsole.report, options.textureFrames),
    console: editorConsole,
    onGameChanged(listener) {
      // A fresh entry per call, so registering one function twice needs two disposals.
      const entry = (control: ControlClient | null) => listener(control);
      gameListeners.add(entry);
      return registry.own(() => gameListeners.delete(entry));
    },
    pluginOf: (registration) => registry.ownerOf(registration),
  };

  // A divergence the game reported belongs to the document open then.
  documents.store.subscribe((state, previous) => {
    if (state.current?.id !== previous.current?.id) timeline.setState({ divergence: null });
  });
  // The active tool stays while registered and available: asked again on every change that may change the answer.
  const checkTool = () => {
    const id = activeTool.getState();
    if (id === null) return;
    const tool = tools.get(id);
    if (!tool || !available(tool)) activeTool.setState(null, true);
  };
  tools.subscribe(checkTool);
  editor.selection.subscribe(checkTool);
  timeline.subscribe(checkTool);
  gameListeners.add(checkTool);
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
