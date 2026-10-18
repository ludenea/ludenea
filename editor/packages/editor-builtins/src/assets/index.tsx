import type { FileChange, FileEntry } from "@ludenea/control-client";
import { AssetFailure, definePlugin, type Editor } from "@ludenea/editor-shell";
import { logToConsole } from "../console";
import { createStore, errorText, shared, type Scroll, type Store } from "../shared";
import { AssetsPanel, baseName } from "./AssetsPanel";

export type Action = { kind: "rename" | "move" | "delete"; path: string };

export interface AssetsState {
  files: FileEntry[];
  filter: string;
  selected: string | null;
  /** The inline form (rename, move) or confirmation (delete) the panel shows. */
  action: Action | null;
  menu: string | null;
  /** The open document changed on disk while it had unsaved edits. */
  banner: { path: string; text: string } | null;
  error: string | null;
  /** Why the files could not be listed. */
  listError: string | null;
}

export interface Assets {
  state: Store<AssetsState>;
  /** Kept here, not in the panel: the shell unmounts a panel while its tab is hidden. */
  scroll: Scroll;
  open(path: string): Promise<void>;
  rename(from: string, to: string): Promise<void>;
  move(path: string, folder: string): Promise<void>;
  remove(path: string): Promise<void>;
  reload(): Promise<void>;
  keepMine(): void;
}

const changedPath = (change: FileChange) => ("Changed" in change ? change.Changed.path : "Created" in change ? change.Created.path : null);
const removedPath = (change: FileChange) => ("Removed" in change ? change.Removed.path : null);

export const assetsPlugin = definePlugin({
  name: "ludenea.assets",
  activate(editor) {
    const { reveal, project } = shared(editor);
    const documents = editor.document;
    const state = createStore<AssetsState>({
      files: [],
      filter: "",
      selected: null,
      action: null,
      menu: null,
      banner: null,
      error: null,
      listError: null,
    });
    const report = (error: unknown) => state.set({ error: errorText(error) });
    const refresh = () =>
      editor.project.request({ type: "list_files" }).then(
        ({ entries }) => state.set({ files: entries, listError: null }),
        (error) => state.set({ files: [], listError: errorText(error) }),
      );
    // A document that fails to load also goes to the console, located in the asset at fault.
    const logged = <T,>(work: Promise<T>) =>
      work.catch((error: unknown) => {
        const location = error instanceof AssetFailure ? { file: error.path, line: error.line, column: error.column } : undefined;
        logToConsole(editor, "error", errorText(error), location);
        throw error;
      });
    // Without a game the new text is only remembered: the next game loads it.
    const replace = (text: string) => logged(documents.getState().replace(text));

    // The watcher may report a rename's removal before the service answers it.
    const renaming = new Set<string>();
    const assets: Assets = {
      state,
      scroll: { top: 0 },
      async open(path) {
        state.set({ selected: path });
        const entry = state.get().files.find((file) => file.path === path);
        if (entry?.kind !== "Scene" && entry?.kind !== "Prefab" && entry?.kind !== "Replay") throw new Error(`${path} is not a scene, a prefab, or a replay`);
        // Documents are usable before the build: the document is remembered and loaded into each game.
        await logged(documents.getState().open(entry.id));
        state.set({ banner: null, error: null });
      },
      async rename(from, to) {
        state.set({ action: null });
        renaming.add(from);
        try {
          await editor.project.request({ type: "rename", from, to });
        } finally {
          renaming.delete(from);
        }
        const open = documents.getState().current;
        if (open?.path === from) documents.setState({ current: { ...open, path: to } });
        state.set({ selected: to });
        await refresh();
      },
      move: (path, folder) => assets.rename(path, `${folder}/${baseName(path)}`),
      async remove(path) {
        state.set({ action: null });
        await editor.project.request({ type: "delete", path });
        if (documents.getState().current?.path === path) documents.setState({ current: null, text: null });
        await refresh();
      },
      async reload() {
        const { banner } = state.get();
        state.set({ banner: null });
        if (banner) await replace(banner.text);
      },
      keepMine: () => state.set({ banner: null }),
    };

    // A change to the open scene or prefab reloads it, unless the editor holds unsaved edits.
    const onDiskChange = async (path: string) => {
      const { text } = await editor.project.request({ type: "read_text", path });
      const { current, text: loaded } = documents.getState();
      if (current?.path !== path || current.kind === "replay" || text === loaded) return;
      if (editor.timeline.getState().timeline?.scene_dirty) state.set({ banner: { path, text } });
      else await replace(text);
    };
    let info = project.get().info;
    const stops = [
      editor.project.on("files_changed", ({ changes }) => {
        void refresh();
        const open = documents.getState().current?.path;
        if (!open) return;
        // Removed or renamed outside the editor: nothing on disk to save the game's document to any more.
        if (!renaming.has(open) && changes.some((change) => removedPath(change) === open)) {
          documents.setState({ current: null, text: null });
          if (state.get().banner?.path === open) state.set({ banner: null });
        } else if (changes.some((change) => changedPath(change) === open)) onDiskChange(open).catch(report);
      }),
      project.subscribe(() => {
        if (project.get().info === info) return;
        info = project.get().info;
        void refresh();
      }),
      reveal.subscribe(() => {
        const { path } = reveal.get();
        if (path !== null) state.set({ filter: path, selected: path });
      }),
    ];
    void refresh();

    // Commands act on the selected asset and report failures in the panel.
    const onSelected = (run: (path: string) => void | Promise<void>) => async () => {
      const path = state.get().selected;
      try {
        if (!path) throw new Error("select an asset first");
        await run(path);
        state.set({ error: null });
      } catch (error) {
        report(error);
      }
    };
    const ask = (kind: Action["kind"]) => onSelected((path) => state.set({ action: { kind, path }, menu: null }));
    editor.commands.register({ id: "assets.open", title: "Open asset", run: onSelected(assets.open) });
    editor.commands.register({ id: "assets.rename", title: "Rename asset", run: ask("rename") });
    editor.commands.register({ id: "assets.move", title: "Move asset", run: ask("move") });
    editor.commands.register({ id: "assets.delete", title: "Delete asset", run: ask("delete") });
    editor.panels.register({
      id: "ludenea.assets",
      title: "Assets",
      defaultPosition: "left",
      component: ({ editor }: { editor: Editor }) => <AssetsPanel editor={editor} assets={assets} />,
    });
    return () => stops.forEach((stop) => stop());
  },
});
