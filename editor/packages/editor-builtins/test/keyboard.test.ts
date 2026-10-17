import { describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { testEditor } from "./support";

const ACTIONS = [
  "project.new",
  "project.open",
  "project.build",
  "assets.rename",
  "assets.move",
  "assets.delete",
  "assets.open",
  "scene.save",
  "scene.despawn-selection",
  "inspector.add-component",
  "inspector.remove-component",
  "inspector.apply-to-scene",
  "viewport.pan-left",
  "viewport.zoom-in",
  "viewport.frame-selection",
  "timeline.play-pause",
  "timeline.step",
  "timeline.step-back",
  "edit.undo",
  "edit.redo",
];

describe("keyboard", () => {
  it("every_action_is_a_command", async () => {
    const host = await testEditor(builtinPlugins);
    const ids = host.editor.commands.list().map((command) => command.id);
    expect(ACTIONS.filter((id) => !ids.includes(id))).toEqual([]);
  });
});
