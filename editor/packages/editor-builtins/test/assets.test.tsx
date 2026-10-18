import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, setSceneDirty, settle, testEditor, testGame } from "./support";

const SCENE = "scenes/main.scene.ron";
const entries = [
  { path: SCENE, id: "0000000000000000000000000000000a", kind: "Scene" },
  { path: "assets/logo.png", id: "0000000000000000000000000000000b", kind: "Other" },
];

/** The editor with every built-in plugin, a game, and the asset browser showing two files. */
const SETTINGS = "0000000000000000000000000000000c";

async function browser(disk: { text: string } = { text: "(on disk)" }, withGame = true, game?: Awaited<ReturnType<typeof testGame>>) {
  const host = await testEditor(builtinPlugins, (request) => {
    if (request.type === "list_files") return { type: "files", entries };
    if (request.type === "read_text") return { type: "text", text: disk.text };
    if (request.type === "read_asset_texts") return { type: "asset_texts", assets: [], paths: [[SETTINGS, "assets/settings.ron"]] };
    if (request.type === "load_stored_session") return { type: "stored_session", bytes: null };
    if (request.type === "info") return undefined;
    return { type: "done" };
  });
  game ??= await testGame();
  if (withGame) act(() => host.setControl(game.client));
  renderPanel(host, "ludenea.assets");
  await settle();
  return { host, game, disk };
}

async function openScene() {
  fireEvent.doubleClick(screen.getByText("main.scene.ron"));
  await settle();
}

describe("assets", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

  it("opening_scene_loads_it_into_the_game", async () => {
    const { host, game } = await browser({ text: "(scene)" });

    await openScene();

    expect(host.project.sent).toContainEqual({ type: "read_text", path: SCENE });
    expect(game.sent.filter((command) => command.type === "load_scene")).toEqual([{ type: "load_scene", scene: "(scene)", assets: [], document: "Scene" }]);
  });

  it("disk_change_with_unsaved_edits_shows_banner_and_keeps_mine", async () => {
    const { host, game, disk } = await browser();
    await openScene();
    setSceneDirty(host, true);

    disk.text = "(changed)";
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: SCENE, kind: "Scene" } }] }));
    await settle();

    expect(screen.getByRole("alert").textContent).toContain("changed on disk");
    fireEvent.click(screen.getByRole("button", { name: "Keep mine" }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([]);

    disk.text = "(changed again)";
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: SCENE, kind: "Scene" } }] }));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    await settle();
    expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([{ type: "replace_scene", scene: "(changed again)", assets: [] }]);
  });

  it("disk_change_without_unsaved_edits_sends_replace_scene", async () => {
    const { host, game, disk } = await browser();
    await openScene();
    setSceneDirty(host, false);

    disk.text = "(changed)";
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: SCENE, kind: "Scene" } }, { Created: { path: "assets/new.png", kind: "Other" } }] }));
    await settle();

    expect(screen.queryByRole("alert")).toBeNull();
    expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([{ type: "replace_scene", scene: "(changed)", assets: [] }]);
  });

  it("a_broken_asset_is_logged_at_its_file_line_and_column", async () => {
    const message = `asset ${SETTINGS} is invalid: 2:17-2:18: Expected integer`;
    const game = await testGame((command) => (command.type === "replace_scene" ? { error: { code: "invalid_scene", message } } : { type: "done" }));
    const { host, disk } = await browser(undefined, true, game);
    renderPanel(host, "ludenea.console");
    await openScene();
    setSceneDirty(host, false);

    disk.text = "(changed)";
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: SCENE, kind: "Scene" } }] }));
    await settle();

    const lines = screen.getByRole("list", { name: "Console" });
    expect(lines.textContent).toContain(message);
    expect(screen.getByRole("button", { name: "assets/settings.ron:2:17" })).toBeTruthy();
  });

  it("unsaved_marker_follows_scene_dirty", async () => {
    const { host } = await browser();
    await openScene();
    expect(screen.queryByLabelText("unsaved changes")).toBeNull();

    setSceneDirty(host, true);
    expect(screen.getByLabelText("unsaved changes")).toBeTruthy();
    setSceneDirty(host, false);
    expect(screen.queryByLabelText("unsaved changes")).toBeNull();
  });

  it("rename_through_command_sends_rename", async () => {
    const { host } = await browser();
    fireEvent.click(screen.getByText("main.scene.ron"));

    await act(() => host.editor.commands.run("assets.rename"));
    const input = screen.getByRole("textbox", { name: "New name" }) as HTMLInputElement;
    expect(input.value).toBe("main.scene.ron");
    fireEvent.change(input, { target: { value: "intro.scene.ron" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await settle();

    expect(host.project.sent).toContainEqual({ type: "rename", from: SCENE, to: "scenes/intro.scene.ron" });
    expect(host.project.sent.filter((request) => request.type === "list_files")).toHaveLength(2);
  });

  it("rename_by_keyboard_only", async () => {
    const { host } = await browser();
    fireEvent.focus(screen.getByText("main.scene.ron").closest("[role=treeitem]")!);

    await act(() => host.editor.commands.run("assets.rename"));
    const input = screen.getByRole("textbox", { name: "New name" });
    fireEvent.change(input, { target: { value: "intro.scene.ron" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await settle();

    expect(host.project.sent).toContainEqual({ type: "rename", from: SCENE, to: "scenes/intro.scene.ron" });
  });

  it("scene_opened_before_the_build_loads_into_each_game", async () => {
    const { host, game, disk } = await browser({ text: "(scene)" }, false);
    await openScene();
    expect(screen.queryByText(/no game/)).toBeNull();

    act(() => host.setControl(game.client));
    await settle();
    expect(game.sent.filter((command) => command.type === "load_scene")).toEqual([{ type: "load_scene", scene: "(scene)", assets: [], document: "Scene" }]);

    disk.text = "(edited without a game)";
    act(() => host.setControl(null));
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: SCENE, kind: "Scene" } }] }));
    await settle();
    const restarted = await testGame();
    act(() => host.setControl(restarted.client));
    await settle();
    expect(restarted.sent.filter((command) => command.type === "load_scene")).toEqual([{ type: "load_scene", scene: "(edited without a game)", assets: [], document: "Scene" }]);
  });

  it("removing_the_open_scene_on_disk_closes_it", async () => {
    const { host } = await browser();
    await openScene();
    setSceneDirty(host, true);

    act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }, { Created: { path: "scenes/moved.scene.ron", kind: "Scene" } }] }));
    await settle();

    expect(screen.queryByLabelText("unsaved changes")).toBeNull();
    await expect(host.editor.commands.run("scene.save")).rejects.toThrow("no scene is open");
  });

  it("delete_asks_for_confirmation_inside_the_panel", async () => {
    const { host } = await browser();
    fireEvent.contextMenu(screen.getByText("logo.png"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    expect(host.project.sent.filter((request) => request.type === "delete")).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: "Delete assets/logo.png" }));
    await settle();
    expect(host.project.sent).toContainEqual({ type: "delete", path: "assets/logo.png" });
  });

  it("dropping_a_file_on_a_folder_moves_it", async () => {
    const { host } = await browser();
    fireEvent.dragStart(screen.getByText("logo.png"), { dataTransfer: { setData() {} } });
    fireEvent.drop(screen.getByText("scenes/"));
    await settle();
    expect(host.project.sent).toContainEqual({ type: "rename", from: "assets/logo.png", to: "scenes/logo.png" });
  });

  it("delete_key_in_the_tree_asks_for_confirmation", async () => {
    const { host } = await browser();
    fireEvent.keyDown(screen.getByText("logo.png").closest("[role=treeitem]")!, { key: "Delete" });
    expect(host.project.sent.filter((request) => request.type === "delete")).toEqual([]);
    expect(screen.getByRole("button", { name: "Delete assets/logo.png" })).toBeTruthy();
  });

  it("a_listing_failure_is_shown", async () => {
    const host = await testEditor(builtinPlugins, (request) => (request.type === "list_files" ? { error: { code: "no_project", message: "no project is open" } } : undefined));
    renderPanel(host, "ludenea.assets");
    await settle();
    expect(screen.getByText("no project is open")).toBeTruthy();
  });

  it("renaming_the_open_scene_keeps_it_open_when_the_removal_arrives_first", async () => {
    let push: (event: object) => void = () => {};
    const host = await testEditor(builtinPlugins, (request) => {
      if (request.type === "list_files") return { type: "files", entries };
      if (request.type === "read_text") return { type: "text", text: "(scene)" };
      // The watcher reports the move before the service answers the rename.
      if (request.type === "rename") push({ type: "files_changed", changes: [{ Removed: { path: request.from, kind: "Scene" } }, { Created: { path: request.to, kind: "Scene" } }] });
      if (request.type === "info") return undefined;
      return { type: "done" };
    });
    push = host.project.push;
    const game = await testGame((command) => (command.type === "save_scene" ? { type: "scene", ron: "(saved)" } : { type: "done" }));
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.assets");
    await settle();
    await openScene();

    await act(() => host.editor.commands.run("assets.rename"));
    const input = screen.getByRole("textbox", { name: "New name" });
    fireEvent.change(input, { target: { value: "intro.scene.ron" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await settle();

    await host.editor.commands.run("scene.save");
    expect(host.project.sent).toContainEqual({ type: "write_text", path: "scenes/intro.scene.ron", text: "(saved)" });
  });

  it("scroll_position_survives_hiding_the_panel", async () => {
    const { host } = await browser();
    const panel = () => document.querySelector(".ludenea-assets") as HTMLElement;
    panel().scrollTop = 120;
    fireEvent.scroll(panel());

    cleanup();
    renderPanel(host, "ludenea.assets");
    expect(panel().scrollTop).toBe(120);
  });
});
