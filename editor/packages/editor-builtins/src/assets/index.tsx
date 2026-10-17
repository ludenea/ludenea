import type { FileChange, FileEntry } from "@ludenea/control-client";
import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { createStore, errorText, shared, type Store } from "../shared";
import { AssetsPanel, baseName } from "./AssetsPanel";

export type Action = { kind: "rename" | "move" | "delete"; path: string };

export interface AssetsState {
  files: FileEntry[];
  filter: string;
  selected: string | null;
  /** The inline form (rename, move) or confirmation (delete) the panel shows. */
  action: Action | null;
  menu: string | null;
  /** The open scene changed on disk while it had unsaved edits. */
  banner: { path: string; text: string } | null;
  error: string | null;
}

export interface Assets {
  state: Store<AssetsState>;
  open(path: string): Promise<void>;
  rename(from: string, to: string): Promise<void>;
  move(path: string, folder: string): Promise<void>;
  remove(path: string): Promise<void>;
  reload(): Promise<void>;
  keepMine(): void;
}

const changedPath = (change: FileChange) => ("Changed" in change ? change.Changed : "Created" in change ? change.Created : null);
const removedPath = (change: FileChange) => ("Removed" in change ? change.Removed : null);

export const assetsPlugin = definePlugin({
  name: "ludenea.assets",
  activate(editor) {
    const { scene, reveal, project } = shared(editor);
    const state = createStore<AssetsState>({
      files: [],
      filter: "",
      selected: null,
      action: null,
      menu: null,
      banner: null,
      error: null,
    });
    const report = (error: unknown) => state.set({ error: errorText(error) });
    const refresh = () =>
      editor.project.request({ type: "list_files" }).then(
        ({ entries }) => state.set({ files: entries }),
        () => state.set({ files: [] }),
      );
    // Without a game the new text is only remembered: the next game loads it.
    const replace = async (text: string) => {
      await editor.control?.request({ type: "replace_scene", ron: text });
      scene.set({ text });
    };

    const assets: Assets = {
      state,
      async open(path) {
        state.set({ selected: path });
        const entry = state.get().files.find((file) => file.path === path);
        if (entry?.kind !== "Scene") throw new Error(`${path} is not a scene`);
        // Scene files are usable before the build: the scene is remembered and loaded into each game.
        const { text } = await editor.project.request({ type: "read_text", path });
        await editor.control?.request({ type: "load_scene", ron: text });
        scene.set({ path, text });
        state.set({ banner: null, error: null });
      },
      async rename(from, to) {
        state.set({ action: null });
        await editor.project.request({ type: "rename", from, to });
        if (scene.get().path === from) scene.set({ path: to });
        state.set({ selected: to });
        await refresh();
      },
      move: (path, folder) => assets.rename(path, `${folder}/${baseName(path)}`),
      async remove(path) {
        state.set({ action: null });
        await editor.project.request({ type: "delete", path });
        if (scene.get().path === path) scene.set({ path: null, text: null });
        await refresh();
      },
      async reload() {
        const { banner } = state.get();
        state.set({ banner: null });
        if (banner) await replace(banner.text);
      },
      keepMine: () => state.set({ banner: null }),
    };

    // A change to the open scene reloads it, unless the editor holds unsaved edits.
    const onDiskChange = async (path: string) => {
      const { text } = await editor.project.request({ type: "read_text", path });
      if (scene.get().path !== path || text === scene.get().text) return;
      if (editor.timeline.getState().timeline?.scene_dirty) state.set({ banner: { path, text } });
      else await replace(text);
    };
    let info = project.get().info;
    const stops = [
      editor.project.on("files_changed", ({ changes }) => {
        void refresh();
        const open = scene.get().path;
        if (!open) return;
        // Removed or renamed outside the editor: nothing on disk to save the game's scene to any more.
        if (changes.some((change) => removedPath(change) === open)) {
          scene.set({ path: null, text: null });
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
    // A new game (first build, rebuild, restart) starts from its own state: show it the open scene.
    editor.onGameChanged((control) => {
      const { text } = scene.get();
      if (control && text !== null) control.request({ type: "load_scene", ron: text }).catch(report);
    });
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
