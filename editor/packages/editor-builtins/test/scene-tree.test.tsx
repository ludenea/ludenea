import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, settle, testEditor, testGame } from "./support";

const entity = (index: number) => ({ index, generation: 0 });
const json = (value: unknown) => ({ type: "json", value });

// jsdom has no layout: give the list a height so the virtualizer shows rows.
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 400 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 300 });
});

/** What the fake game holds: entity 1's components and hash can change. */
const world = () => ({ collider: { "ludenea::Collider": json({}) } as Record<string, unknown>, hash: "00000000000000a1" });

async function sceneTree(project: (request: { type: string }) => object | undefined = () => ({ type: "done" }), held = world()) {
  const host = await testEditor(builtinPlugins, project);
  const game = await testGame((command) => {
    if (command.type === "query_entities") {
      return { type: "entities", total: 2, rows: [0, 1].map((index) => ({ entity: entity(index), hash: index === 0 ? "00000000000000a0" : held.hash })) };
    }
    if (command.type === "get_entity") {
      const { index } = command.entity as { index: number };
      const components = index === 0 ? { "ludenea::Pose": json({}), "ludenea::Body": json({}), "my_game::Player": json({}) } : held.collider;
      return { type: "entity", entity: command.entity, components };
    }
    if (command.type === "save_scene") return { type: "scene", ron: "(saved)" };
    return { type: "done" };
  });
  act(() => host.setControl(game.client));
  renderPanel(host, "ludenea.scene-tree");
  await settle();
  return { host, game, held };
}

const nextFrame = () => act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
const queries = (game: { sent: { type: string }[] }) => game.sent.filter((command) => command.type === "query_entities").length;

describe("scene tree", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

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

  it("keyboard_selects_and_extends_selection", async () => {
    const { host } = await sceneTree();
    const row = (text: string) => screen.getByText(text).closest("[role=option]")!;
    expect(row("Collider").getAttribute("tabindex")).toBe("0");

    fireEvent.keyDown(row("Collider"), { key: "Enter" });
    expect(host.editor.selection.getState().entities).toEqual([entity(1)]);
    fireEvent.keyDown(row("Pose, Body, Player"), { key: "Enter", metaKey: true });
    expect(host.editor.selection.getState().entities).toEqual([entity(1), entity(0)]);

    await host.editor.commands.run("scene.despawn-selection");
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([]);
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

  it("space_selects_a_row", async () => {
    const { host } = await sceneTree();
    const row = screen.getByText("Collider").closest("[role=option]")!;
    const space = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    act(() => void row.dispatchEvent(space));
    expect(host.editor.selection.getState().entities).toEqual([entity(1)]);
    expect(space.defaultPrevented).toBe(true);
  });

  it("changes_in_one_frame_refetch_the_rows_once", async () => {
    const { game } = await sceneTree();
    const frames: FrameRequestCallback[] = [];
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => frames.push(callback));
    const before = queries(game);
    // A playing game reports a change each tick, each in its own message.
    for (let tick = 0; tick < 5; tick++) {
      act(() => game.push({ type: "entities_changed", filter: "", total: 2 }));
      await settle();
    }
    expect(queries(game)).toBe(before);
    act(() => frames.splice(0).forEach((frame) => frame(0)));
    await settle();
    expect(queries(game)).toBe(before + 1);
  });

  it("label_follows_a_component_added_while_paused", async () => {
    const { game, held } = await sceneTree();
    expect(screen.getByText("Collider")).toBeTruthy();
    held.collider = { ...held.collider, "my_game::Health": json({}) };
    held.hash = "00000000000000b1";
    act(() => game.push({ type: "entities_changed", filter: "", total: 2 }));
    await nextFrame();
    await settle();
    expect(screen.getByText("Collider, Health")).toBeTruthy();
  });

  it("a_new_game_starts_from_an_empty_list", async () => {
    const { host } = await sceneTree();
    expect(screen.getAllByRole("option")).toHaveLength(2);
    // A game that has not answered yet shows none of the previous game's rows.
    const silent = await testGame(() => undefined);
    act(() => host.setControl(silent.client));
    await settle();
    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });

  it("delete_despawns_only_from_the_scene_tree", async () => {
    const { host, game } = await sceneTree();
    host.editor.selection.getState().select([entity(1)]);
    const despawns = () => game.sent.filter((command) => command.type === "despawn");
    // Not a global shortcut: Delete in the asset browser deletes an asset.
    expect(host.registry.shortcuts.get("Delete")).toBeUndefined();

    fireEvent.keyDown(document.body, { key: "Delete" });
    await settle();
    expect(despawns()).toEqual([]);

    fireEvent.keyDown(screen.getByText("Collider"), { key: "Delete" });
    await settle();
    expect(despawns()).toEqual([{ type: "despawn", entity: entity(1) }]);
  });

  it("scroll_position_survives_hiding_the_panel", async () => {
    const { host } = await sceneTree();
    const list = () => document.querySelector(".ludenea-entity-list") as HTMLElement;
    list().scrollTop = 440;
    fireEvent.scroll(list());

    // The shell unmounts a panel while its tab is hidden.
    cleanup();
    renderPanel(host, "ludenea.scene-tree");
    await settle();
    expect(list().scrollTop).toBe(440);
  });
});
