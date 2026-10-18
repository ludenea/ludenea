import { ProtocolFailure, type AssetId, type JsonValue } from "@ludenea/control-client";
import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { createStore, showPanel, type Store } from "../shared";
import { assertEditable, edit } from "../timeline/editing";
import { InspectorPanel } from "./InspectorPanel";

/**
 * The component being added: the one picked (the first candidate when it is no longer one), the
 * value form's content for it once a default was rejected, and the last failure.
 */
export type AddDraft = { chosen: string; draft: { component: string; value: JsonValue } | null; error: string | null };

/** What the inspector's commands and panel share; kept here while the panel is hidden. */
export type InspectorState = Store<{ focused: string | null; adding: AddDraft | null }>;

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
    const state: InspectorState = createStore({ focused: null as string | null, adding: null as AddDraft | null });
    const target = () => {
      const control = editor.control;
      if (!control) throw new Error("no game is running");
      const entity = editor.selection.getState().entities[0];
      if (!entity) throw new Error("select an entity first");
      return { control, entity };
    };
    const focused = () => {
      const { control, entity } = target();
      const component = state.get().focused;
      if (!component) throw new Error("focus a component in the inspector first");
      return { control, entity, component };
    };
    editor.commands.register({
      id: "inspector.add-component",
      title: "Add component to the selected entity",
      run() {
        target();
        assertEditable(editor);
        state.set({ adding: state.get().adding ?? { chosen: "", draft: null, error: null } });
        showPanel(editor, "ludenea.inspector");
      },
    });
    editor.commands.register({
      id: "inspector.remove-component",
      title: "Remove the focused component",
      async run() {
        const { control, entity, component } = focused();
        await onEntity(editor, edit(editor, control, { type: "remove_component", entity, component }));
        state.set({ focused: null });
      },
    });
    editor.commands.register({
      id: "inspector.apply-to-scene",
      title: "Apply the focused component to the scene",
      async run() {
        const { control, entity, component } = focused();
        await onEntity(editor, edit(editor, control, { type: "apply_to_scene", entity, component }));
      },
    });
    const selectedAsset = () => {
      const control = editor.control;
      if (!control) throw new Error("no game is running");
      const id = editor.selection.getState().asset;
      if (!id) throw new Error("select a data asset first");
      return { control, id };
    };
    editor.commands.register({
      id: "inspector.apply-to-asset",
      title: "Apply the selected data asset's current value to the asset",
      async run() {
        const { control, id } = selectedAsset();
        await edit(editor, control, { type: "apply_to_asset", id });
        markUnsaved(editor, id, true);
      },
    });
    editor.commands.register({
      id: "inspector.save-asset",
      title: "Save the selected data asset",
      async run() {
        const { control, id } = selectedAsset();
        const { text } = await control.request({ type: "save_asset", id });
        const { entries } = await editor.project.request({ type: "list_files" });
        const path = entries.find((file) => file.id === id)?.path;
        if (!path) throw new Error(`no project file has the identifier ${id}`);
        // Saved before the write: the watcher's reload of the written file may come first.
        markUnsaved(editor, id, false);
        try {
          // The shell reloads the session's assets from the written file.
          await editor.project.request({ type: "write_text", path, text });
        } catch (error) {
          markUnsaved(editor, id, true);
          throw error;
        }
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

/** Marks the data asset `id` as differing from its file, or not. */
export function markUnsaved(editor: Editor, id: AssetId, unsaved: boolean): void {
  const { unsavedAssets } = editor.document.getState();
  const others = unsavedAssets.filter((other) => other !== id);
  editor.document.setState({ unsavedAssets: unsaved ? [...others, id] : others });
}
