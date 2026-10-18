import type { ControlClient, ProjectInfo } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useSyncExternalStore } from "react";

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
  /**
   * The scene the asset browser loaded into the game, which `scene.save` writes back, and
   * its text as last loaded or saved (a disk change to that same text is the editor's own).
   */
  scene: Store<{ path: string | null; text: string | null }>;
  /** Asks the asset browser to show `path` (from a console location). */
  reveal: Store<{ path: string | null }>;
}

const sharedState = new WeakMap<Editor, Shared>();

export function shared(editor: Editor): Shared {
  let state = sharedState.get(editor);
  if (!state) {
    state = {
      project: createStore({ info: null as ProjectInfo | null, build: { status: "idle", done: 0, total: null } as BuildState }),
      scene: createStore({ path: null as string | null, text: null as string | null }),
      reveal: createStore({ path: null as string | null }),
    };
    sharedState.set(editor, state);
  }
  return state;
}

/** Brings a panel forward through the shell's `panels.open.<id>` command, when a shell provides it. */
export function showPanel(editor: Editor, id: string): void {
  const command = `panels.open.${id}`;
  if (!editor.commands.list().some((registered) => registered.id === command)) return;
  editor.commands.run(command).catch((error) => console.error(`command "${command}" failed:`, error));
}

/** A failure as one line for a panel to show. */
export const errorText = (error: unknown): string => (error instanceof Error ? error.message : String(error));
