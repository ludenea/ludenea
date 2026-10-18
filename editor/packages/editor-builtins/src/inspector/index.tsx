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

/**
 * What the inspector's commands and panel share; kept here while the panel is hidden.
 * `unsaved`: the data assets whose text the session changed (a tick 0 edit, "Apply to asset") since saved.
 */
export type InspectorState = Store<{ focused: string | null; adding: AddDraft | null; unsaved: AssetId[] }>;

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
    const state: InspectorState = createStore({ focused: null as string | null, adding: null as AddDraft | null, unsaved: [] as AssetId[] });
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
        markUnsaved(state, id);
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
        // The shell reloads the session's assets from the written file.
        await editor.project.request({ type: "write_text", path, text });
        state.set({ unsaved: state.get().unsaved.filter((other) => other !== id) });
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

export function markUnsaved(state: InspectorState, id: AssetId): void {
  const { unsaved } = state.get();
  if (!unsaved.includes(id)) state.set({ unsaved: [...unsaved, id] });
}
