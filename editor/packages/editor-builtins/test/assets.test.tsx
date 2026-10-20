import type { TypeSchema } from "@ludenea/control-client";
import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, setSceneDirty, settle, testEditor, testGame } from "./support";

const SCENE = "scenes/main.scene.ron";
const entries = [
  { path: SCENE, id: "0000000000000000000000000000000a", kind: "Scene" },
  { path: "assets/logo.png", id: "0000000000000000000000000000000b", kind: "Other" },
];

const SETTINGS = "0000000000000000000000000000000c";

/**
 * The editor with every built-in plugin, a game, and the asset browser showing two files, or
 * `disk.files` once set. Each file holds `disk.text`, unless `disk.texts` has its own; a write
 * goes there.
 */
async function browser(disk: { text: string; files?: typeof entries; texts?: { [path: string]: string }; storeError?: string } = { text: "(on disk)" }, withGame = true) {
  const host = await testEditor(builtinPlugins, (request) => {
    const path = request.path as string;
    if (request.type === "list_files") return { type: "files", entries: disk.files ?? entries };
    if (request.type === "read_text") return { type: "text", text: disk.texts?.[path] ?? disk.text };
    if (request.type === "write_text") disk.texts = { ...disk.texts, [path]: request.text as string };
    if (request.type === "read_asset_texts") return { type: "asset_texts", assets: [], paths: [] };
    if (request.type === "load_stored_session") return { type: "stored_session", bytes: null };
    if (request.type === "store_session" && disk.storeError) return { error: { code: "io", message: disk.storeError } };
    if (request.type === "info") return undefined;
    return { type: "done" };
  });
  const game = await testGame((command) => (command.type === "save_scene" ? { type: "scene", ron: "(saved)" } : { type: "done" }));
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

  describe("assets changed on disk", () => {
    const BALL = "0000000000000000000000000000000d";
    const OTHER = "0000000000000000000000000000000e";
    const paths = [
      [BALL, "assets/ball.prefab.ron"],
      [SETTINGS, "assets/settings.ron"],
    ];
    const files = [...entries, { path: "scenes/other.scene.ron", id: OTHER, kind: "Scene" }];
    const timeline = (sceneDirty: boolean) => ({ head: 0, simulated: 0, current: 0, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, undo_label: null, redo_label: null, scene_dirty: sceneDirty, unsaved_assets: [], resimulating: false, branch: 0, branches: [], compare: null });

    /**
     * The open scene with an unsaved tick 0 edit, over a project whose asset texts `disk` holds.
     * The game keeps its unsaved state as the host does: `ReplaceScene` marks its scene saved,
     * `LoadSession` keeps the session's.
     */
    async function project() {
      const disk = { ball: "(ball)", settings: "(gravity_scale: 0.5)" };
      const texts = () => [
        [BALL, disk.ball],
        [SETTINGS, `AssetFile(type: "demo::Settings", value: ${disk.settings})`],
      ];
      const sessions = new Map<string, string>();
      let dirty = false;
      let scene = "";
      const changedTo = (next: boolean) => {
        dirty = next;
        setTimeout(() => game.push({ type: "timeline_changed", timeline: timeline(dirty) }));
      };
      const game = await testGame((command) => {
        // The host refuses the broken data asset, naming it by identifier.
        const broken = (command.assets as string[][] | undefined)?.some(([, text]) => text!.includes("oops"));
        if (broken) return { error: { code: "invalid_scene", message: `asset ${SETTINGS} is invalid: 1:17: Expected float` } };
        if (command.type === "export_session") return { type: "session", scene: dirty ? "(edited)" : scene, saved_scene: dirty ? scene : null, inputs: "", edits: "", undo: "", hashes: ["0000000000000001"], assets: [], saved_assets: [], branches: "", document: "Scene", keeps_assets: false };
        if (command.type === "get_timeline") return { type: "timeline", ...timeline(dirty) };
        if (command.type === "load_scene") {
          scene = command.scene as string;
          changedTo(false);
        }
        if (command.type === "replace_scene") changedTo(false);
        if (command.type === "load_session") {
          scene = (command.saved_scene ?? command.scene) as string;
          changedTo(command.saved_scene !== null);
        }
        return { type: "done" };
      });
      const host = await testEditor(builtinPlugins, (request) => {
        if (request.type === "list_files") return { type: "files", entries: files };
        if (request.type === "read_text") return { type: "text", text: `(${request.path})` };
        if (request.type === "read_asset_texts") return { type: "asset_texts", assets: texts(), paths };
        if (request.type === "store_session") sessions.set(request.document as string, request.bytes as string);
        if (request.type === "load_stored_session") return { type: "stored_session", bytes: sessions.get(request.document as string) ?? null };
        if (request.type === "info") return undefined;
        return { type: "done" };
      });
      act(() => host.setControl(game.client));
      renderPanel(host, "ludenea.assets");
      renderPanel(host, "ludenea.console");
      await settle();
      await openScene();
      // A tick 0 edit the user has not saved.
      act(() => changedTo(true));
      await settle();
      const changed = async (path: string, kind: object | string) => {
        act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path, kind } }] }));
        await settle();
        await settle();
      };
      return { host, game, disk, texts, changed };
    }

    it("a_prefab_change_reloads_the_open_scene_and_keeps_its_unsaved_edits", async () => {
      const { host, game, disk, texts, changed } = await project();
      expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();

      disk.ball = "(bigger ball)";
      await changed("assets/ball.prefab.ron", "Prefab");

      expect(game.sent.filter((command) => command.type === "load_session")).toMatchObject([{ scene: "(edited)", saved_scene: `(${SCENE})`, assets: texts() }]);
      expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([]);
      expect(host.editor.timeline.getState().timeline?.scene_dirty).toBe(true);
      expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();

      // Away and back: the stored session still holds the edited scene.
      await act(() => host.editor.document.getState().open(OTHER));
      game.sent.length = 0;
      await act(() => host.editor.document.getState().open("0000000000000000000000000000000a"));
      await settle();
      expect(game.sent.find((command) => command.type === "load_session")).toMatchObject({ scene: "(edited)", saved_scene: `(${SCENE})` });
    });

    it("a_data_asset_broken_on_disk_is_refused_and_logged_at_its_file_line_and_column", async () => {
      const { host, game, disk, changed } = await project();
      const before = `AssetFile(type: "demo::Settings", value: ${disk.settings})`;

      disk.settings = "(gravity_scale: oops)";
      await changed("assets/settings.ron", { Data: { type: "demo::Settings" } });

      expect(game.sent.filter((command) => command.type === "load_session")).toHaveLength(1);
      expect(screen.getByRole("button", { name: "assets/settings.ron:1:17" })).toBeTruthy();
      expect(screen.getByRole("list", { name: "Console" }).textContent).toContain("Expected float");
      // The game keeps the previous value, and so does every game started next.
      const next = await testGame();
      act(() => host.setControl(next.client));
      await settle();
      expect(next.sent.find((command) => command.type === "load_scene")?.assets).toContainEqual([SETTINGS, before]);
    });
  });

  it("unsaved_marker_follows_scene_dirty", async () => {
    const { host } = await browser();
    await openScene();
    expect(screen.queryByRole("img", { name: "unsaved changes" })).toBeNull();

    setSceneDirty(host, true);
    expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();
    setSceneDirty(host, false);
    expect(screen.queryByRole("img", { name: "unsaved changes" })).toBeNull();
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

  describe("the open scene moved or removed outside the editor", () => {
    const SCENE_ID = entries[0]!.id;
    const LEVEL = "scenes/level.scene.ron";
    const writes = (host: { project: { sent: { type: string }[] } }) => host.project.sent.filter((request) => request.type === "write_text");
    const stores = (host: { project: { sent: { type: string }[] } }) => host.project.sent.filter((request) => request.type === "store_session");

    it("an_open_scene_renamed_outside_with_its_meta_stays_open_at_its_new_path", async () => {
      const { host, disk } = await browser();
      await openScene();
      setSceneDirty(host, true);

      // Moved with its meta: the listing has the scene's identifier at the new path.
      disk.files = [{ ...entries[0]!, path: LEVEL }, entries[1]!];
      act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }, { Created: { path: LEVEL, kind: "Scene" } }] }));
      await settle();

      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: LEVEL });
      expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();
      expect(screen.queryByRole("alert")).toBeNull();
      await host.editor.commands.run("scene.save");
      expect(writes(host)).toEqual([{ type: "write_text", path: LEVEL, text: "(saved)" }]);
    });

    it("an_open_scene_renamed_outside_without_its_meta_follows_the_file_holding_its_text", async () => {
      const LEVEL_ID = "0000000000000000000000000000001a";
      const COPY = "scenes/copy.scene.ron";
      const { host, disk } = await browser();
      await openScene();

      // The meta stays behind, so the moved file gets an identifier of its own; another scene
      // created at the same time holds other text.
      disk.files = [{ path: COPY, id: "0000000000000000000000000000001b", kind: "Scene" }, { path: LEVEL, id: LEVEL_ID, kind: "Scene" }, entries[1]!];
      disk.texts = { [COPY]: "(another scene)" };
      act(() =>
        host.project.push({
          type: "files_changed",
          changes: [{ Removed: { path: SCENE, kind: "Scene" } }, { Created: { path: COPY, kind: "Scene" } }, { Created: { path: LEVEL, kind: "Scene" } }],
        }),
      );
      await settle();
      await settle();

      expect(host.editor.document.getState().current).toEqual({ id: LEVEL_ID, kind: "scene", path: LEVEL });
      // Stored under the new identifier at once, where the next open and crash recovery look.
      expect(stores(host)).toEqual([expect.objectContaining({ document: LEVEL_ID })]);
      expect(screen.queryByRole("alert")).toBeNull();
    });

    it("a_session_that_cannot_be_stored_under_the_moved_file_s_identifier_is_logged_with_both_paths", async () => {
      const LEVEL_ID = "0000000000000000000000000000001a";
      const { host, disk } = await browser();
      await openScene();

      // Moved without its meta: the session is stored under the new file's identifier.
      disk.files = [{ path: LEVEL, id: LEVEL_ID, kind: "Scene" }, entries[1]!];
      disk.storeError = "permission denied";
      act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }, { Created: { path: LEVEL, kind: "Scene" } }] }));
      await settle();
      await settle();

      expect(host.editor.document.getState().current).toEqual({ id: LEVEL_ID, kind: "scene", path: LEVEL });
      expect(host.editor.console.list().map(({ level, text }) => [level, text])).toContainEqual([
        "warning",
        `Cannot store the session of ${SCENE}, moved to ${LEVEL}, under its new identifier: permission denied`,
      ]);
      expect(screen.queryByText("permission denied")).toBeNull();
    });

    it("a_dirty_scene_removed_on_disk_stays_open_with_a_banner", async () => {
      const { host, game, disk } = await browser();
      await openScene();
      setSceneDirty(host, true);

      disk.files = [entries[1]!];
      act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }] }));
      await settle();

      expect(stores(host)).toEqual([expect.objectContaining({ document: SCENE_ID })]);
      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: SCENE });
      const banner = screen.getByRole("alert");
      expect(banner.textContent).toContain(`${SCENE} was deleted on disk`);

      fireEvent.click(within(banner).getByRole("button", { name: "Save again" }));
      await settle();
      expect(writes(host)).toEqual([{ type: "write_text", path: SCENE, text: "(saved)" }]);
      expect(game.sent).toContainEqual({ type: "mark_scene_saved", text: "(saved)" });

      // The watcher reports the file back: the scene is in place again.
      disk.files = entries;
      act(() => host.project.push({ type: "files_changed", changes: [{ Created: { path: SCENE, kind: "Scene" } }] }));
      await settle();
      expect(screen.queryByRole("alert")).toBeNull();
      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: SCENE });
    });

    // A checkout removes every file before it writes them back, so the watcher can report the
    // removal and the creation of a moved scene in two batches.
    const removeThenCreate = async (host: Awaited<ReturnType<typeof browser>>["host"], disk: Awaited<ReturnType<typeof browser>>["disk"], level: (typeof entries)[number]) => {
      disk.files = [entries[1]!];
      act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }] }));
      await settle();
      expect(screen.getByRole("alert").textContent).toContain(`${SCENE} was deleted on disk`);
      disk.files = [level, entries[1]!];
      act(() => host.project.push({ type: "files_changed", changes: [{ Created: { path: LEVEL, kind: "Scene" } }] }));
      await settle();
      await settle();
    };

    it("a_scene_moved_with_its_meta_across_two_batches_follows_its_identifier", async () => {
      const { host, disk } = await browser();
      await openScene();
      setSceneDirty(host, true);

      await removeThenCreate(host, disk, { ...entries[0]!, path: LEVEL });

      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: LEVEL });
      expect(screen.queryByRole("alert")).toBeNull();
      await host.editor.commands.run("scene.save");
      expect(writes(host)).toEqual([{ type: "write_text", path: LEVEL, text: "(saved)" }]);
    });

    it("a_scene_moved_without_its_meta_across_two_batches_follows_the_file_holding_its_text", async () => {
      const LEVEL_ID = "0000000000000000000000000000001a";
      const { host, disk } = await browser();
      await openScene();

      await removeThenCreate(host, disk, { path: LEVEL, id: LEVEL_ID, kind: "Scene" });

      expect(host.editor.document.getState().current).toEqual({ id: LEVEL_ID, kind: "scene", path: LEVEL });
      // Stored on removal, then under the new identifier.
      expect(stores(host)).toEqual([expect.objectContaining({ document: SCENE_ID }), expect.objectContaining({ document: LEVEL_ID })]);
      expect(screen.queryByRole("alert")).toBeNull();
    });

    it("a_dirty_scene_moved_and_changed_across_two_batches_shows_the_change_on_disk", async () => {
      const { host, game, disk } = await browser();
      await openScene();
      setSceneDirty(host, true);
      disk.texts = { [LEVEL]: "(checked out)" };

      await removeThenCreate(host, disk, { ...entries[0]!, path: LEVEL });

      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: LEVEL });
      expect(screen.getByRole("alert").textContent).toContain(`${LEVEL} changed on disk`);
      expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([]);
    });

    // A file manager copies a file with its meta as "main copy.scene.ron", listed before the
    // original: both have the scene's identifier.
    const COPY = "scenes/main copy.scene.ron";

    it("a_copy_sharing_the_open_scenes_identifier_leaves_it_at_its_own_file", async () => {
      const { host, disk } = await browser();
      await openScene();
      setSceneDirty(host, true);

      disk.files = [{ ...entries[0]!, path: COPY }, ...entries];
      act(() => host.project.push({ type: "files_changed", changes: [{ Created: { path: COPY, kind: "Scene" } }] }));
      await settle();
      // A later batch, about any file, does not move it either.
      act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: "assets/logo.png", kind: "Other" } }] }));
      await settle();

      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: SCENE });
      expect(screen.queryByRole("alert")).toBeNull();
      await host.editor.commands.run("scene.save");
      expect(writes(host)).toEqual([{ type: "write_text", path: SCENE, text: "(saved)" }]);
    });

    it("a_removed_scene_follows_none_of_several_files_sharing_its_identifier", async () => {
      const { host, disk } = await browser();
      await openScene();
      setSceneDirty(host, true);

      // Moved, then copied with its meta: two files have its identifier, and its text.
      disk.files = [{ ...entries[0]!, path: COPY }, { ...entries[0]!, path: LEVEL }, entries[1]!];
      act(() =>
        host.project.push({
          type: "files_changed",
          changes: [{ Removed: { path: SCENE, kind: "Scene" } }, { Created: { path: COPY, kind: "Scene" } }, { Created: { path: LEVEL, kind: "Scene" } }],
        }),
      );
      await settle();
      await settle();

      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: SCENE });
      expect(stores(host)).toEqual([expect.objectContaining({ document: SCENE_ID })]);
      expect(screen.getByRole("alert").textContent).toContain(`${SCENE} was deleted on disk`);
    });

    it("a_scene_removed_on_disk_whose_session_cannot_be_stored_keeps_its_banner_and_says_why", async () => {
      const { host, disk } = await browser();
      await openScene();
      disk.files = [entries[1]!];
      disk.storeError = "permission denied";
      act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }] }));
      await settle();

      expect(host.editor.console.list().map(({ level, text }) => [level, text])).toContainEqual(["warning", `Cannot store the session of ${SCENE}, deleted on disk: permission denied`]);
      expect(screen.getByRole("alert").textContent).toContain(`${SCENE} was deleted on disk`);
      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: SCENE });
    });

    it("closing_a_scene_removed_on_disk_whose_session_cannot_be_stored_keeps_it_open", async () => {
      const { host, disk } = await browser();
      await openScene();
      disk.files = [entries[1]!];
      act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }] }));
      await settle();
      disk.storeError = "permission denied";

      fireEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "Close" }));
      await settle();

      expect(host.editor.document.getState().current).toEqual({ id: SCENE_ID, kind: "scene", path: SCENE });
      expect(screen.getByText(`${SCENE} stays open: its session cannot be stored: permission denied`)).toBeTruthy();
      expect(screen.getByRole("alert").textContent).toContain(`${SCENE} was deleted on disk`);
    });

    it("the_banner_of_a_scene_removed_on_disk_goes_once_another_document_is_open", async () => {
      const { host, disk } = await browser();
      await openScene();
      disk.files = [entries[1]!];
      act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }] }));
      await settle();
      expect(screen.getByRole("alert").textContent).toContain(`${SCENE} was deleted on disk`);

      act(() => host.editor.document.setState({ current: { id: "0000000000000000000000000000001b", kind: "scene", path: LEVEL } }));

      expect(screen.queryByRole("alert")).toBeNull();
    });

    it("closing_a_scene_removed_on_disk_stores_its_session_first", async () => {
      const { host, disk } = await browser();
      await openScene();
      disk.files = [entries[1]!];
      act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: SCENE, kind: "Scene" } }] }));
      await settle();

      fireEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "Close" }));
      await settle();

      // Stored on removal, and again with what changed since.
      expect(stores(host)).toHaveLength(2);
      expect(host.editor.document.getState().current).toBeNull();
      expect(screen.queryByRole("alert")).toBeNull();
      // Saving now advises nothing that would drop the game's session.
      await host.editor.commands.run("scene.save");
      expect(writes(host)).toEqual([]);
      expect(host.editor.console.list().at(-1)?.text).toBe("Nothing saved: no scene or prefab is open, so the game's session has no file to go to.");
    });
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

describe("data assets, replays, traces, and snapshots", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

  const SWORD = "00000000000000000000000000000020";
  const RUN = "00000000000000000000000000000021";
  const settingsSchema: TypeSchema = { kind: "struct", name: "demo::Settings", fields: [{ name: "gravity_scale", schema: { kind: "f32" } }] };
  const weaponSchema: TypeSchema = {
    kind: "struct",
    name: "demo::Weapon",
    fields: [
      { name: "damage", schema: { kind: "u32" } },
      { name: "cooldown_ticks", schema: { kind: "u32" } },
    ],
  };
  const spellSchema: TypeSchema = {
    kind: "struct",
    name: "demo::Spell",
    fields: [
      {
        name: "kind",
        schema: {
          kind: "enum",
          name: "demo::SpellKind",
          variants: [
            { name: "Heal", fields: { kind: "unit" } },
            { name: "Bolt", fields: { kind: "struct", fields: [{ name: "power", schema: { kind: "u32" } }] } },
          ],
        },
      },
      { name: "name", schema: { kind: "string" } },
    ],
  };
  const files = [
    ...entries,
    { path: "assets/settings.ron", id: SETTINGS, kind: { Data: { type: "demo::Settings" } } },
    { path: "assets/sword.ron", id: SWORD, kind: { Data: { type: "demo::Weapon" } } },
    { path: "assets/run.replay", id: RUN, kind: "Replay" },
    { path: "assets/run.trace", id: "00000000000000000000000000000022", kind: "Trace" },
    { path: "assets/tick.snap", id: "00000000000000000000000000000023", kind: "Snapshot" },
  ];
  const timeline = (current: number, unsaved: string[] = []) => ({ head: 100, simulated: 100, current, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, undo_label: null, redo_label: null, scene_dirty: false, unsaved_assets: unsaved, resimulating: false, branch: 0, branches: [], compare: null });

  /** The asset browser over `files`, with a game whose asset types are Settings (with a default) and Weapon (without). */
  const disk = [
    [SETTINGS, "(disk settings)"],
    [SWORD, "(disk sword)"],
  ];

  async function project(options: { current?: number; answer?: (command: { type: string; [key: string]: unknown }) => object | undefined } = {}) {
    const { current = 0, answer = () => undefined } = options;
    const onDisk = { scene: "(scene)", files: {} as { [path: string]: string }, unwritable: false, removed: [] as string[] };
    // The assets the game holds unsaved, as the host keeps them: a tick 0 edit or "Apply to asset"
    // marks one, `MarkAssetSaved` clears it, and `LoadSession` says which stay. While `late` is
    // set, the game reports its timeline only once released, as a game busy elsewhere does.
    let unsaved: string[] = [];
    const reports: (() => void)[] = [];
    const late = {
      on: false,
      release() {
        late.on = false;
        for (const report of reports.splice(0)) report();
      },
    };
    const unsavedNow = (next: string[]) => {
      unsaved = next;
      const report = () => game.push({ type: "timeline_changed", timeline: timeline(current, unsaved) });
      if (late.on) reports.push(report);
      else setTimeout(report);
    };
    const game = await testGame(
      (command) => {
        const id = command.id as string;
        if ((command.type === "set_asset_field" && current === 0) || command.type === "apply_to_asset") unsavedNow([...unsaved.filter((other) => other !== id), id]);
        if (command.type === "mark_asset_saved") unsavedNow(unsaved.filter((other) => other !== id));
        if (command.type === "load_session") unsavedNow((command.saved_assets as [string, string][]).map(([asset]) => asset));
        const reply = answer(command);
        if (reply) return reply;
        if (command.type === "get_timeline") return { type: "timeline", ...timeline(current, unsaved) };
        if (command.type === "get_hash") return { type: "hash", tick: current, hash: "00000000000000aa", pending: false };
        if (command.type === "get_resources") return { type: "resources", resources: {} };
        if (command.type === "get_asset" && command.id === SWORD) return { type: "asset", asset: "demo::Weapon", value: { type: "json", value: { damage: 1, cooldown_ticks: 2 } } };
        if (command.type === "get_asset") return { type: "asset", asset: "demo::Settings", value: { type: "json", value: { gravity_scale: 0.5 } } };
        if (command.type === "export_session") {
          const assets = [
            [SETTINGS, "(session settings)"],
            [SWORD, "(session sword)"],
          ];
          const saved_assets = disk.filter(([asset]) => unsaved.includes(asset!));
          return { type: "session", scene: "(scene)", saved_scene: null, inputs: "", edits: "", undo: "", hashes: ["0000000000000001"], assets, saved_assets, branches: "", document: "Scene", keeps_assets: false };
        }
        if (command.type === "save_asset") return { type: "text", text: "(gravity_scale: 2.0)" };
        if (command.type === "load_replay") return { type: "replay_loaded", fingerprint_differs: false };
        if (command.type === "new_asset_value") {
          if (command.asset !== "demo::Settings") return { error: { code: "invalid_command", message: "asset type `demo::Weapon` has no default value; register it with `World::register_asset_with_default`" } };
          return { type: "text", text: "(gravity_scale: 1.0)" };
        }
        return { type: "done" };
      },
      {
        assets: [
          { name: "demo::Settings", schema: settingsSchema },
          { name: "demo::Weapon", schema: weaponSchema },
          { name: "demo::Spell", schema: spellSchema },
        ],
      },
    );
    const host = await testEditor(builtinPlugins, (request) => {
      if (request.type === "list_files") return { type: "files", entries: files.filter((file) => !onDisk.removed.includes(file.path)) };
      if (request.type === "read_text") return { type: "text", text: onDisk.files[request.path as string] ?? onDisk.scene };
      if (request.type === "write_text" && onDisk.unwritable) return { error: { code: "io", message: "permission denied" } };
      if (request.type === "write_text") onDisk.files[request.path as string] = request.text as string;
      if (request.type === "read_binary") return { type: "bytes", bytes: "cmVwbGF5" };
      if (request.type === "read_asset_texts") return { type: "asset_texts", assets: disk, paths: [] };
      if (request.type === "load_stored_session") return { type: "stored_session", bytes: null };
      if (request.type === "create_asset") return { type: "file", path: request.path, id: SWORD, kind: request.kind };
      if (request.type === "info") return undefined;
      return { type: "done" };
    });
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.assets");
    await settle();
    return { host, game, onDisk, late };
  }

  const newDataAsset = async (type: string, path: string) => {
    fireEvent.click(screen.getByRole("button", { name: "New data asset" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Asset type" }), { target: { value: type } });
    fireEvent.change(screen.getByRole("textbox", { name: "Path" }), { target: { value: path } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await settle();
  };
  const type = (label: string, value: string) => {
    const input = screen.getByLabelText(label);
    fireEvent.change(input, { target: { value } });
    fireEvent.blur(input);
  };

  it("new_data_asset_lists_game_asset_types", async () => {
    const { host, game } = await project();
    fireEvent.click(screen.getByRole("button", { name: "New data asset" }));
    const options = within(screen.getByRole("combobox", { name: "Asset type" })).getAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual(["demo::Settings", "demo::Weapon", "demo::Spell"]);

    fireEvent.change(screen.getByRole("textbox", { name: "Path" }), { target: { value: "assets/moon.ron" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await settle();
    expect(game.sent).toContainEqual({ type: "new_asset_value", asset: "demo::Settings" });
    expect(host.project.sent).toContainEqual({ type: "create_asset", path: "assets/moon.ron", kind: { Data: { type: "demo::Settings" } }, text: "(gravity_scale: 1.0)" });
  });

  it("new_data_asset_without_default_requires_every_field", async () => {
    const { host } = await project();
    await newDataAsset("demo::Weapon", "assets/sword.ron");

    const create = () => screen.getByRole("button", { name: "Create assets/sword.ron" }) as HTMLButtonElement;
    expect(create().disabled).toBe(true);
    type("damage", "5");
    expect(create().disabled).toBe(true);
    type("cooldown_ticks", "30");
    expect(create().disabled).toBe(false);
    expect(host.project.sent.filter((request) => request.type === "create_asset")).toEqual([]);

    fireEvent.click(create());
    await settle();
    expect(host.project.sent).toContainEqual({ type: "create_asset", path: "assets/sword.ron", kind: { Data: { type: "demo::Weapon" } }, text: "(damage: 5, cooldown_ticks: 30)" });
    expect(screen.queryByRole("button", { name: "Create assets/sword.ron" })).toBeNull();
  });

  it("opening_replay_uses_document_store", async () => {
    const { host, game } = await project();
    fireEvent.doubleClick(screen.getByText("run.replay"));
    await settle();

    expect(host.project.sent).toContainEqual({ type: "read_binary", path: "assets/run.replay" });
    expect(game.sent).toContainEqual({ type: "load_replay", replay: "cmVwbGF5" });
    expect(host.editor.document.getState().current).toEqual({ id: RUN, kind: "replay", path: "assets/run.replay" });
  });

  it("a_replay_keeps_its_data_assets_to_itself", async () => {
    const { host, game } = await project();
    fireEvent.doubleClick(screen.getByText("run.replay"));
    await settle();
    renderPanel(host, "ludenea.inspector");
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();

    // A tick 0 edit changes the replay's asset, not the project's file: never shown unsaved.
    type("gravity_scale", "2");
    await settle();
    expect(host.editor.timeline.getState().timeline?.unsaved_assets).toEqual([SETTINGS]);
    expect(screen.queryByRole("img", { name: "unsaved changes" })).toBeNull();
    await expect(host.editor.commands.run("inspector.save-asset")).rejects.toThrow("assets/run.replay is a replay");
    expect(host.project.sent.filter((request) => request.type === "write_text")).toEqual([]);
  });

  it("a_replay_removed_on_disk_stays_open_and_offers_only_close", async () => {
    const { host, onDisk } = await project();
    fireEvent.doubleClick(screen.getByText("run.replay"));
    await settle();

    onDisk.removed.push("assets/run.replay");
    act(() => host.project.push({ type: "files_changed", changes: [{ Removed: { path: "assets/run.replay", kind: "Replay" } }] }));
    await settle();

    // A replay's file is never written: there is nothing to save again.
    const banner = screen.getByRole("alert");
    expect(banner.textContent).toContain("assets/run.replay was deleted on disk");
    expect(within(banner).getAllByRole("button").map((button) => button.textContent)).toEqual(["Close"]);
    expect(host.editor.document.getState().current).toEqual({ id: RUN, kind: "replay", path: "assets/run.replay" });
  });

  it("opening_trace_runs_divergence_command", async () => {
    const { host } = await project();
    fireEvent.doubleClick(screen.getByText("run.trace"));
    fireEvent.keyDown(screen.getByText("tick.snap").closest("[role=treeitem]")!, { key: "Enter" });
    await settle();

    // The divergence viewer reads each file to compare it.
    const read = host.project.sent.filter((request) => request.type === "read_binary").map((request) => request.path);
    expect(read).toEqual(["assets/run.trace", "assets/tick.snap"]);
    expect(host.editor.document.getState().current).toBeNull();
  });

  it("kind_filter_shows_only_that_kind", async () => {
    await project();
    fireEvent.change(screen.getByRole("combobox", { name: "Kind" }), { target: { value: "Data" } });
    expect(screen.getAllByRole("treeitem").map((item) => item.textContent)).toEqual([expect.stringContaining("settings.ron"), expect.stringContaining("sword.ron")]);
  });

  it("inspector_edits_selected_data_asset", async () => {
    const { host, game } = await project();
    renderPanel(host, "ludenea.inspector");
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();
    expect(host.editor.selection.getState().asset).toBe(SETTINGS);
    expect(screen.queryByRole("img", { name: "unsaved changes" })).toBeNull();

    type("gravity_scale", "2");
    await settle();
    expect(game.sent).toContainEqual({ type: "set_asset_field", id: SETTINGS, path: [{ type: "field", name: "gravity_scale" }], value: 2 });
    expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Apply to asset" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Save asset" }));
    await settle();
    await settle();
    expect(game.sent).toContainEqual({ type: "save_asset", id: SETTINGS });
    expect(host.project.sent).toContainEqual({ type: "write_text", path: "assets/settings.ron", text: "(gravity_scale: 2.0)" });
    // Saved once its file holds the text.
    expect(game.sent.at(-1)).toEqual({ type: "mark_asset_saved", id: SETTINGS, text: "(gravity_scale: 2.0)" });
    expect(screen.queryByRole("img", { name: "unsaved changes" })).toBeNull();
  });

  it("a_failed_asset_write_keeps_the_asset_unsaved", async () => {
    const { host, game, onDisk } = await project();
    renderPanel(host, "ludenea.inspector");
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();
    type("gravity_scale", "2");
    await settle();
    onDisk.unwritable = true;

    await expect(host.editor.commands.run("inspector.save-asset")).rejects.toThrow("permission denied");
    await settle();

    expect(game.sent.filter((command) => command.type === "mark_asset_saved")).toEqual([]);
    expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();
  });

  it("the_unsaved_marker_follows_the_game", async () => {
    const { host, game } = await project();
    renderPanel(host, "ludenea.inspector");
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();

    // An undo, a redo, or another client: the game says which assets differ from their file.
    act(() => game.push({ type: "timeline_changed", timeline: timeline(0, [SETTINGS]) }));
    await settle();
    expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();
    act(() => game.push({ type: "timeline_changed", timeline: timeline(0) }));
    await settle();
    expect(screen.queryByRole("img", { name: "unsaved changes" })).toBeNull();
  });

  it("apply_to_asset_shown_past_tick_0", async () => {
    const { host, game } = await project({ current: 40 });
    renderPanel(host, "ludenea.inspector");
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();

    type("gravity_scale", "2");
    await settle();
    expect(game.sent).toContainEqual({ type: "set_asset_field", id: SETTINGS, path: [{ type: "field", name: "gravity_scale" }], value: 2 });
    // Past tick 0 the edit is recorded, the file's text is unchanged.
    expect(screen.queryByRole("img", { name: "unsaved changes" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Apply to asset" }));
    await settle();
    expect(game.sent).toContainEqual({ type: "apply_to_asset", id: SETTINGS });
    expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();
  });

  it("replay_with_other_types_shows_banner_and_divergence", async () => {
    const { host, game } = await project({ answer: (command) => (command.type === "load_replay" ? { type: "replay_loaded", fingerprint_differs: true } : undefined) });
    renderPanel(host, "ludenea.timeline");
    // A divergence reported for the previous document is not the replay's.
    act(() => host.editor.timeline.setState({ divergence: { tick: 3 } }));
    fireEvent.doubleClick(screen.getByText("run.replay"));
    await settle();

    const banner = screen.getByRole("status", { name: "Replay" });
    expect(banner.textContent).toContain("recorded with other game types");
    expect(banner.textContent).not.toContain("tick 3");

    act(() => game.push({ type: "diverged", tick: 12, expected: "0000000000000001", got: "0000000000000002" }));
    await settle();
    expect(screen.getByRole("status", { name: "Replay" }).textContent).toContain("first divergent tick 12");
  });

  it("replay_that_fails_to_decode_names_the_value", async () => {
    const message = "cannot decode the input of tick 7 of the replay: invalid type; was it recorded with other game types?";
    const { host } = await project({ answer: (command) => (command.type === "load_replay" ? { error: { code: "invalid_replay", message } } : undefined) });
    renderPanel(host, "ludenea.console");
    renderPanel(host, "ludenea.timeline");
    await openScene();
    fireEvent.doubleClick(screen.getByText("run.replay"));
    await settle();

    expect(screen.getByText(message)).toBeTruthy();
    expect(screen.getByRole("list", { name: "Console" }).textContent).toContain(`Cannot open assets/run.replay: ${message}`);
    expect(host.editor.document.getState().current).toMatchObject({ kind: "scene", path: SCENE });
    expect(screen.queryByRole("status", { name: "Replay" })).toBeNull();
  });

  it("new_data_asset_requires_the_fields_of_a_chosen_variant", async () => {
    const { host } = await project();
    await newDataAsset("demo::Spell", "assets/bolt.ron");
    const create = () => screen.getByRole("button", { name: "Create assets/bolt.ron" }) as HTMLButtonElement;

    fireEvent.change(screen.getByRole("combobox", { name: "kind" }), { target: { value: "Bolt" } });
    // An empty name is a value, once given.
    type("name", "");
    expect(create().disabled).toBe(true);
    type("kind.Bolt.power", "3");
    expect(create().disabled).toBe(false);

    fireEvent.click(create());
    await settle();
    expect(host.project.sent).toContainEqual({ type: "create_asset", path: "assets/bolt.ron", kind: { Data: { type: "demo::Spell" } }, text: '(kind: Bolt(power: 3), name: "")' });
  });

  /** The open scene, with an unsaved tick 0 edit to each data asset. */
  async function unsavedEdits() {
    const context = await project();
    await openScene();
    renderPanel(context.host, "ludenea.inspector");
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();
    type("gravity_scale", "2");
    await settle();
    fireEvent.click(screen.getByText("sword.ron"));
    await settle();
    type("damage", "7");
    await settle();
    expect(context.host.editor.timeline.getState().timeline?.unsaved_assets).toEqual([SETTINGS, SWORD]);
    const changed = async (path: string, type: string) => {
      act(() => context.host.project.push({ type: "files_changed", changes: [{ Changed: { path, kind: { Data: { type } } } }] }));
      await settle();
      await settle();
    };
    return { ...context, changed };
  }
  const loadSessions = (sent: { type: string; [key: string]: unknown }[]) => sent.filter((command) => command.type === "load_session");

  it("saving_an_asset_keeps_another_assets_unsaved_edit", async () => {
    const { host, game, changed } = await unsavedEdits();
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Save asset" }));
    await settle();
    expect(host.project.sent).toContainEqual({ type: "write_text", path: "assets/settings.ron", text: "(gravity_scale: 2.0)" });

    // The watcher reports the written file: the session reloads it, and keeps the sword's edit.
    await changed("assets/settings.ron", "demo::Settings");
    expect(loadSessions(game.sent).at(-1)).toMatchObject({ assets: [[SETTINGS, "(disk settings)"], [SWORD, "(session sword)"]], saved_assets: [[SWORD, "(disk sword)"]] });
    expect(host.editor.timeline.getState().timeline?.unsaved_assets).toEqual([SWORD]);
    expect(screen.queryByRole("alert")).toBeNull();
    fireEvent.click(screen.getByText("sword.ron"));
    await settle();
    expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();
  });

  it("an_asset_save_reported_on_disk_before_the_game_confirms_it_shows_no_banner", async () => {
    const { host, game, onDisk, late, changed } = await unsavedEdits();
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();
    late.on = true;
    fireEvent.click(screen.getByRole("button", { name: "Save asset" }));
    await settle();
    expect(game.sent).toContainEqual({ type: "mark_asset_saved", id: SETTINGS, text: "(gravity_scale: 2.0)" });

    // The watcher reports the written file while the editor still shows the asset unsaved.
    expect(host.editor.timeline.getState().timeline?.unsaved_assets).toEqual([SETTINGS, SWORD]);
    await changed("assets/settings.ron", "demo::Settings");
    expect(screen.queryByRole("alert")).toBeNull();
    late.release();
    await settle();
    expect(host.editor.timeline.getState().timeline?.unsaved_assets).toEqual([SWORD]);

    // Unsaved again, the file changed by another program shows the banner.
    type("gravity_scale", "3");
    await settle();
    expect(host.editor.timeline.getState().timeline?.unsaved_assets).toEqual([SWORD, SETTINGS]);
    onDisk.files["assets/settings.ron"] = "(gravity_scale: 4.0)";
    await changed("assets/settings.ron", "demo::Settings");
    expect(screen.getByRole("alert").textContent).toContain("assets/settings.ron changed on disk");
  });

  it("a_disk_change_to_an_unsaved_asset_shows_the_banner", async () => {
    const { host, game, changed } = await unsavedEdits();
    await changed("assets/sword.ron", "demo::Weapon");

    expect(screen.getByRole("alert").textContent).toContain("assets/sword.ron changed on disk");
    expect(loadSessions(game.sent).at(-1)).toMatchObject({ assets: [[SETTINGS, "(session settings)"], [SWORD, "(session sword)"]] });

    fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    await settle();
    await settle();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(loadSessions(game.sent).at(-1)).toMatchObject({ assets: [[SETTINGS, "(session settings)"], [SWORD, "(disk sword)"]], saved_assets: [[SETTINGS, "(disk settings)"]] });
    expect(host.editor.timeline.getState().timeline?.unsaved_assets).toEqual([SETTINGS]);
  });

  it("a_scene_and_an_asset_changed_together_each_show_a_banner", async () => {
    const { host, game, onDisk } = await unsavedEdits();
    setSceneDirty(host, true);
    onDisk.scene = "(scene changed)";
    act(() =>
      host.project.push({
        type: "files_changed",
        changes: [{ Changed: { path: SCENE, kind: "Scene" } }, { Changed: { path: "assets/sword.ron", kind: { Data: { type: "demo::Weapon" } } } }],
      }),
    );
    await settle();
    await settle();

    const banners = () => screen.getAllByRole("alert").map((banner) => banner.textContent);
    expect(banners()).toEqual([expect.stringContaining("assets/sword.ron changed on disk"), expect.stringContaining(`${SCENE} changed on disk`)]);
    // Keeping my sword leaves the scene's banner.
    fireEvent.click(within(screen.getAllByRole("alert")[0]!).getByRole("button", { name: "Keep mine" }));
    expect(banners()).toEqual([expect.stringContaining(`${SCENE} changed on disk`)]);
    fireEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "Reload" }));
    await settle();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(game.sent.filter((command) => command.type === "replace_scene").at(-1)).toMatchObject({ scene: "(scene changed)" });
  });

  it("a_scene_changed_on_disk_keeps_an_unsaved_asset_edit", async () => {
    const { host, game, onDisk } = await unsavedEdits();
    onDisk.scene = "(scene changed)";
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: SCENE, kind: "Scene" } }] }));
    await settle();
    await settle();

    // The game keeps the session's text of each unsaved asset.
    expect(game.sent.filter((command) => command.type === "replace_scene")).toEqual([{ type: "replace_scene", scene: "(scene changed)", assets: disk }]);
    expect(host.editor.timeline.getState().timeline?.unsaved_assets).toEqual([SETTINGS, SWORD]);
    expect(screen.getByRole("img", { name: "unsaved changes" })).toBeTruthy();
  });

  it("the_asset_section_refetches_on_changes_not_on_every_tick", async () => {
    const { host, game } = await project({ current: 40 });
    renderPanel(host, "ludenea.inspector");
    fireEvent.click(screen.getByText("settings.ron"));
    await settle();
    const fetches = () => game.sent.filter((command) => command.type === "get_asset").length;
    const before = fetches();

    for (const tick of [41, 42, 43]) act(() => game.push({ type: "ticked", tick, hash: "00000000000000aa", pending: false, playing: true, effective_rate: 60 }));
    await settle();
    expect(fetches()).toBe(before);

    act(() => game.push({ type: "timeline_changed", timeline: { ...timeline(43), undo_label: "Undo edit at tick 20", can_undo: true } }));
    await settle();
    expect(fetches()).toBe(before + 1);

    act(() => game.push({ type: "timeline_changed", timeline: { ...timeline(43), undo_label: "Undo edit at tick 20", can_undo: true, branch: 1 } }));
    await settle();
    expect(fetches()).toBe(before + 2);
  });
});
