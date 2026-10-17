import { ProtocolFailure } from "@ludenea/control-client";
import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { createStore, showPanel, type Store } from "../shared";
import { InspectorPanel } from "./InspectorPanel";

/** What the inspector's commands and panel share. */
export type InspectorState = Store<{ focused: string | null; adding: boolean }>;

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
    const state: InspectorState = createStore({ focused: null as string | null, adding: false });
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
        state.set({ adding: true });
        showPanel(editor, "ludenea.inspector");
      },
    });
    editor.commands.register({
      id: "inspector.remove-component",
      title: "Remove the focused component",
      async run() {
        const { control, entity, component } = focused();
        await onEntity(editor, control.request({ type: "remove_component", entity, component }));
        state.set({ focused: null });
      },
    });
    editor.commands.register({
      id: "inspector.apply-to-scene",
      title: "Apply the focused component to the scene",
      async run() {
        const { control, entity, component } = focused();
        await onEntity(editor, control.request({ type: "apply_to_scene", entity, component }));
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
