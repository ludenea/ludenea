import type { BranchInfo, Timeline } from "@ludenea/control-client";
import { CommandPalette } from "@ludenea/editor-shell/app";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { Profiler } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { builtinPlugins } from "../src";
import { BranchLanes } from "../src/timeline/BranchLanes";
import { renderPanel, settle, testEditor, testGame } from "./support";

type Message = { type: string; [key: string]: unknown };

const branches: BranchInfo[] = [
  { id: 0, name: "Branch 1", parent: null, fork_tick: 0, head: 600, stale: false, edit_ticks: [] },
  { id: 1, name: "Branch 2", parent: 0, fork_tick: 300, head: 500, stale: false, edit_ticks: [300, 320] },
  { id: 2, name: "Branch 3", parent: 1, fork_tick: 400, head: 450, stale: true, edit_ticks: [400] },
  { id: 3, name: "Branch 4", parent: 0, fork_tick: 100, head: 200, stale: false, edit_ticks: [100, 150] },
];
const timeline = (patch: Partial<Timeline> = {}): Timeline => ({
  head: 500,
  simulated: 500,
  current: 350,
  checkpoints: [0],
  edit_ticks: [300, 320],
  can_undo: true,
  can_redo: true,
  undo_label: "Undo edit at tick 320",
  redo_label: "Redo edit at tick 340",
  scene_dirty: false,
  resimulating: false,
  branch: 1,
  branches,
  compare: 0,
  ...patch,
});

async function editorWith(options: { game?: (command: Message) => object | undefined; project?: (request: Message) => object | undefined; timeline?: Partial<Timeline> } = {}) {
  const host = await testEditor(builtinPlugins, options.project);
  const game = await testGame((command) => {
    const reply = options.game?.(command);
    if (reply !== undefined) return reply;
    if (command.type === "get_timeline") return { type: "timeline", ...timeline(options.timeline) };
    if (command.type === "get_hash") return { type: "hash", tick: 350, hash: "00000000000000aa", pending: false };
    return { type: "done" };
  });
  act(() => host.setControl(game.client));
  await settle();
  return { host, game };
}

const sentOf = (sent: Message[], type: string) => sent.filter((message) => message.type === type);
const lane = (name: string) => screen.getAllByRole("listitem").find((item) => item.textContent?.startsWith(name))!;

describe("branches", () => {
  afterEach(cleanup);

  it("lanes_render_one_per_branch_with_fork_points", async () => {
    const { host } = await editorWith();
    renderPanel(host, "ludenea.timeline");
    await settle();

    const lanes = within(screen.getByRole("list", { name: "Branches" })).getAllByRole("listitem");
    // Depth first, each branch under its parent.
    expect(lanes.map((item) => item.textContent?.split(" ").slice(0, 2).join(" "))).toEqual(["Branch 1", "Branch 2", "Branch 3", "Branch 4"]);
    expect(screen.getByLabelText("Branch 2 forks at tick 300")).toBeTruthy();
    expect(screen.getByLabelText("Branch 3 forks at tick 400")).toBeTruthy();
    expect(screen.getByLabelText("Branch 4 forks at tick 100")).toBeTruthy();
    expect(screen.queryByLabelText(/^Branch 1 forks/)).toBeNull();
    // Edit markers on every lane, the current one and the others.
    expect(screen.getByLabelText("Branch 2 edit at tick 320")).toBeTruthy();
    expect(screen.getByLabelText("Branch 4 edit at tick 150")).toBeTruthy();
    expect(screen.getByLabelText("Branch 3 edit at tick 400")).toBeTruthy();
    expect(screen.queryByLabelText(/^Branch 1 edit/)).toBeNull();
    expect(within(lane("Branch 2")).getByRole("button").getAttribute("aria-current")).toBe("true");
    expect(lane("Branch 1").textContent).toContain("compare");
    expect(lane("Branch 3").textContent).toContain("stale");
    expect(lane("Branch 4").textContent).not.toContain("stale");
  });

  it("lanes_render_only_when_the_branches_change", async () => {
    const { host } = await editorWith();
    let renders = 0;
    render(
      <Profiler id="lanes" onRender={() => void renders++}>
        <BranchLanes editor={host.editor} />
      </Profiler>,
    );
    renders = 0;

    // Playing back recorded ticks: every tick a new timeline, the same branches.
    act(() => {
      for (const current of [351, 352, 353]) host.editor.timeline.setState({ timeline: timeline({ current, branches: branches.map((branch) => ({ ...branch })), edit_ticks: [300, 320] }) });
    });
    expect(renders).toBe(0);

    act(() => host.editor.timeline.setState({ timeline: timeline({ branches: branches.map((branch) => (branch.id === 2 ? { ...branch, stale: false } : branch)) }) }));
    expect(renders).toBe(1);
  });

  it("clicking_lane_switches_branch", async () => {
    const { host, game } = await editorWith();
    renderPanel(host, "ludenea.timeline");
    await settle();
    const refreshes = sentOf(game.sent, "get_timeline").length;

    fireEvent.click(within(lane("Branch 4")).getByRole("button"));
    await settle();
    expect(sentOf(game.sent, "switch_branch")).toEqual([{ type: "switch_branch", branch: 3 }]);
    expect(sentOf(game.sent, "get_timeline").length).toBeGreaterThan(refreshes);
  });

  it("delete_confirmation_names_descendants", async () => {
    const { host, game } = await editorWith({ game: (command) => (command.type === "delete_branch" ? { type: "deleted", branches: [1, 2] } : undefined) });
    renderPanel(host, "ludenea.timeline");
    await settle();

    await act(() => host.editor.commands.run("timeline.delete-branch", 1));
    expect(screen.getByRole("alertdialog").textContent).toContain("Delete Branch 2 and its descendants Branch 3?");
    expect(sentOf(game.sent, "delete_branch")).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: "Delete Branch 2" }));
    await settle();
    expect(sentOf(game.sent, "delete_branch")).toEqual([{ type: "delete_branch", branch: 1 }]);
    expect(screen.queryByRole("alertdialog")).toBeNull();

    // From the palette: the current branch, the root never.
    await act(() => host.editor.commands.run("timeline.delete-branch"));
    expect(screen.getByRole("alertdialog").textContent).toContain("Delete Branch 2 and its descendants Branch 3?");
    const picker = screen.getByRole("combobox", { name: "Branch" }) as HTMLSelectElement;
    expect([...picker.options].map((option) => option.text)).toEqual(["Branch 2", "Branch 3", "Branch 4"]);
    fireEvent.change(picker, { target: { value: "3" } });
    expect(screen.getByRole("alertdialog").textContent).toContain("Delete Branch 4?");
  });

  it("save_replay_writes_replay_asset", async () => {
    const { host, game } = await editorWith({
      game: (command) => (command.type === "export_replay" ? { type: "replay", replay: "cmVwbGF5" } : undefined),
      project: (request) => {
        if (request.type === "list_files") return { type: "files", entries: [{ path: "assets/replays/branch-2.replay", id: "a", kind: "Replay" }] };
        if (request.type === "create_asset") return { type: "file", path: request.path, id: "b", kind: "Replay" };
        return { type: "done" };
      },
    });

    await act(() => host.editor.commands.run("timeline.save-replay"));
    expect(sentOf(game.sent, "export_replay")).toEqual([{ type: "export_replay", branch: null }]);
    const path = "assets/replays/branch-2-2.replay";
    // The file and its meta exist before the bytes go in: a refused write leaves no file without a meta.
    expect(host.project.sent.filter((request) => request.type === "create_asset" || request.type === "write_binary")).toEqual([
      { type: "create_asset", path, kind: "Replay", text: "" },
      { type: "write_binary", path, bytes: "cmVwbGF5" },
    ]);
  });

  it("deleting_the_root_by_name_is_refused_before_any_form", async () => {
    const { host, game } = await editorWith();
    renderPanel(host, "ludenea.timeline");
    await settle();

    await expect(host.editor.commands.run("timeline.delete-branch", 0)).rejects.toThrow("the root branch cannot be deleted");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(sentOf(game.sent, "delete_branch")).toEqual([]);
  });

  it("rename_sends_the_new_name", async () => {
    const { host, game } = await editorWith();
    renderPanel(host, "ludenea.timeline");
    await settle();

    await act(() => host.editor.commands.run("timeline.rename-branch", 3, "Jump test"));
    // From the palette: the form names the current branch.
    await act(() => host.editor.commands.run("timeline.rename-branch"));
    const name = screen.getByRole("textbox", { name: "New name" }) as HTMLInputElement;
    expect(name.value).toBe("Branch 2");
    fireEvent.change(name, { target: { value: "Faster" } });
    fireEvent.click(within(screen.getByRole("group", { name: "Rename branch" })).getByRole("button", { name: "Rename" }));
    await settle();

    expect(sentOf(game.sent, "rename_branch")).toEqual([
      { type: "rename_branch", branch: 3, name: "Jump test" },
      { type: "rename_branch", branch: 1, name: "Faster" },
    ]);
    expect(screen.queryByRole("group", { name: "Rename branch" })).toBeNull();
  });

  it("compare_none_turns_the_ghost_off", async () => {
    const { host, game } = await editorWith();
    renderPanel(host, "ludenea.timeline");
    await settle();

    await act(() => host.editor.commands.run("timeline.set-compare-branch"));
    const form = screen.getByRole("group", { name: "Compare branch" });
    fireEvent.change(within(form).getByRole("combobox", { name: "Branch" }), { target: { value: "" } });
    fireEvent.click(within(form).getByRole("button", { name: "Compare" }));
    await settle();
    await act(() => host.editor.commands.run("timeline.set-compare-branch", null));

    expect(sentOf(game.sent, "set_compare_branch")).toEqual([
      { type: "set_compare_branch", branch: null },
      { type: "set_compare_branch", branch: null },
    ]);
  });

  it("save_replay_removes_the_file_when_the_write_fails_and_reports_a_failed_removal", async () => {
    let deletes = 0;
    const { host } = await editorWith({
      game: (command) => (command.type === "export_replay" ? { type: "replay", replay: "cmVwbGF5" } : undefined),
      project: (request) => {
        if (request.type === "list_files") return { type: "files", entries: [] };
        if (request.type === "create_asset") return { type: "file", path: request.path, id: "b", kind: "Replay" };
        if (request.type === "write_binary") return { error: { code: "io", message: "disk full" } };
        if (request.type === "delete") return deletes++ === 0 ? { type: "done" } : { error: { code: "io", message: "read-only" } };
        return { type: "done" };
      },
    });
    renderPanel(host, "ludenea.console");
    const path = "assets/replays/branch-2.replay";

    await expect(host.editor.commands.run("timeline.save-replay")).rejects.toThrow("disk full");
    expect(host.project.sent.filter((request) => request.type === "delete")).toEqual([{ type: "delete", path }]);
    expect(screen.queryByText(/Cannot remove/)).toBeNull();

    await expect(host.editor.commands.run("timeline.save-replay")).rejects.toThrow("disk full");
    await settle();
    expect(screen.getByText(`Cannot remove ${path}, left empty by the failed save: read-only`)).toBeTruthy();
  });

  it("undo_label_names_removed_branches", async () => {
    const { host } = await editorWith({ timeline: { undo_label: "Undo edit at tick 300 (removes Branch 3, Branch 4)" } });
    // jsdom has no layout, hence no `scrollIntoView`.
    Element.prototype.scrollIntoView = vi.fn();
    render(<CommandPalette editor={host.editor} />);
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });

    const options = screen.getAllByRole("option").map((option) => option.textContent);
    expect(options).toContain("Undo edit at tick 300 (removes Branch 3, Branch 4)");
    expect(options).toContain("Redo edit at tick 340");
    delete (Element.prototype as Partial<Element>).scrollIntoView;
  });
});
