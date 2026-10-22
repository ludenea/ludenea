import type { AssetId, ControlClient, ProjectInfo, TextureMeta } from "@ludenea/control-client";
import { runReported, type Editor, type TextureFrames } from "@ludenea/editor-shell";
import { useLayoutEffect, useRef, useSyncExternalStore, type UIEvent } from "react";
import { instantiate, type RenderModule } from "./render-module";

let renderModule: Promise<RenderModule> | undefined;
const panicListeners = new Set<(message: string) => void>();

/**
 * Loads the renderer's WebAssembly module once for every built-in: loaded twice, it would be two
 * instances. A load that fails is tried again on the next call. A panic aborts the module: the
 * listeners of `onRenderPanic` hear its message, and the next call loads a new instance (the
 * aborted one stays in memory until the page reloads: see `instantiate`).
 */
export function loadRenderModule(): Promise<RenderModule> {
  if (renderModule) return renderModule;
  const loading: Promise<RenderModule> = instantiate().then(
    (module) => {
      module.setPanicHandler((message: string) => {
        if (renderModule === loading) renderModule = undefined;
        for (const listener of [...panicListeners]) listener(message);
      });
      return module;
    },
    (error: unknown) => {
      if (renderModule === loading) renderModule = undefined;
      throw error;
    },
  );
  renderModule = loading;
  return loading;
}

/**
 * What `meta` cuts `png` into, by the renderer's module (`textureFrames`), loaded anew after a
 * panic; rejects with its message. What the applications give `createEditor` for `editor.textures.frames`.
 */
export async function textureFrames(png: Uint8Array, meta: TextureMeta): Promise<TextureFrames> {
  const module = await loadRenderModule();
  return JSON.parse(module.textureFrames(png, JSON.stringify(meta))) as TextureFrames;
}

/** Calls `listener` with the message of each panic of the renderer's module; returns how to stop. */
export function onRenderPanic(listener: (message: string) => void): () => void {
  panicListeners.add(listener);
  return () => void panicListeners.delete(listener);
}

/** The project folders the asset browser lists. */
export const ASSET_FOLDERS = ["assets", "scenes"];

/** A minimal observable value, for state a plugin's commands and panel share. */
export interface Store<T> {
  get(): T;
  set(patch: Partial<T>): void;
  subscribe(listener: () => void): () => void;
}

export function createStore<T extends object>(initial: T): Store<T> {
  let state = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    set(patch) {
      state = { ...state, ...patch };
      for (const listener of [...listeners]) listener();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export const useStore = <T>(store: Store<T>): T => useSyncExternalStore(store.subscribe, store.get);

/** A scroll position a plugin keeps across the shell unmounting its hidden panel. */
export type Scroll = { top: number };

/**
 * Keeps the scroll position of the element given `ref` in `scroll`, and restores it once on
 * mount, as soon as `ready` (the content is tall enough: a browser clamps it otherwise).
 */
export function useKeptScroll<E extends HTMLElement>(scroll: Scroll, ready = true) {
  const ref = useRef<E>(null);
  const restored = useRef(false);
  useLayoutEffect(() => {
    if (restored.current || !ready || !ref.current) return;
    restored.current = true;
    ref.current.scrollTop = scroll.top;
  }, [scroll, ready]);
  const onScroll = (event: UIEvent<E>) => {
    scroll.top = event.currentTarget.scrollTop;
  };
  return { ref, onScroll };
}

/** The connected game, re-rendering when it changes. */
export function useGame(editor: Editor): ControlClient | null {
  return useSyncExternalStore(
    (listener) => {
      const subscription = editor.onGameChanged(listener);
      return () => subscription.dispose();
    },
    () => editor.control,
  );
}

export type BuildState = { status: "idle" | "building" | "succeeded" | "failed"; done: number; total: number | null };

/** What the built-in plugins of one editor share with each other. */
export interface Shared {
  project: Store<{ info: ProjectInfo | null; build: BuildState }>;
  /** Asks the asset browser to show `path` (from a console location). */
  reveal: Store<{ path: string | null }>;
  /** Changed by a built-in viewport overlay with something new to draw: the viewport repaints. */
  overlays: Store<{ version: number }>;
  /** The debug layers the viewport hides, by name. */
  layers: Store<{ hidden: string[] }>;
  /** The text the editor last wrote to each project file, by path: the watcher reports it as a change. */
  written: Map<string, string>;
}

const sharedState = new WeakMap<Editor, Shared>();

export function shared(editor: Editor): Shared {
  let state = sharedState.get(editor);
  if (!state) {
    state = {
      project: createStore({ info: null as ProjectInfo | null, build: { status: "idle", done: 0, total: null } as BuildState }),
      reveal: createStore({ path: null as string | null }),
      overlays: createStore({ version: 0 }),
      layers: createStore({ hidden: [] as string[] }),
      written: new Map(),
    };
    sharedState.set(editor, state);
  }
  return state;
}

/** Brings a panel forward through the shell's `panels.open.<id>` command, when a shell provides it; a failure goes to the editor console. */
export function showPanel(editor: Editor, id: string): void {
  const command = `panels.open.${id}`;
  if (!editor.commands.list().some((registered) => registered.id === command)) return;
  void runReported(editor, command);
}

/** Reports the failure of work repeated every frame or tick once, until it works again. */
export function reportedOnce(editor: Editor, context: string) {
  let failing = false;
  return {
    failed(error: unknown) {
      if (!failing) editor.console.report(error, context);
      failing = true;
    },
    worked: () => void (failing = false),
  };
}

/** A failure as one line for a panel to show. */
export const errorText = (error: unknown): string => (error instanceof Error ? error.message : String(error));

/** The prefab and data assets whose session text differs from their file; none in a replay, whose assets are its own. */
export function unsavedAssets(editor: Editor): AssetId[] {
  if (editor.document.getState().current?.kind === "replay") return [];
  return editor.timeline.getState().timeline?.unsaved_assets ?? [];
}

/** Whether the asset `id` is among `unsavedAssets`, following the game. */
export function useUnsavedAsset(editor: Editor, id: AssetId): boolean {
  const unsaved = editor.timeline((state) => state.timeline?.unsaved_assets.includes(id) ?? false);
  const replay = editor.document((state) => state.current?.kind === "replay");
  return unsaved && !replay;
}
