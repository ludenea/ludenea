import { definePlugin } from "@ludenea/editor-shell";
import { ExplorerPanel } from "./ExplorerPanel";

export const explorerPlugin = definePlugin({
  name: "ludenea.explorer",
  activate(editor) {
    editor.panels.register({ id: "ludenea.explorer", title: "Explorer", defaultPosition: "bottom", component: ExplorerPanel });
  },
});
