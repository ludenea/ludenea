import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { runReported } from "@ludenea/editor-shell";
import { builtinPlugins } from "../src";
import { renderPanel, settle, testEditor, testGame } from "./support";

const entity = (index: number) => ({ index, generation: 0 });
const json = (value: unknown) => ({ type: "json", value });

// jsdom has no layout: give the list a height so the virtualizer shows rows.
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 400 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 300 });
});

/** What the fake game holds: entity 1's components and hash can change, its reads can fail with `failing`, and `save_scene` with `saveRefused`. */
const world = () => ({
  collider: { "ludenea::Collider": json({}) } as Record<string, unknown>,
  hash: "00000000000000a1",
  silent: false,
  failing: null as { code: string; message: string } | null,
  saveRefused: null as { code: string; message: string } | null,
});
const notAlive = { code: "no_such_entity", message: "not alive" };

async function sceneTree(project: (request: { type: string }) => object | undefined = () => ({ type: "done" }), held = world()) {
  const host = await testEditor(builtinPlugins, project);
  const game = await testGame((command) => {
    if (command.type === "query_entities") {
      return { type: "entities", total: 2, rows: [0, 1].map((index) => ({ entity: entity(index), hash: index === 0 ? "00000000000000a0" : held.hash })) };
    }
    if (command.type === "get_entity") {
      const { index } = command.entity as { index: number };
      // Left unanswered: the label stays in flight.
      if (held.silent && index === 1) return undefined;
      if (held.failing && index === 1) return { error: held.failing };
      const components = index === 0 ? { "ludenea::Pose": json({}), "ludenea::Body": json({}), "my_game::Player": json({}) } : held.collider;
      return { type: "entity", entity: command.entity, components };
    }
    if (command.type === "save_scene") return held.saveRefused ? { error: held.saveRefused } : { type: "scene", ron: "(saved)" };
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

  it("despawning_three_selected_entities_sends_one_batch", async () => {
    const host = await testEditor(builtinPlugins);
    // At tick 0 with a recorded future, the game refuses a second edit while the first resimulates.
    let edits = 0;
    const game = await testGame((command) => {
      if (command.type !== "despawn" && command.type !== "batch") return { type: "done" };
      edits += 1;
      return edits === 1 ? { type: "done" } : { error: { code: "resimulating", message: "the session is resimulated up to tick 0 only; wait for resimulation to finish" } };
    });
    act(() => host.setControl(game.client));
    host.editor.selection.getState().select([entity(1), entity(2), entity(3)]);

    await host.editor.commands.run("scene.despawn-selection");

    const despawns = [1, 2, 3].map((index) => ({ type: "despawn", entity: entity(index) }));
    expect(game.sent.filter((command) => command.type === "despawn" || command.type === "batch")).toEqual([{ type: "batch", commands: despawns }]);
    expect(host.editor.selection.getState().entities).toEqual([]);
  });

  it("save_without_an_open_document_writes_nothing", async () => {
    const info = { type: "info", name: "demo", root: "/demo", default_scene: "scenes/main.scene.ron", mode: "browser" };
    const { host, game } = await sceneTree((request) => (request.type === "info" ? info : { type: "done" }));
    renderPanel(host, "ludenea.console");

    await host.editor.commands.run("scene.save");
    await settle();

    expect(game.sent.filter((command) => command.type === "save_scene")).toEqual([]);
    expect(host.project.sent.filter((request) => request.type === "write_text")).toEqual([]);
    // Opening a document would replace the session the game holds: the message does not advise it.
    expect(screen.getByText("Nothing saved: no scene or prefab is open, so the game's session has no file to go to.")).toBeTruthy();
  });

  it("save_writes_a_prefab_document_to_its_own_file", async () => {
    const { host, game } = await sceneTree();
    host.editor.document.setState({ current: { id: "0000000000000000000000000000000d", kind: "prefab", path: "assets/ball.prefab.ron" } });

    await host.editor.commands.run("scene.save");

    expect(host.project.sent.filter((request) => request.type === "write_text")).toEqual([{ type: "write_text", path: "assets/ball.prefab.ron", text: "(saved)" }]);
    // Saved once its file holds the text.
    expect(game.sent.at(-1)).toEqual({ type: "mark_scene_saved", text: "(saved)" });
    expect(host.editor.document.getState().text).toBe("(saved)");
  });

  it("a_failed_write_keeps_the_scene_unsaved_and_says_why", async () => {
    const refused = { error: { code: "io", message: "permission denied" } };
    const { host, game } = await sceneTree((request) => (request.type === "write_text" ? refused : { type: "done" }));
    host.editor.document.setState({ current: { id: "0000000000000000000000000000000c", kind: "scene", path: "scenes/main.scene.ron" }, text: "(on disk)" });

    // A plugin running it sees the failure.
    await expect(host.editor.commands.run("scene.save")).rejects.toThrow("Cannot save scenes/main.scene.ron: permission denied");
    // The palette and Mod+S report it once.
    expect(await runReported(host.editor, "scene.save")).toBe("Save scene failed: Cannot save scenes/main.scene.ron: permission denied");

    expect(game.sent.filter((command) => command.type === "mark_scene_saved")).toEqual([]);
    expect(host.editor.document.getState().text).toBe("(on disk)");
    const errors = host.editor.console.list().filter(({ level }) => level === "error");
    expect(errors.map(({ text }) => text)).toEqual(["Save scene failed: Cannot save scenes/main.scene.ron: permission denied"]);
  });

  it("a_save_the_game_refuses_writes_nothing_and_says_why", async () => {
    const held = { ...world(), saveRefused: { code: "invalid_scene", message: "prefab instances form a cycle" } };
    const { host, game } = await sceneTree(undefined, held);
    host.editor.document.setState({ current: { id: "0000000000000000000000000000000c", kind: "scene", path: "scenes/main.scene.ron" }, text: "(on disk)" });

    expect(await runReported(host.editor, "scene.save")).toBe("Save scene failed: Cannot save scenes/main.scene.ron: prefab instances form a cycle");

    expect(host.project.sent.filter((request) => request.type === "write_text")).toEqual([]);
    expect(game.sent.filter((command) => command.type === "mark_scene_saved")).toEqual([]);
    expect(host.editor.document.getState().text).toBe("(on disk)");
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

  it("a_failed_label_read_is_retried_on_the_next_hash_change", async () => {
    const { host, game, held } = await sceneTree(undefined, { ...world(), failing: { code: "invalid_command", message: "cannot encode it" } });
    expect(screen.queryByText("Collider")).toBeNull();
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toContainEqual(["warning", "Cannot read the components of entity 1v0 for its label: cannot encode it"]);
    held.failing = null;
    held.hash = "00000000000000b1";
    act(() => game.push({ type: "entities_changed", filter: "", total: 2 }));
    await nextFrame();
    await settle();
    expect(screen.getByText("Collider")).toBeTruthy();
  });

  it("a_label_of_an_entity_gone_meanwhile_is_no_console_entry", async () => {
    // While playing, a short-lived entity can be gone before its label is read.
    const { host } = await sceneTree(undefined, { ...world(), failing: notAlive });
    expect(screen.queryByText("Collider")).toBeNull();
    expect(host.editor.console.list().filter(({ text }) => text.includes("for its label"))).toEqual([]);
  });

  it("a_failed_label_read_waits_for_a_pause_to_retry_while_playing", async () => {
    const held = { ...world(), failing: notAlive as { code: string; message: string } | null };
    const { host, game } = await sceneTree(undefined, held);
    const labelReads = () => game.sent.filter((command) => command.type === "get_entity" && (command.entity as { index: number }).index === 1).length;
    act(() => host.editor.timeline.setState({ playing: true }));
    for (const hash of ["00000000000000b1", "00000000000000c1", "00000000000000d1"]) {
      held.hash = hash;
      act(() => game.push({ type: "entities_changed", filter: "", total: 2 }));
      await nextFrame();
      await settle();
    }
    expect(labelReads()).toBe(1);
    held.failing = null;
    act(() => host.editor.timeline.setState({ playing: false }));
    await settle();
    expect(screen.getByText("Collider")).toBeTruthy();
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

  it("a_label_in_flight_is_not_requested_again_while_playing", async () => {
    const held = { ...world(), silent: true };
    const { host, game } = await sceneTree(undefined, held);
    act(() => host.editor.timeline.setState({ playing: true }));
    for (const hash of ["00000000000000b1", "00000000000000c1", "00000000000000d1"]) {
      held.hash = hash;
      act(() => game.push({ type: "entities_changed", filter: "", total: 2 }));
      await nextFrame();
      await settle();
    }
    const labelRequests = game.sent.filter((command) => command.type === "get_entity" && (command.entity as { index: number }).index === 1);
    expect(labelRequests).toHaveLength(1);
  });
});
