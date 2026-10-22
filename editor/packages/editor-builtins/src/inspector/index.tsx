import { ProtocolFailure, type JsonValue } from "@ludenea/control-client";
import { definePlugin, type EditCommand, type Editor } from "@ludenea/editor-shell";
import { createStore, shared, showPanel, type Store } from "../shared";
import { assertEditable } from "../timeline/editing";
import { InspectorPanel } from "./InspectorPanel";

/**
 * The component being added: the one picked (the first candidate when it is no longer one), the
 * value form's content for it once a default was rejected, and the last failure.
 */
export type AddDraft = { chosen: string; draft: { component: string; value: JsonValue } | null; error: string | null };

/** What the inspector's commands and panel share; kept here while the panel is hidden. */
/** `expanded`: the resources whose section is open, the only ones that follow the game. */
export type InspectorState = Store<{ focused: string | null; adding: AddDraft | null; expanded: string[] }>;

/** Runs a request on the selection; `NoSuchEntity` (seeked before its spawn) clears the selection. */
export async function onEntity<T>(editor: Editor, request: Promise<T>): Promise<T> {
  try {
    return await request;
  } catch (error) {
    if (error instanceof ProtocolFailure && error.code === "no_such_entity") editor.selection.getState().clear();
    throw error;
  }
}

export const inspectorPlugin = definePlugin({
  name: "ludenea.inspector",
  activate(editor) {
    const state: InspectorState = createStore({ focused: null as string | null, adding: null as AddDraft | null, expanded: [] as string[] });
    const target = () => {
      if (!editor.control) throw new Error("no game is running");
      const { entities } = editor.selection.getState();
      if (entities.length === 0) throw new Error("select an entity first");
      return { entities };
    };
    const focused = () => {
      const { entities } = target();
      const component = state.get().focused;
      if (!component) throw new Error("focus a component in the inspector first");
      return { entities, component };
    };
    editor.commands.register({
      id: "inspector.add-component",
      title: "Add component to the selected entities",
      run() {
        target();
        assertEditable(editor);
        state.set({ adding: state.get().adding ?? { chosen: "", draft: null, error: null } });
        showPanel(editor, "ludenea.inspector");
      },
    });
    editor.commands.register({
      id: "inspector.remove-component",
      title: "Remove the focused component from the selected entities",
      async run() {
        const { entities, component } = focused();
        const removals = entities.map((entity): EditCommand => ({ type: "remove_component", entity, component }));
        await onEntity(editor, editor.batched(removals));
        state.set({ focused: null });
      },
    });
    editor.commands.register({
      id: "inspector.apply-to-scene",
      title: "Apply the focused component to the scene",
      async run() {
        const { entities, component } = focused();
        const [entity] = entities;
        if (!entity || entities.length > 1) throw new Error("select one entity to apply its component to the scene");
        await onEntity(editor, editor.edit({ type: "apply_to_scene", entity, component }));
      },
    });
    /** The game and the asset `asked` names, else the selected data asset. */
    const assetOf = (asked?: unknown) => {
      const control = editor.control;
      if (!control) throw new Error("no game is running");
      const selected = editor.selection.getState().asset;
      const id = typeof asked === "string" ? asked : selected?.kind === "Data" ? selected.id : null;
      if (!id) throw new Error("select a data asset first");
      return { control, id };
    };
    editor.commands.register({
      id: "inspector.apply-to-asset",
      title: "Apply the selected data asset's current value to the asset",
      async run() {
        const { id } = assetOf();
        await editor.edit({ type: "apply_to_asset", id });
      },
    });
    editor.commands.register({
      id: "inspector.save-asset",
      title: "Save the selected data asset",
      // Given an asset identifier, such as a prefab "Apply to prefab" changed, it saves that asset.
      async run(_, asked) {
        const { control, id } = assetOf(asked);
        const open = editor.document.getState().current;
        if (open?.kind === "replay") throw new Error(`${open.path} is a replay: its data assets are its own, never written to the project's files`);
        const { text } = await control.request({ type: "save_asset", id });
        const { entries } = await editor.project.request({ type: "list_files" });
        const path = entries.find((file) => file.id === id)?.path;
        if (!path) throw new Error(`no project file has the identifier ${id}`);
        // The shell reloads the session's assets from the written file, and the asset browser
        // knows its text for the editor's own when the watcher reports it.
        shared(editor).written.set(path, text);
        await editor.project.request({ type: "write_text", path, text });
        // Saved only if the asset still holds the text written: an edit made meanwhile stays unsaved.
        await control.request({ type: "mark_asset_saved", id, text });
      },
    });
    editor.panels.register({
      id: "ludenea.inspector",
      title: "Inspector",
      defaultPosition: "right",
      component: ({ editor }) => <InspectorPanel editor={editor} state={state} />,
    });
  },
});
