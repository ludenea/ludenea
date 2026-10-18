import { definePlugin } from "@ludenea/editor-shell";
import { shared } from "../shared";
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
        if (open?.kind === "replay") throw new Error(`${open.path} is a replay: its file is never overwritten`);
        // The session of a prefab document holds the prefab file.
        const path = open?.path ?? shared(editor).project.get().info?.default_scene;
        if (!path) throw new Error("no scene is open");
        const { ron } = await game().request({ type: "save_scene" });
        await editor.project.request({ type: "write_text", path, text: ron });
        if (open) editor.document.setState({ text: ron });
      },
    });
    editor.shortcuts.register({ key: "Mod+S", command: "scene.save" });
    // Kept here, not in the panel: the shell unmounts a panel while its tab is hidden.
    const scroll = { top: 0 };
    editor.panels.register({ id: "ludenea.scene-tree", title: "Scene", defaultPosition: "left", component: ({ editor }) => <SceneTree editor={editor} scroll={scroll} /> });
  },
});
