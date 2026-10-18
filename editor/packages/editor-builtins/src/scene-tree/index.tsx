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
        const { scene, project } = shared(editor);
        const path = scene.get().path ?? project.get().info?.default_scene;
        if (!path) throw new Error("no scene is open");
        const { ron } = await game().request({ type: "save_scene" });
        await editor.project.request({ type: "write_text", path, text: ron });
        scene.set({ path, text: ron });
      },
    });
    editor.shortcuts.register({ key: "Mod+S", command: "scene.save" });
    editor.panels.register({ id: "ludenea.scene-tree", title: "Scene", defaultPosition: "left", component: SceneTree });
  },
});
