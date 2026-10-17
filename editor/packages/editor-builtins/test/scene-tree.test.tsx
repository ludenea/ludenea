import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, settle, testEditor, testGame } from "./support";

const entity = (index: number) => ({ index, generation: 0 });
const json = (value: unknown) => ({ type: "json", value });

// jsdom has no layout: give the list a height so the virtualizer shows rows.
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 400 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 300 });
});

async function sceneTree(project: (request: { type: string }) => object | undefined = () => ({ type: "done" })) {
  const host = await testEditor(builtinPlugins, project);
  const game = await testGame((command) => {
    if (command.type === "query_entities") {
      return { type: "entities", total: 2, rows: [0, 1].map((index) => ({ entity: entity(index), hash: "00000000000000a" + index })) };
    }
    if (command.type === "get_entity") {
      const { index } = command.entity as { index: number };
      const components =
        index === 0
          ? { "ludenea::Pose": json({}), "ludenea::Body": json({}), "my_game::Player": json({}) }
          : { "ludenea::Collider": json({}) };
      return { type: "entity", entity: command.entity, components };
    }
    if (command.type === "save_scene") return { type: "scene", ron: "(saved)" };
    return { type: "done" };
  });
  act(() => host.setControl(game.client));
  renderPanel(host, "ludenea.scene-tree");
  await settle();
  return { host, game };
}

describe("scene tree", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

  it("scene_tree_selection_updates_store", async () => {
    const { host, game } = await sceneTree();
    expect(game.sent).toContainEqual({ type: "subscribe", topic: { type: "entities", filter: "" } });
    expect(game.sent).toContainEqual({ type: "query_entities", filter: "", offset: 0, limit: 200 });

    fireEvent.click(screen.getByText("Pose, Body, Player"));
    expect(host.editor.selection.getState().entities).toEqual([entity(0)]);
    fireEvent.click(screen.getByText("Collider"), { shiftKey: true });
    expect(host.editor.selection.getState().entities).toEqual([entity(0), entity(1)]);
    expect(screen.getByText("Collider").closest("[aria-selected]")?.getAttribute("aria-selected")).toBe("true");
  });

  it("despawn_selection_sends_despawn_and_clears_selection", async () => {
    const { host, game } = await sceneTree();
    host.editor.selection.getState().select([entity(1)]);

    await host.editor.commands.run("scene.despawn-selection");

    expect(game.sent).toContainEqual({ type: "despawn", entity: entity(1) });
    expect(host.editor.selection.getState().entities).toEqual([]);
  });

  it("save_writes_the_saved_scene_to_the_default_scene", async () => {
    const info = { type: "info", name: "demo", root: "/demo", default_scene: "scenes/main.scene.ron", mode: "browser" };
    const { host, game } = await sceneTree((request) => (request.type === "info" ? info : { type: "done" }));

    await host.editor.commands.run("scene.save");

    expect(game.sent).toContainEqual({ type: "save_scene" });
    expect(host.project.sent).toContainEqual({ type: "write_text", path: "scenes/main.scene.ron", text: "(saved)" });
  });
});
