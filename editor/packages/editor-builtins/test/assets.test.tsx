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
async function browser(disk: { text: string } = { text: "(on disk)" }, withGame = true) {
  const host = await testEditor(builtinPlugins, (request) => {
    if (request.type === "list_files") return { type: "files", entries };
    if (request.type === "read_text") return { type: "text", text: disk.text };
    if (request.type === "info") return undefined;
    return { type: "done" };
  });
  const game = await testGame();
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
    expect(game.sent.filter((command) => command.type === "load_scene")).toEqual([{ type: "load_scene", ron: "(scene)" }]);
  });

  it("disk_change_with_unsaved_edits_shows_banner_and_keeps_mine", async () => {
    const { host, game, disk } = await browser();
    await openScene();
    setSceneDirty(host, true);

    disk.text = "(changed)";
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: SCENE }] }));
    await settle();

    expect(screen.getByRole("alert").textContent).toContain("changed on disk");
    fireEvent.click(screen.getByRole("button", { name: "Keep mine" }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([]);

    disk.text = "(changed again)";
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: SCENE }] }));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    await settle();
    expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([{ type: "replace_scene", ron: "(changed again)" }]);
  });

  it("disk_change_without_unsaved_edits_sends_replace_scene", async () => {
    const { host, game, disk } = await browser();
    await openScene();
    setSceneDirty(host, false);

    disk.text = "(changed)";
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: SCENE }, { Created: "assets/new.png" }] }));
    await settle();

    expect(screen.queryByRole("alert")).toBeNull();
    expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([{ type: "replace_scene", ron: "(changed)" }]);
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
    expect(game.sent.filter((command) => command.type === "load_scene")).toEqual([{ type: "load_scene", ron: "(scene)" }]);

    disk.text = "(edited without a game)";
    act(() => host.setControl(null));
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: SCENE }] }));
    await settle();
    const restarted = await testGame();
    act(() => host.setControl(restarted.client));
    await settle();
    expect(restarted.sent.filter((command) => command.type === "load_scene")).toEqual([{ type: "load_scene", ron: "(edited without a game)" }]);
  });

  it("removing_the_open_scene_on_disk_closes_it", async () => {
    const { host } = await browser();
    await openScene();
    setSceneDirty(host, true);

    act(() => host.project.push({ type: "files_changed", changes: [{ Removed: SCENE }, { Created: "scenes/moved.scene.ron" }] }));
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
});
