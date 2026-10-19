import type { AssetId, ControlClient, ProjectInfo } from "@ludenea/control-client";
import { runReported, type Editor } from "@ludenea/editor-shell";
import { useLayoutEffect, useRef, useSyncExternalStore, type UIEvent } from "react";

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
