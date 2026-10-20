import type { Timeline } from "@ludenea/control-client";
import { installShortcuts } from "@ludenea/editor-shell/app";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { Profiler } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { ResimulatingNotice } from "../src/timeline/editing";
import { renderFrame, renderPanel, schemas, settle, testEditor, testGame } from "./support";

type Command = { type: string; [key: string]: unknown };

const entity = { index: 4, generation: 1 };
const timeline = (patch: Partial<Timeline> = {}): Timeline => ({
  head: 100,
  simulated: 100,
  current: 40,
  checkpoints: [0, 50],
  edit_ticks: [20],
  can_undo: true,
  can_redo: false,
  undo_label: "Undo edit at tick 20",
  redo_label: null,
  scene_dirty: false,
  unsaved_assets: [],
  resimulating: false,
  branch: 0,
  branches: [{ id: 0, name: "Branch 1", parent: null, fork_tick: 0, head: 100, stale: false, edit_ticks: [20] }],
  compare: null,
  ...patch,
});
const mod = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? { metaKey: true } : { ctrlKey: true };

/** `answer` replies to a command first; `null` leaves it unanswered. */
async function editorWith(answer: (command: Command) => object | null | undefined = () => undefined) {
  const host = await testEditor(builtinPlugins);
  const game = await testGame(
    (command) => {
      const reply = answer(command);
      if (reply !== undefined) return reply ?? undefined;
      if (command.type === "get_timeline") return { type: "timeline", ...timeline() };
      if (command.type === "get_hash") return { type: "hash", tick: 40, hash: "00000000000000aa", pending: false };
      if (command.type === "get_entity") return { type: "entity", entity, components: { "ludenea::Collider": { type: "json", value: { shape: { Circle: { center: { x: 0, y: 0 }, radius: 1 } }, density: 1 } } } };
      if (command.type === "get_resources") return { type: "resources", resources: {} };
      return { type: "done" };
    },
    { components: [{ name: "ludenea::Collider", schema: schemas.collider }] },
  );
  act(() => host.setControl(game.client));
  await settle();
  return { host, game };
}

const sentOf = (sent: Command[], type: string) => sent.filter((command) => command.type === type);

describe("timeline", () => {
  afterEach(cleanup);

  it("scrubbing_sends_at_most_one_seek_in_flight", async () => {
    const { host, game } = await editorWith();
    renderPanel(host, "ludenea.timeline");
    await settle();
    const scrubber = screen.getByRole("slider", { name: "Tick" });
    expect(scrubber.getAttribute("max")).toBe("100");

    act(() => {
      for (const value of ["10", "20", "30"]) fireEvent.change(scrubber, { target: { value } });
    });
    expect(sentOf(game.sent, "seek")).toEqual([{ type: "seek", tick: 10 }]);
    await settle();
    // The positions passed while the first was in flight collapse into the latest.
    expect(sentOf(game.sent, "seek")).toEqual([
      { type: "seek", tick: 10 },
      { type: "seek", tick: 30 },
    ]);
  });

  it("pending_marker_follows_hash_reply", async () => {
    let pending = false;
    const { host, game } = await editorWith((command) => (command.type === "get_hash" ? { type: "hash", tick: 40, hash: "00000000000000bb", pending } : undefined));
    renderPanel(host, "ludenea.timeline");
    await settle();
    expect(screen.getByLabelText("Hash").textContent).toBe("00000000000000bb");

    pending = true;
    await act(() => host.editor.commands.run("timeline.step"));
    await settle();
    expect(host.editor.timeline.getState().pending).toBe(true);
    expect(screen.getByLabelText("Hash").textContent).toContain("pending");

    // Playing reports recorded ticks only.
    act(() => game.push({ type: "ticked", tick: 41, hash: "00000000000000cc", pending: false, playing: true, effective_rate: 60 }));
    expect(screen.getByLabelText("Hash").textContent).toBe("00000000000000cc");
    expect(screen.getByText(/Tick 41/)).toBeTruthy();
  });

  it("undo_shortcut_runs_undo_and_refreshes_timeline", async () => {
    let undone = false;
    const { host, game } = await editorWith((command) => {
      if (command.type === "undo") undone = true;
      if (command.type === "get_timeline") return { type: "timeline", ...timeline({ can_undo: !undone, can_redo: undone }) };
      return undefined;
    });
    const uninstall = installShortcuts(host.editor, host.registry, window);
    expect(host.editor.timeline.getState().timeline?.can_redo).toBe(false);

    fireEvent.keyDown(document.body, { key: "z", ...mod });
    await settle();
    expect(sentOf(game.sent, "undo")).toHaveLength(1);
    expect(host.editor.timeline.getState().timeline?.can_redo).toBe(true);

    fireEvent.keyDown(document.body, { key: "Z", shiftKey: true, ...mod });
    await settle();
    expect(sentOf(game.sent, "redo")).toHaveLength(1);
    uninstall();
  });

  it("divergence_marker_appears_on_diverged_event", async () => {
    const { host, game } = await editorWith();
    renderPanel(host, "ludenea.timeline");
    await settle();
    expect(screen.queryByLabelText(/^Divergence/)).toBeNull();

    act(() => game.push({ type: "diverged", tick: 30, expected: "0000000000000001", got: "0000000000000002" }));
    expect(screen.getByLabelText("Divergence at tick 30")).toBeTruthy();

    // A new game starts without one.
    const other = await testGame((command) => (command.type === "get_timeline" ? { type: "timeline", ...timeline() } : { type: "done" }));
    act(() => host.setControl(other.client));
    await settle();
    expect(screen.queryByLabelText(/^Divergence/)).toBeNull();
  });

  it("editing_disabled_while_resimulating", async () => {
    const { host, game } = await editorWith((command) => (command.type === "get_timeline" ? { type: "timeline", ...timeline({ resimulating: true, simulated: 60 }) } : undefined));
    act(() => host.editor.selection.getState().select([entity]));
    renderPanel(host, "ludenea.inspector");
    await settle();

    expect(screen.getByRole("status").textContent).toMatch(/Resimulating/);
    expect((screen.getByLabelText("density") as HTMLInputElement).readOnly).toBe(true);
    fireEvent.focusIn(screen.getByRole("region", { name: "ludenea::Collider" }));
    for (const command of ["inspector.remove-component", "inspector.apply-to-scene", "scene.despawn-selection", "inspector.add-component", "viewport.nudge-right", "edit.undo", "edit.redo"]) {
      await expect(host.editor.commands.run(command)).rejects.toThrow(/resimulating/);
    }
    expect(["despawn", "set_field", "undo", "redo"].flatMap((type) => sentOf(game.sent, type))).toEqual([]);
    expect(["remove_component", "apply_to_scene"].flatMap((type) => sentOf(game.sent, type))).toEqual([]);
  });

  it("resimulating_notice_renders_only_when_what_it_shows_changes", async () => {
    const { host } = await editorWith();
    let renders = 0;
    render(
      <Profiler id="notice" onRender={() => void renders++}>
        <ResimulatingNotice editor={host.editor} />
      </Profiler>,
    );
    renders = 0;

    act(() => {
      for (const current of [41, 42, 43]) host.editor.timeline.setState({ timeline: timeline({ current, head: current }) });
    });
    expect(renders).toBe(0);

    act(() => host.editor.timeline.setState({ timeline: timeline({ resimulating: true, simulated: 60 }) }));
    expect(screen.getByRole("status").textContent).toContain("tick 60 of 100");
    expect(renders).toBe(1);
  });

  it("speed_steps_by_powers_of_two", async () => {
    const { host, game } = await editorWith();
    for (const command of ["timeline.speed-down", "timeline.speed-down", "timeline.speed-down", "timeline.speed-down", "timeline.speed-up"]) {
      await host.editor.commands.run(command);
    }
    expect(sentOf(game.sent, "set_speed").map((command) => command.speed)).toEqual([0.5, 0.25, 0.125, 0.125, 0.25]);
  });

  it("scrubber_keeps_the_dragged_position_while_a_seek_is_queued", async () => {
    let current = 40;
    const { host } = await editorWith((command) => {
      if (command.type === "get_timeline") return { type: "timeline", ...timeline({ current }) };
      if (command.type !== "seek") return undefined;
      // The first seek lands; the one queued behind it stays in flight.
      if (command.tick === 20) return null;
      current = command.tick as number;
      return { type: "done" };
    });
    renderPanel(host, "ludenea.timeline");
    await settle();
    const scrubber = screen.getByRole("slider", { name: "Tick" }) as HTMLInputElement;

    act(() => {
      fireEvent.change(scrubber, { target: { value: "10" } });
      fireEvent.change(scrubber, { target: { value: "20" } });
    });
    await settle();
    expect(host.editor.timeline.getState().timeline?.current).toBe(10);
    expect(scrubber.value).toBe("20");
  });

  it("scene_tree_and_viewport_show_the_resimulating_notice", async () => {
    const { host } = await editorWith((command) => {
      if (command.type === "get_timeline") return { type: "timeline", ...timeline({ resimulating: true, simulated: 60 }) };
      if (command.type === "query_entities") return { type: "entities", total: 0, rows: [] };
      if (command.type === "get_render_frame") return renderFrame(40);
      return undefined;
    });
    renderPanel(host, "ludenea.scene-tree");
    renderPanel(host, "ludenea.viewport");
    await settle();
    const notices = screen.getAllByRole("status").map((status) => status.textContent);
    expect(notices).toHaveLength(2);
    for (const notice of notices) expect(notice).toContain("Resimulating (tick 60 of 100)");
  });

  it("scrubber_shows_where_the_last_seek_landed", async () => {
    let current = 40;
    let push: (event: object) => void = () => {};
    const { host, game } = await editorWith((command) => {
      if (command.type === "get_timeline") return { type: "timeline", ...timeline({ current }) };
      if (command.type === "seek") {
        // The host stops short of the tick asked for, and reports the tick before answering.
        current = Math.min(command.tick as number, 30);
        push({ type: "ticked", tick: current, hash: "00000000000000aa", pending: false, playing: false, effective_rate: 0 });
      }
      return undefined;
    });
    push = game.push;
    renderPanel(host, "ludenea.timeline");
    await settle();
    const scrubber = screen.getByRole("slider", { name: "Tick" }) as HTMLInputElement;
    act(() => void fireEvent.change(scrubber, { target: { value: "50" } }));
    await settle();
    expect(scrubber.value).toBe("30");
  });
});
