import type { DebugDrawFrame, DivergenceReport, Entity, Timeline } from "@ludenea/control-client";
import type { OverlayContext } from "@ludenea/editor-shell";
import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { PluginDefinition } from "@ludenea/editor-shell";
import { divergencePlugin } from "../src/divergence";
import { viewportPlugin } from "../src/viewport";
import { renderPanel, settle, testEditor, testGame } from "./support";

type Message = { type: string; [key: string]: unknown };

const entity = (index: number): Entity => ({ index, generation: 0 });
const timeline = (patch: Partial<Timeline> = {}): Timeline => ({
  head: 500,
  simulated: 500,
  current: 350,
  checkpoints: [0],
  edit_ticks: [300],
  can_undo: true,
  can_redo: false,
  undo_label: null,
  redo_label: null,
  scene_dirty: false,
  resimulating: false,
  branch: 1,
  branches: [
    { id: 0, name: "Branch 1", parent: null, fork_tick: 0, head: 600, stale: false, edit_ticks: [] },
    { id: 1, name: "Branch 2", parent: 0, fork_tick: 300, head: 500, stale: false, edit_ticks: [300] },
  ],
  compare: 0,
  ...patch,
});

const files = [
  { path: "assets/run.trace", id: "1", kind: "Trace" },
  { path: "assets/other.trace", id: "2", kind: "Trace" },
  { path: "assets/run.snap", id: "3", kind: "Snapshot" },
];

const report: DivergenceReport = {
  fork_tick: 300,
  up_to: 400,
  complete: true,
  first_divergent: 301,
  root_cause: { tick: 300, kind: { type: "edit", description: "set ludenea::Pose.position on 4v0" }, entities: [entity(4)] },
  entities: [
    {
      entity: entity(4),
      kind: "changed",
      first_tick: 301,
      first_difference: { subject: "entity 4v0", component: "ludenea::Pose", path: ["position", "x"], left: 1, right: 2, left_text: "1.0", right_text: "2.0" },
      causes: [],
    },
    {
      entity: entity(7),
      kind: "changed",
      first_tick: 320,
      first_difference: null,
      causes: [{ type: "event", tick: 319, event: "ludenea::Collision", path: "b", other: entity(4) }],
    },
  ],
};

async function divergence(options: { game?: (command: Message) => object | undefined; timeline?: Partial<Timeline>; plugins?: PluginDefinition[] } = {}) {
  const host = await testEditor([divergencePlugin, ...(options.plugins ?? [])], (request) => {
    if (request.type === "list_files") return { type: "files", entries: files };
    if (request.type === "read_binary") return { type: "bytes", bytes: `bytes of ${request.path}` };
    return undefined;
  });
  const game = await testGame(
    (command) => {
      const reply = options.game?.(command);
      if (reply !== undefined) return reply;
      if (command.type === "get_timeline") return { type: "timeline", ...timeline(options.timeline) };
      if (command.type === "get_hash") return { type: "hash", tick: 350, hash: "00000000000000aa", pending: false };
      return { type: "done" };
    },
    { debug_layers: ["colliders", "contacts"] },
  );
  act(() => host.setControl(game.client));
  act(() => host.editor.timeline.setState({ timeline: timeline(options.timeline) }));
  await settle();
  return { host, game };
}

const sentOf = (sent: Message[], type: string) => sent.filter((message) => message.type === type);

/** A 2D context recording what is drawn; jsdom has none. */
function fakeContext() {
  const calls: { method: string; args: unknown[] }[] = [];
  const canvas = new Proxy({} as Record<string | symbol, unknown>, {
    get: (target, key) => (key in target ? target[key] : (...args: unknown[]) => void calls.push({ method: String(key), args })),
    set: (target, key, value) => ((target[key] = value), true),
  });
  const context: OverlayContext = { canvas: canvas as unknown as CanvasRenderingContext2D, worldToScreen: (point) => point, tick: 350, selection: [] };
  return { calls, context };
}

const circle: DebugDrawFrame = {
  layers: { colliders: [{ entity: entity(4), primitive: { type: "circle", center: { x: 1, y: 1 }, radius: 0.5, color: 0xffffffff, fill: true } }] },
};

describe("divergence", () => {
  afterEach(cleanup);

  it("trace_strip_marks_first_divergent_tick", async () => {
    const { host, game } = await divergence({ game: (command) => (command.type === "compare_traces" ? { type: "trace_comparison", common: 500, first_divergent: 42, only_in_longer: 10 } : undefined) });
    renderPanel(host, "ludenea.divergence");
    await act(() => host.editor.commands.run("divergence.open-file", { path: "assets/run.trace" }));
    await settle();
    // Against the current branch: no second trace sent.
    expect(sentOf(game.sent, "compare_traces").at(-1)).toEqual({ type: "compare_traces", a: "bytes of assets/run.trace", b: null });
    const marker = await screen.findByRole("button", { name: "First divergent tick 42" });

    // The current session is one side: selecting the tick seeks there.
    fireEvent.click(marker);
    await settle();
    expect(sentOf(game.sent, "seek")).toEqual([{ type: "seek", tick: 42 }]);
    // A trace holds hashes only: the snapshot taken there is the user's to choose, then diffed against the current tick.
    expect(sentOf(game.sent, "diff_snapshots")).toEqual([]);
    fireEvent.change(screen.getByLabelText("Choose a snapshot to compare with tick 42"), { target: { value: "assets/run.snap" } });
    await settle();
    expect(sentOf(game.sent, "diff_snapshots")).toEqual([{ type: "diff_snapshots", a: "bytes of assets/run.snap", b: null }]);
    expect(screen.getByRole("heading", { name: "assets/run.snap" })).toBeTruthy();

    // Two trace files: nothing to seek, the marker stays but does nothing.
    fireEvent.change(screen.getByLabelText("Compare the trace with"), { target: { value: "assets/other.trace" } });
    await settle();
    expect(sentOf(game.sent, "compare_traces").at(-1)).toEqual({ type: "compare_traces", a: "bytes of assets/run.trace", b: "bytes of assets/other.trace" });
    fireEvent.click(screen.getByRole("button", { name: "First divergent tick 42" }));
    await settle();
    expect(sentOf(game.sent, "seek")).toHaveLength(1);
  });

  it("comparisons_with_the_session_follow_it", async () => {
    const { host, game } = await divergence({
      game: (command) => {
        if (command.type === "compare_traces") return { type: "trace_comparison", common: 500, first_divergent: 42, only_in_longer: 10 };
        if (command.type === "diff_snapshots") return { type: "differences", differences: [] };
        return undefined;
      },
    });
    renderPanel(host, "ludenea.divergence");
    await act(() => host.editor.commands.run("divergence.open-file", { path: "assets/run.trace" }));
    await act(() => host.editor.commands.run("divergence.open-file", { path: "assets/run.snap" }));
    await settle();
    const traces = () => sentOf(game.sent, "compare_traces").length;
    const snapshots = () => sentOf(game.sent, "diff_snapshots").length;
    expect([traces(), snapshots()]).toEqual([1, 1]);

    // The session records more ticks: the trace is compared again, its marker kept meanwhile.
    act(() => host.editor.timeline.setState({ timeline: timeline({ simulated: 520, head: 520 }) }));
    expect(screen.getByRole("button", { name: "First divergent tick 42" })).toBeTruthy();
    await settle();
    expect([traces(), snapshots()]).toEqual([2, 1]);
    // Another current tick: the snapshot is compared with it.
    act(() => host.editor.timeline.setState({ timeline: timeline({ simulated: 520, head: 520, current: 360 }) }));
    await settle();
    expect([traces(), snapshots()]).toEqual([2, 2]);
    // While playing, the snapshot waits for the pause.
    act(() => host.editor.timeline.setState({ playing: true, timeline: timeline({ simulated: 520, head: 520, current: 361 }) }));
    act(() => host.editor.timeline.setState({ timeline: timeline({ simulated: 520, head: 520, current: 362 }) }));
    await settle();
    expect(snapshots()).toBe(2);
    act(() => host.editor.timeline.setState({ playing: false }));
    await settle();
    expect(snapshots()).toBe(3);

    // Two files: nothing follows the session.
    fireEvent.change(screen.getByLabelText("Compare the trace with"), { target: { value: "assets/other.trace" } });
    await settle();
    act(() => host.editor.timeline.setState({ timeline: timeline({ simulated: 530, head: 530, current: 360 }) }));
    await settle();
    expect(traces()).toBe(3);
  });

  it("snapshot_diff_groups_by_entity_component_field", async () => {
    const differences = [
      { subject: "entity 3v0", component: "ludenea::Pose", path: ["position", "x"], left: 1, right: 2, left_text: "1.0", right_text: "2.0" },
      { subject: "entity 3v0", component: "ludenea::Pose", path: ["position", "y"], left: 0, right: 5, left_text: "0.0", right_text: "5.0" },
      { subject: "entity 3v0", component: "ludenea_physics::components::Body", path: ["kind"], left: "Static", right: "Dynamic", left_text: "Static", right_text: "Dynamic" },
      { subject: "entity 9v1", component: "ludenea::Pose", path: [], left: null, right: {}, left_text: "absent", right_text: "(position:(x:0.0,y:0.0))" },
    ];
    const { host, game } = await divergence({ game: (command) => (command.type === "diff_snapshots" ? { type: "differences", differences } : undefined) });
    renderPanel(host, "ludenea.divergence");
    await act(() => host.editor.commands.run("divergence.open-file", { path: "assets/run.snap" }));
    await settle();
    expect(sentOf(game.sent, "diff_snapshots").at(-1)).toEqual({ type: "diff_snapshots", a: "bytes of assets/run.snap", b: null });

    const tree = screen.getByRole("tree", { name: "Snapshot differences" });
    const entities = within(tree).getAllByRole("treeitem", { name: /^entity / });
    expect(entities.map((item) => item.getAttribute("aria-label"))).toEqual(["entity 3v0", "entity 9v1"]);
    const pose = within(entities[0]!).getByRole("treeitem", { name: "ludenea::Pose" });
    expect(within(pose).getAllByRole("treeitem").map((field) => field.textContent)).toEqual(["position.x1.02.0", "position.y0.05.0"]);
    // A variant change shows the variant names.
    const body = within(entities[0]!).getByRole("treeitem", { name: "ludenea_physics::components::Body" });
    expect(within(body).getByRole("treeitem").textContent).toBe("kindStaticDynamic");
  });

  it("fingerprint_refusal_is_shown", async () => {
    const message = "snapshot schema fingerprint 00000000000000aa differs from the game's 00000000000000bb";
    const { host } = await divergence({ game: (command) => (command.type === "diff_snapshots" ? { error: { code: "invalid_command", message } } : undefined) });
    renderPanel(host, "ludenea.divergence");
    await act(() => host.editor.commands.run("divergence.open-file", { path: "assets/run.snap" }));
    await settle();
    expect(screen.getByRole("alert").textContent).toBe(message);
  });

  it("causal_report_starts_from_root_cause", async () => {
    const { host, game } = await divergence({ game: (command) => (command.type === "get_divergence" ? { type: "divergence", ...report } : undefined) });
    // Nothing is computed while the panel is closed.
    expect(sentOf(game.sent, "get_divergence")).toEqual([]);
    renderPanel(host, "ludenea.divergence-report");
    await settle();
    expect(sentOf(game.sent, "subscribe")).toContainEqual({ type: "subscribe", topic: { type: "divergence" } });
    expect(sentOf(game.sent, "get_divergence").at(-1)).toEqual({ type: "get_divergence", up_to: null });

    const entries = screen.getAllByRole("treeitem").filter((item) => item.getAttribute("aria-level") === "1");
    expect(entries.map((item) => item.getAttribute("aria-label"))).toEqual(["Root cause at tick 300", "4v0 at tick 301", "7v0 at tick 320"]);
    expect(entries[0]!.textContent).toContain("set ludenea::Pose.position on 4v0");
    expect(entries[1]!.textContent).toContain("no recorded interaction");
    expect(entries[2]!.textContent).toContain("ludenea::Collision");

    // Progress asks for the report again; Extend asks for a larger range.
    act(() => game.push({ type: "divergence_progress", up_to: 450 }));
    await settle();
    expect(sentOf(game.sent, "get_divergence")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Extend" }));
    await settle();
    expect(sentOf(game.sent, "get_divergence").at(-1)).toEqual({ type: "get_divergence", up_to: 1000 });

    // Without a compare branch the report is not asked for: the panel says why.
    act(() => host.editor.timeline.setState({ timeline: timeline({ compare: null }) }));
    await settle();
    const asked = sentOf(game.sent, "get_divergence").length;
    act(() => game.push({ type: "divergence_progress", up_to: 460 }));
    await settle();
    expect(sentOf(game.sent, "get_divergence")).toHaveLength(asked);
    expect(screen.getByText(/No compare branch/)).toBeTruthy();

    cleanup();
    await settle();
    expect(sentOf(game.sent, "unsubscribe")).toContainEqual({ type: "unsubscribe", topic: { type: "divergence" } });
  });

  it("selecting_cause_seeks_and_selects", async () => {
    const { host, game } = await divergence({ game: (command) => (command.type === "get_divergence" ? { type: "divergence", ...report } : undefined) });
    renderPanel(host, "ludenea.divergence-report");
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "4v0 at tick 301" }));
    await settle();
    expect(sentOf(game.sent, "seek").at(-1)).toEqual({ type: "seek", tick: 301 });
    expect(host.editor.selection.getState().entities).toEqual([entity(4)]);

    fireEvent.click(screen.getByRole("button", { name: /ludenea::Collision/ }));
    await settle();
    expect(sentOf(game.sent, "seek").at(-1)).toEqual({ type: "seek", tick: 319 });
    expect(host.editor.selection.getState().entities).toEqual([entity(7), entity(4)]);
  });

  it("explorer_column_shows_divergence_tick", async () => {
    const { host } = await divergence({ game: (command) => (command.type === "get_divergence" ? { type: "divergence", ...report } : undefined) });
    const column = host.editor.explorer.columns.list().find((registered) => registered.id === "divergence.diverged-at")!;
    expect(column.title).toBe("Diverged at");
    // No report yet: empty.
    expect(column.value(entity(4))).toBeNull();

    let notified = 0;
    const stop = column.subscribe!(() => notified++);
    renderPanel(host, "ludenea.divergence-report");
    await settle();
    // The same column, in its place: it notifies the explorer instead of registering again.
    const current = host.editor.explorer.columns.list().find((registered) => registered.id === "divergence.diverged-at")!;
    expect(current).toBe(column);
    expect(notified).toBe(1);
    expect(current.value(entity(4))).toBe("301");
    expect(current.value(entity(5))).toBeNull();

    // Closed, nothing keeps the report current: the column empties.
    cleanup();
    await settle();
    const closed = host.editor.explorer.columns.list().find((registered) => registered.id === "divergence.diverged-at")!;
    expect(closed.value(entity(4))).toBeNull();
    expect(notified).toBe(2);
    stop();
  });

  it("a_report_answered_after_the_last_watcher_left_is_dropped", async () => {
    let answer: (reply: object) => void = () => {};
    const { host } = await divergence({ game: (command) => (command.type === "get_divergence" ? new Promise<object>((resolve) => (answer = resolve)) : undefined) });
    renderPanel(host, "ludenea.divergence-report");
    await settle();
    cleanup();
    await settle();

    answer({ type: "divergence", ...report });
    await settle();
    const column = host.editor.explorer.columns.list().find((registered) => registered.id === "divergence.diverged-at")!;
    expect(column.value(entity(4))).toBeNull();
  });

  it("ghost_overlay_draws_only_with_compare_branch", async () => {
    const { host, game } = await divergence({
      timeline: { compare: null },
      game: (command) => (command.type === "get_ghost_draw" ? { type: "ghost_draw", frame: circle, tick: command.tick, lagging: false } : undefined),
    });
    const overlay = host.editor.overlays.list().find((registered) => registered.id === "divergence.ghost")!;
    const empty = fakeContext();
    overlay.draw(empty.context);
    expect(empty.calls).toEqual([]);
    expect(sentOf(game.sent, "subscribe")).toEqual([]);

    act(() => host.editor.timeline.setState({ timeline: timeline({ compare: 0 }) }));
    await settle();
    expect(sentOf(game.sent, "subscribe")).toContainEqual({ type: "subscribe", topic: { type: "ghost" } });
    expect(sentOf(game.sent, "get_ghost_draw").at(-1)).toEqual({ type: "get_ghost_draw", tick: 350, layers: ["colliders", "contacts"] });
    const drawn = fakeContext();
    overlay.draw(drawn.context);
    expect(drawn.calls.some((call) => call.method === "stroke")).toBe(true);
    expect(drawn.calls.some((call) => call.method === "fill")).toBe(false);
    // The canvas is left as the viewport set it.
    expect(drawn.calls[0]!.method).toBe("save");
    expect(drawn.calls.at(-1)!.method).toBe("restore");

    // The toggle hides it and stops the compare simulation.
    await act(() => host.editor.commands.run("divergence.toggle-ghost"));
    await settle();
    const hidden = fakeContext();
    overlay.draw(hidden.context);
    expect(hidden.calls).toEqual([]);
    expect(sentOf(game.sent, "unsubscribe")).toContainEqual({ type: "unsubscribe", topic: { type: "ghost" } });
  });

  it("lagging_badge_shows_when_ghost_lags", async () => {
    const { host, game } = await divergence({
      game: (command) => (command.type === "get_ghost_draw" ? { type: "ghost_draw", frame: circle, tick: 340, lagging: true } : undefined),
    });
    const overlay = host.editor.overlays.list().find((registered) => registered.id === "divergence.ghost")!;
    await waitFor(() => expect(sentOf(game.sent, "get_ghost_draw").length).toBeGreaterThan(0));
    await settle();
    const drawn = fakeContext();
    overlay.draw(drawn.context);
    expect(drawn.calls.filter((call) => call.method === "fillText").map((call) => call.args[0])).toContain("Ghost lagging at tick 340");
  });

  it("ghost_requests_only_layers_the_viewport_shows", async () => {
    const { host, game } = await divergence({
      plugins: [viewportPlugin],
      game: (command) => (command.type === "get_ghost_draw" ? { type: "ghost_draw", frame: circle, tick: 350, lagging: false } : undefined),
    });
    await act(() => host.editor.commands.run("viewport.toggle-layer.colliders"));
    await settle();
    expect(sentOf(game.sent, "get_ghost_draw").at(-1)).toEqual({ type: "get_ghost_draw", tick: 350, layers: ["contacts"] });
  });
});
