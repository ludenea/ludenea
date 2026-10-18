import { definePlugin } from "@ludenea/editor-shell";
import { createStore } from "../shared";
import { ExplorerPanel, type ExplorerState, type Tab } from "./ExplorerPanel";

export const explorerPlugin = definePlugin({
  name: "ludenea.explorer",
  activate(editor) {
    // Kept here, not in the panel: the shell unmounts a panel while its tab is hidden.
    const state: ExplorerState = createStore({ tab: "Entities" as Tab, text: "", filter: "", scroll: { top: 0 } });
    editor.panels.register({
      id: "ludenea.explorer",
      title: "Explorer",
      defaultPosition: "bottom",
      component: ({ editor }) => <ExplorerPanel editor={editor} state={state} />,
    });
  },
});
