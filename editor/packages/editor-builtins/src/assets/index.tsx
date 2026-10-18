import type { AssetId, AssetKind, FileChange, FileEntry, JsonValue, TypeSchema } from "@ludenea/control-client";
import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { createStore, errorText, shared, showPanel, type Scroll, type Store } from "../shared";
import { AssetsPanel, baseName } from "./AssetsPanel";
import { emptyValue, toRon } from "./value";

export type Action = { kind: "rename" | "move" | "delete"; path: string };

/** A file kind without its data type: what the browser shows and filters by. */
export type KindName = "Scene" | "Prefab" | "Data" | "Replay" | "Trace" | "Snapshot" | "Other";
export const kindName = (kind: AssetKind): KindName => (typeof kind === "string" ? kind : "Data");

/**
 * The data asset being created: its type and path, then, for a type without a default value,
 * the value the user fills from the schema.
 */
export type NewData = { type: string; path: string; value: { schema: TypeSchema; draft: JsonValue } | null };

export interface AssetsState {
  files: FileEntry[];
  filter: string;
  /** Only files of this kind; null for every kind. */
  kind: KindName | null;
  selected: string | null;
  /** The inline form (rename, move) or confirmation (delete) the panel shows. */
  action: Action | null;
  menu: string | null;
  creating: NewData | null;
  /** The open document, or a data asset, changed on disk while it had unsaved edits. */
  banner: { path: string; text: string } | { path: string; asset: AssetId } | null;
  error: string | null;
  /** Why the files could not be listed. */
  listError: string | null;
}

export interface Assets {
  state: Store<AssetsState>;
  /** Kept here, not in the panel: the shell unmounts a panel while its tab is hidden. */
  scroll: Scroll;
  /** Opens a scene, prefab, or replay as the document, a trace or snapshot in the divergence viewer, and a data asset in the inspector. */
  open(path: string): Promise<void>;
  /** Starts the "New data asset" form, for the game's asset types. */
  newDataAsset(): void;
  /** Creates the data asset the form describes: from its type's default value, else from the value filled in. */
  create(): Promise<void>;
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
      kind: null,
      selected: null,
      action: null,
      menu: null,
      creating: null,
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
    // Without a game the new text is only remembered: the next game loads it. The document
    // store reports a refused load in the console, at the asset at fault.
    const replace = (text: string) => documents.getState().replace(text);

    // The watcher may report a rename's removal before the service answers it.
    const renaming = new Set<string>();
    const assets: Assets = {
      state,
      scroll: { top: 0 },
      async open(path) {
        state.set({ selected: path });
        const entry = state.get().files.find((file) => file.path === path);
        if (!entry) throw new Error(`no project file is at ${path}`);
        const kind = kindName(entry.kind);
        // Registered by the divergence viewer.
        if (kind === "Trace" || kind === "Snapshot") return editor.commands.run("divergence.open-file", { path });
        if (kind === "Data") {
          editor.selection.getState().selectAsset(entry.id);
          return showPanel(editor, "ludenea.inspector");
        }
        if (kind !== "Scene" && kind !== "Prefab" && kind !== "Replay") throw new Error(`${path} is not a scene, a prefab, a replay, a trace, a snapshot, or a data asset`);
        // Documents are usable before the build: the document is remembered and loaded into each game.
        await documents.getState().open(entry.id);
        state.set({ banner: null, error: null });
      },
      newDataAsset() {
        const type = editor.control?.game.assets[0]?.name;
        if (!type) throw new Error(editor.control ? "the game registers no asset type" : "no game is running");
        state.set({ creating: { type, path: "", value: null }, action: null, menu: null });
      },
      async create() {
        const creating = state.get().creating;
        if (!creating) return;
        const { type, path, value } = creating;
        if (!path.trim()) throw new Error("enter the new asset's path");
        if (value) {
          await editor.project.request({ type: "create_asset", path, kind: { Data: { type } }, text: toRon(value.schema, value.draft) });
        } else {
          const created = await editor.assets.create(type, path);
          if (created.kind === "needs-value") {
            state.set({ creating: { ...creating, value: { schema: created.schema, draft: emptyValue(created.schema) } } });
            return;
          }
        }
        state.set({ creating: null, selected: path });
        await refresh();
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
        if (!banner) return;
        if ("text" in banner) return replace(banner.text);
        const { unsavedAssets } = documents.getState();
        documents.setState({ unsavedAssets: unsavedAssets.filter((id) => id !== banner.asset) });
        await documents.getState().reloadAssets();
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
        // The shell's reload keeps an unsaved data asset's text: the user picks which stays.
        const { unsavedAssets } = documents.getState();
        for (const change of changes) {
          const id = state.get().files.find((file) => file.path === changedPath(change))?.id;
          if (id && unsavedAssets.includes(id)) state.set({ banner: { path: changedPath(change)!, asset: id } });
        }
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
    editor.commands.register({
      id: "assets.new-data",
      title: "New data asset",
      run: () => {
        try {
          assets.newDataAsset();
        } catch (error) {
          report(error);
        }
      },
    });
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
