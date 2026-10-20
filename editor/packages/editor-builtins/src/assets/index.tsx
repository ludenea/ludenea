import type { AssetId, AssetKind, FileChange, FileEntry, JsonValue, TypeSchema } from "@ludenea/control-client";
import { definePlugin, type Editor, type OpenDocument } from "@ludenea/editor-shell";
import { createStore, errorText, shared, showPanel, unsavedAssets, type Scroll, type Store } from "../shared";
import { AssetsPanel, baseName } from "./AssetsPanel";
import { emptyValue, toRon } from "./value";

export type Action = { kind: "rename" | "move" | "delete"; path: string };

/**
 * A file changed on disk while the editor held unsaved edits of it: the open document (its new
 * text) or a data asset; or the open document's file was deleted on disk (`removed`), while
 * the document stays open with its session.
 */
export type Banner = { path: string; text: string } | { path: string; asset: AssetId } | { path: string; removed: true };

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
  /** The open document, and data assets, changed on disk while they had unsaved edits: one banner each. */
  banners: Banner[];
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
  /** Takes the disk's version of the file `banner` names. */
  reload(banner: Banner): Promise<void>;
  /** Keeps the editor's version of the file `banner` names. */
  keepMine(banner: Banner): void;
  /** Closes the open document whose file `banner` says was deleted on disk, once its session is stored. */
  close(banner: Banner): Promise<void>;
}

const changedPath = (change: FileChange) => ("Changed" in change ? change.Changed.path : "Created" in change ? change.Created.path : null);
const createdPath = (change: FileChange) => ("Created" in change ? change.Created.path : null);
const removedPath = (change: FileChange) => ("Removed" in change ? change.Removed.path : null);

export const assetsPlugin = definePlugin({
  name: "ludenea.assets",
  activate(editor) {
    const { reveal, project, written } = shared(editor);
    const documents = editor.document;
    const state = createStore<AssetsState>({
      files: [],
      filter: "",
      kind: null,
      selected: null,
      action: null,
      menu: null,
      creating: null,
      banners: [],
      error: null,
      listError: null,
    });
    // A newer change of the same file replaces its banner.
    const showBanner = (banner: Banner) => state.set({ banners: [...withoutBanner(banner.path), banner] });
    const withoutBanner = (path: string) => state.get().banners.filter((other) => other.path !== path);
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
        state.set({ banners: state.get().banners.filter((banner) => !("text" in banner)), error: null });
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
      async reload(banner) {
        state.set({ banners: withoutBanner(banner.path) });
        if ("text" in banner) return replace(banner.text);
        if ("asset" in banner) await documents.getState().reloadAssets([banner.asset]);
      },
      keepMine: (banner) => state.set({ banners: withoutBanner(banner.path) }),
      async close(banner) {
        if (documents.getState().current?.path !== banner.path) return;
        // Stored again: the session changed since the file was deleted.
        await documents
          .getState()
          .store()
          .catch((error: unknown) => {
            throw new Error(`${banner.path} stays open: its session cannot be stored: ${errorText(error)}`);
          });
        documents.setState({ current: null, text: null });
      },
    };

    // A change to the open scene or prefab reloads it, unless the editor holds unsaved edits.
    const onDiskChange = async (path: string) => {
      const { text } = await editor.project.request({ type: "read_text", path });
      const { current, text: loaded } = documents.getState();
      if (current?.path !== path || current.kind === "replay" || text === loaded) return;
      if (editor.timeline.getState().timeline?.scene_dirty) showBanner({ path, text });
      else await replace(text);
    };
    /**
     * Finds the file of `open` in the fresh listing, after any batch of `changes`: a move can
     * arrive in several batches (a checkout removes every file before it writes them back).
     * The document stays with its file while that file keeps its identifier. Otherwise it
     * follows the one file with its identifier, then takes the moved file's text as any change
     * on disk; several files with it (copied with their metas, which the project service warns
     * about) are none it can tell is its own, so it follows none. With no file of its
     * identifier, it follows a file `changes` created that holds its text (moved without its
     * meta, the file has an identifier of its own, under which the session is stored at once,
     * where the next open and crash recovery look for it). When `changes` removed the file and
     * it is really gone, the session is stored and the document stays open as deleted on disk
     * until it is saved again, closed, or found by a later batch.
     */
    const follow = async (open: OpenDocument, changes: FileChange[]) => {
      // Another document, or this one renamed in the editor, is open now.
      const still = () => documents.getState().current === open;
      const sameKind = (file: FileEntry) => kindName(file.kind).toLowerCase() === open.kind;
      const { files } = state.get();
      const withId = files.filter((file) => file.id === open.id && sameKind(file));
      let found = withId.find((file) => file.path === open.path) ?? (withId.length === 1 ? withId[0] : undefined);
      // A replay has no text to recognise.
      const text = documents.getState().text;
      if (withId.length === 0 && text !== null) {
        const created = files.filter((file) => sameKind(file) && changes.some((change) => createdPath(change) === file.path));
        for (const file of created) {
          // A file that cannot be read is not known to hold it.
          const read = await editor.project.request({ type: "read_text", path: file.path }).catch(() => null);
          if (read?.text !== text) continue;
          found = file;
          break;
        }
      }
      if (!still()) return;
      if (found) {
        const { banners } = state.get();
        const kept = banners.filter((banner) => !("removed" in banner && banner.path === open.path));
        if (kept.length < banners.length) state.set({ banners: kept });
        if (found.id === open.id && found.path === open.path) return;
        documents.setState({ current: { ...open, id: found.id, path: found.path } });
        if (found.id !== open.id) await documents.getState().store();
        else if (text !== null) await onDiskChange(found.path);
        return;
      }
      if (!changes.some((change) => removedPath(change) === open.path)) return;
      try {
        await documents.getState().store();
      } catch (error) {
        editor.console.log("warning", `Cannot store the session of ${open.path}, deleted on disk: ${errorText(error)}`);
      }
      if (still()) showBanner({ path: open.path, removed: true });
    };
    let info = project.get().info;
    const stops = [
      editor.project.on("files_changed", ({ changes }) => {
        // The shell's reload keeps an unsaved data asset's text: the user picks which stays.
        const unsaved = unsavedAssets(editor);
        for (const change of changes) {
          const path = changedPath(change);
          const id = state.get().files.find((file) => file.path === path)?.id;
          if (!path || !id || !unsaved.includes(id)) continue;
          const mine = written.get(path);
          if (mine === undefined) {
            showBanner({ path, asset: id });
            continue;
          }
          // The editor's own save, reported before the game says the asset is saved, is no change.
          editor.project
            .request({ type: "read_text", path })
            .then(({ text }) => {
              if (text !== mine && unsavedAssets(editor).includes(id)) showBanner({ path, asset: id });
            })
            .catch(report);
        }
        const listed = refresh();
        const open = documents.getState().current;
        if (!open) return;
        if (!renaming.has(open.path)) listed.then(() => follow(open, changes)).catch(report);
        if (changes.some((change) => changedPath(change) === open.path)) onDiskChange(open.path).catch(report);
      }),
      // A deleted document's banner goes once another document, or none, is open.
      documents.subscribe(({ current }) => {
        const { banners } = state.get();
        const kept = banners.filter((banner) => !("removed" in banner) || banner.path === current?.path);
        if (kept.length < banners.length) state.set({ banners: kept });
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
