import { definePlugin } from "@ludenea/editor-shell";
import { edit } from "../timeline/editing";
import { SceneTree } from "./SceneTree";

export const sceneTreePlugin = definePlugin({
  name: "ludenea.scene-tree",
  activate(editor) {
    const game = () => {
      if (!editor.control) throw new Error("no game is running");
      return editor.control;
    };
    editor.commands.register({
      id: "scene.despawn-selection",
      title: "Despawn selected entities",
      async run() {
        const control = game();
        const { entities } = editor.selection.getState();
        await Promise.all(entities.map((entity) => edit(editor, control, { type: "despawn", entity })));
        editor.selection.getState().clear();
      },
    });
    editor.commands.register({
      id: "scene.save",
      title: "Save scene",
      async run() {
        const open = editor.document.getState().current;
        // The game may still hold the session of a document removed or moved since: it has no file to go to.
        if (!open) return editor.console.log("warning", "Nothing saved: no document is open (the open one was removed or moved); open a document first.");
        if (open.kind === "replay") throw new Error(`${open.path} is a replay: its file is never overwritten`);
        // The session of a prefab document holds the prefab file.
        const { ron } = await game().request({ type: "save_scene" });
        await editor.project.request({ type: "write_text", path: open.path, text: ron });
        editor.document.setState({ text: ron });
      },
    });
    editor.shortcuts.register({ key: "Mod+S", command: "scene.save" });
    // Kept here, not in the panel: the shell unmounts a panel while its tab is hidden.
    const scroll = { top: 0 };
    editor.panels.register({ id: "ludenea.scene-tree", title: "Scene", defaultPosition: "left", component: ({ editor }) => <SceneTree editor={editor} scroll={scroll} /> });
  },
});
