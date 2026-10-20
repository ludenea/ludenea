import type { DebugDrawFrame, Entity, GameInfo } from "@ludenea/control-client";
import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { viewportPlugin } from "../src/viewport";
import { shared } from "../src/shared";
import { bounds, screenToWorld, worldToScreen, zoomAt } from "../src/viewport/camera";
import { renderers } from "./fake-renderer";
import { renderPanel, settle, testEditor, testGame } from "./support";

const entity = (index: number): Entity => ({ index, generation: 0 });
const size = { width: 400, height: 300 };
const pose = (x: number, y: number) => ({ type: "json", value: { position: { x, y }, rotation: { cos: 1, sin: 0 } } });

type Command = { type: string; [key: string]: unknown };

/** A 2D context recording what is drawn; jsdom has none. */
function fakeContext() {
  const calls: { method: string; args: unknown[] }[] = [];
  const context = new Proxy({} as Record<string | symbol, unknown>, {
    get: (target, key) => (key in target ? target[key] : (...args: unknown[]) => void calls.push({ method: String(key), args })),
    set: (target, key, value) => ((target[key] = value), true),
  });
  return { calls, context };
}

let drawn: ReturnType<typeof fakeContext>;

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => size.width });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => size.height });
});

async function viewport(answer: (command: Command) => object | undefined = () => undefined, info: Partial<GameInfo> = {}) {
  const host = await testEditor([viewportPlugin]);
  const game = await testGame((command) => answer(command) ?? (command.type === "get_debug_draw" ? { type: "debug_draw", tick: 7, frame: { layers: {} } } : { type: "done" }), {
    debug_layers: ["colliders", "contacts"],
    ...info,
  });
  act(() => host.setControl(game.client));
  renderPanel(host, "ludenea.viewport");
  await settle();
  return { host, game, view: screen.getByLabelText("Viewport") };
}

const sentOf = (sent: Command[], type: string) => sent.filter((command) => command.type === type);

describe("camera", () => {
  it("screen_to_world_inverts_world_to_screen", () => {
    const camera = { center: { x: 3, y: -2 }, pixelsPerMeter: 40 };
    const point = { x: 1.5, y: 4.25 };
    const screenPoint = worldToScreen(camera, size, point);
    expect(screenPoint.y).toBeLessThan(size.height / 2); // y up in the world, down on screen
    const back = screenToWorld(camera, size, screenPoint);
    expect(back.x).toBeCloseTo(point.x, 6);
    expect(back.y).toBeCloseTo(point.y, 6);
  });

  it("bounds_cover_the_selected_entities_shapes_in_the_shown_layers", () => {
    const frame: DebugDrawFrame = {
      layers: {
        colliders: [
          { entity: entity(1), primitive: { type: "circle", center: { x: 2, y: 3 }, radius: 1, color: 0, fill: false } },
          { entity: entity(1), primitive: { type: "capsule", a: { x: -1, y: 0 }, b: { x: 0, y: 0 }, radius: 0.5, color: 0 } },
          { entity: entity(2), primitive: { type: "line", a: { x: 50, y: 50 }, b: { x: 60, y: 60 }, color: 0 } },
        ],
        contacts: [{ entity: entity(1), primitive: { type: "label", at: { x: 9, y: 9 }, text: "hidden layer", color: 0 } }],
      },
    };
    expect(bounds(frame, ["colliders"], [entity(1)])).toEqual({ min: { x: -1.5, y: -0.5 }, max: { x: 3, y: 4 } });
    expect(bounds(frame, ["colliders", "contacts"], [entity(1)])!.max).toEqual({ x: 9, y: 9 });
    expect(bounds(frame, ["colliders"], [entity(3)])).toBeNull();
  });

  it("zoom_keeps_point_under_cursor", () => {
    const camera = { center: { x: 1, y: 1 }, pixelsPerMeter: 50 };
    const cursor = { x: 320, y: 40 };
    const before = screenToWorld(camera, size, cursor);
    const zoomed = zoomAt(camera, size, cursor, 2);
    expect(zoomed.pixelsPerMeter).toBe(100);
    const after = screenToWorld(zoomed, size, cursor);
    expect(after.x).toBeCloseTo(before.x, 6);
    expect(after.y).toBeCloseTo(before.y, 6);
  });
});

describe("viewport", () => {
  beforeEach(() => {
    localStorage.clear();
    drawn = fakeContext();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => drawn.context as unknown as CanvasRenderingContext2D);
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("click_sends_pick_and_selects", async () => {
    let picked = [entity(1)];
    const { host, game, view } = await viewport((command) => (command.type === "pick" ? { type: "picked", entities: picked } : undefined));
    // The default camera looks at the origin at 50 pixels per meter.
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(sentOf(game.sent, "pick")).toEqual([{ type: "pick", point: { x: 1, y: 1 } }]);
    expect(host.editor.selection.getState().entities).toEqual([entity(1)]);

    picked = [entity(2)];
    fireEvent.pointerDown(view, { button: 0, clientX: 200, clientY: 150, pointerId: 1, shiftKey: true });
    fireEvent.pointerUp(view, { button: 0, clientX: 200, clientY: 150, pointerId: 1, shiftKey: true });
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([entity(1), entity(2)]);

    picked = [];
    fireEvent.pointerDown(view, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([]);
  });

  it("drag_sends_one_set_field_on_release", async () => {
    const { host, game, view } = await viewport((command) => {
      if (command.type === "pick") return { type: "picked", entities: [entity(1)] };
      if (command.type === "get_entity") return { type: "entity", entity: entity(1), components: { "ludenea::Pose": pose(1, 1) } };
      return undefined;
    });
    act(() => host.editor.selection.getState().select([entity(1)]));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    fireEvent.pointerMove(view, { buttons: 1, clientX: 275, clientY: 125, pointerId: 1 });
    fireEvent.pointerMove(view, { buttons: 1, clientX: 300, clientY: 150, pointerId: 1 });
    expect(sentOf(game.sent, "set_field")).toEqual([]);
    fireEvent.pointerUp(view, { button: 0, clientX: 300, clientY: 150, pointerId: 1 });
    await settle();

    expect(sentOf(game.sent, "set_field")).toEqual([
      {
        type: "set_field",
        target: { type: "component", entity: entity(1), component: "ludenea::Pose" },
        path: [{ type: "field", name: "position" }],
        value: { x: 2, y: 0 },
      },
    ]);
    expect(host.editor.selection.getState().entities).toEqual([entity(1)]);
  });

  it("arrow_nudges_by_tenth_meter", async () => {
    const { host, game, view } = await viewport((command) =>
      command.type === "get_entity" ? { type: "entity", entity: entity(1), components: { "ludenea::Pose": pose(1, 2) } } : undefined,
    );
    act(() => host.editor.selection.getState().select([entity(1)]));
    fireEvent.keyDown(view, { key: "ArrowRight" });
    await settle();
    fireEvent.keyDown(view, { key: "ArrowDown" });
    await settle();
    expect(sentOf(game.sent, "set_field").map((command) => command.value)).toEqual([
      { x: 1.1, y: 2 },
      { x: 1, y: 1.9 },
    ]);

    // While playing, plain arrows belong to the game.
    act(() => host.editor.timeline.setState({ playing: true }));
    fireEvent.keyDown(view, { key: "ArrowLeft" });
    await settle();
    expect(sentOf(game.sent, "set_field")).toHaveLength(2);
  });

  it("nudging_a_multi_selection_sends_one_batch", async () => {
    const { host, game, view } = await viewport((command) => {
      if (command.type !== "get_entity") return undefined;
      const { index } = command.entity as Entity;
      return { type: "entity", entity: command.entity, components: { "ludenea::Pose": pose(index, 2) } };
    });
    act(() => host.editor.selection.getState().select([entity(1), entity(3)]));
    fireEvent.keyDown(view, { key: "ArrowUp" });
    await settle();

    expect(sentOf(game.sent, "set_field")).toEqual([]);
    expect(sentOf(game.sent, "batch")).toEqual([
      {
        type: "batch",
        commands: [1, 3].map((index) => ({
          type: "set_field",
          target: { type: "component", entity: entity(index), component: "ludenea::Pose" },
          path: [{ type: "field", name: "position" }],
          value: { x: index, y: 2.1 },
        })),
      },
    ]);
  });

  it("a_drag_moves_every_selected_entity_by_the_same_offset", async () => {
    const { host, game, view } = await viewport((command) => {
      if (command.type === "get_debug_draw") return { type: "debug_draw", tick: 7, frame: { layers: { colliders: [...circleOf(1, 1, 1).layers.colliders!, ...circleOf(2, -1, 1).layers.colliders!] } } };
      if (command.type === "pick") return { type: "picked", entities: [entity(2)] };
      if (command.type === "get_entity") return { type: "entity", entity: command.entity, components: { "ludenea::Pose": pose((command.entity as Entity).index, 1) } };
      return undefined;
    });
    act(() => host.editor.selection.getState().select([entity(1), entity(2)]));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    fireEvent.pointerMove(view, { buttons: 1, clientX: 300, clientY: 150, pointerId: 1 });
    // Both outlines move one meter right and one down.
    await waitFor(() => expect(drawn.calls).toContainEqual({ method: "arc", args: [300, 150, 25, 0, 2 * Math.PI] }));
    expect(drawn.calls).toContainEqual({ method: "arc", args: [200, 150, 25, 0, 2 * Math.PI] });
    fireEvent.pointerUp(view, { button: 0, clientX: 300, clientY: 150, pointerId: 1 });
    await settle();

    expect(sentOf(game.sent, "set_field")).toEqual([]);
    expect(sentOf(game.sent, "batch").map((command) => (command.commands as Command[]).map(({ value }) => value))).toEqual([
      [
        { x: 2, y: 0 },
        { x: 3, y: 0 },
      ],
    ]);
    expect(host.editor.selection.getState().entities).toEqual([entity(1), entity(2)]);
  });

  it("layer_toggle_commands_follow_game_info", async () => {
    const { host, game } = await viewport();
    const toggles = () => host.editor.commands.list().map((command) => command.id).filter((id) => id.startsWith("viewport.toggle-layer."));
    expect(toggles()).toEqual(["viewport.toggle-layer.colliders", "viewport.toggle-layer.contacts"]);
    await waitFor(() => expect(sentOf(game.sent, "get_debug_draw").at(-1)).toEqual({ type: "get_debug_draw", layers: ["colliders", "contacts"] }));

    await act(() => host.editor.commands.run("viewport.toggle-layer.colliders"));
    await waitFor(() => expect(sentOf(game.sent, "get_debug_draw").at(-1)).toEqual({ type: "get_debug_draw", layers: ["contacts"] }));
    // The layers the built-in plugins share are the viewport's: a change there is drawn.
    act(() => shared(host.editor).layers.set({ hidden: ["contacts"] }));
    await waitFor(() => expect(sentOf(game.sent, "get_debug_draw").at(-1)).toEqual({ type: "get_debug_draw", layers: ["colliders"] }));
    await act(() => host.editor.commands.run("viewport.toggle-layer.colliders"));
    expect(shared(host.editor).layers.get().hidden).toEqual(["contacts", "colliders"]);

    const other = await testGame(() => ({ type: "debug_draw", tick: 0, frame: { layers: {} } }), { debug_layers: ["aabbs"] });
    act(() => host.setControl(other.client));
    expect(toggles()).toEqual(["viewport.toggle-layer.aabbs"]);
  });

  it("labels_are_drawn_on_overlay", async () => {
    const frame: DebugDrawFrame = {
      layers: { colliders: [{ entity: null, primitive: { type: "label", at: { x: 1, y: 1 }, text: "hello", color: 0xff0000ff } }] },
    };
    const overlay = vi.fn();
    const { host } = await viewport((command) => (command.type === "get_debug_draw" ? { type: "debug_draw", tick: 7, frame } : undefined));
    host.editor.overlays.register({ id: "test.overlay", draw: overlay });
    act(() => host.editor.selection.getState().select([entity(3)]));

    await waitFor(() => expect(drawn.calls).toContainEqual({ method: "fillText", args: ["hello", 250, 100] }));
    await waitFor(() => expect(overlay).toHaveBeenCalled());
    const context = overlay.mock.calls.at(-1)![0];
    expect(context.tick).toBe(7);
    expect(context.selection).toEqual([entity(3)]);
    expect(context.worldToScreen({ x: 1, y: 1 })).toEqual({ x: 250, y: 100 });
    const renderer = renderers.at(-1)!;
    expect(renderer.calls.some((call) => call.method === "draw" && call.args[0] === JSON.stringify(frame))).toBe(true);
  });

  it("delete_despawns_the_selection_while_paused", async () => {
    const { host, view } = await viewport();
    const despawn = vi.fn();
    host.editor.commands.register({ id: "scene.despawn-selection", title: "Despawn", run: despawn });
    fireEvent.keyDown(view, { key: "Delete" });
    expect(despawn).toHaveBeenCalledTimes(1);

    act(() => host.editor.timeline.setState({ playing: true }));
    fireEvent.keyDown(view, { key: "Delete" });
    expect(despawn).toHaveBeenCalledTimes(1);
  });

  /** The camera the renderer last drew with: center x, center y, pixels per meter. */
  const camera = () => renderers.at(-1)!.calls.filter((call) => call.method === "set_camera").at(-1)?.args;
  const circleOf = (index: number, x: number, y: number): DebugDrawFrame => ({
    layers: { colliders: [{ entity: entity(index), primitive: { type: "circle", center: { x, y }, radius: 0.5, color: 0xffffffff, fill: false } }] },
  });

  it("space_drag_and_middle_drag_pan", async () => {
    const { view } = await viewport();
    fireEvent.keyDown(view, { key: " " });
    fireEvent.pointerDown(view, { button: 0, clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerMove(view, { clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.keyUp(view, { key: " " });
    // The content follows the pointer: 50 pixels at 50 pixels per meter is one meter.
    await waitFor(() => expect(camera()).toEqual([-1, -1, 50]));

    fireEvent.pointerDown(view, { button: 1, clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerMove(view, { clientX: 150, clientY: 150, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 1, clientX: 150, clientY: 150, pointerId: 1 });
    await waitFor(() => expect(camera()).toEqual([0, -1, 50]));
  });

  it("wheel_zooms_at_the_cursor", async () => {
    const { view } = await viewport();
    fireEvent.wheel(view, { deltaY: -200, clientX: 200, clientY: 150 });
    await waitFor(() => expect(camera()?.[2]).toBeCloseTo(50 * Math.exp(0.3), 6));
    // Centered on the view's middle, the center stays.
    expect(camera()!.slice(0, 2)).toEqual([0, 0]);
  });

  it("pan_zoom_and_frame_commands_move_the_camera", async () => {
    const { host } = await viewport((command) => (command.type === "get_debug_draw" ? { type: "debug_draw", tick: 7, frame: circleOf(1, 4, -2) } : undefined));
    await act(() => host.editor.commands.run("viewport.pan-right"));
    await waitFor(() => expect(camera()).toEqual([64 / 50, 0, 50]));
    await act(() => host.editor.commands.run("viewport.zoom-in"));
    await waitFor(() => expect(camera()?.[2]).toBe(62.5));

    act(() => host.editor.selection.getState().select([entity(1)]));
    await act(() => host.editor.commands.run("viewport.frame-selection"));
    // A one-meter box in a 400 by 300 view, with a fifth of margin each side: 300 * 0.6.
    await waitFor(() => expect(camera()).toEqual([4, -2, 180]));
  });

  it("drag_outline_is_drawn_while_dragging", async () => {
    const { host, view } = await viewport((command) => {
      if (command.type === "get_debug_draw") return { type: "debug_draw", tick: 7, frame: circleOf(1, 1, 1) };
      if (command.type === "pick") return { type: "picked", entities: [entity(1)] };
      return undefined;
    });
    act(() => host.editor.selection.getState().select([entity(1)]));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    fireEvent.pointerMove(view, { clientX: 300, clientY: 100, pointerId: 1 });
    // The outline: dashed, around the circle moved one meter right.
    await waitFor(() => expect(drawn.calls).toContainEqual({ method: "arc", args: [300, 100, 25, 0, 2 * Math.PI] }));
    expect(drawn.calls).toContainEqual({ method: "setLineDash", args: [[4, 3]] });
  });

  it("a_drag_starting_on_an_unselected_entity_selects_and_moves_it", async () => {
    const { host, game, view } = await viewport((command) => {
      if (command.type === "pick") return { type: "picked", entities: [entity(2)] };
      if (command.type === "get_entity") return { type: "entity", entity: entity(2), components: { "ludenea::Pose": pose(1, 1) } };
      return undefined;
    });
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([entity(2)]);
    fireEvent.pointerMove(view, { clientX: 300, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 300, clientY: 100, pointerId: 1 });
    await settle();
    expect(sentOf(game.sent, "set_field").map((command) => command.value)).toEqual([{ x: 2, y: 1 }]);
  });

  it("a_cancelled_drag_moves_nothing", async () => {
    const { host, game, view } = await viewport((command) => {
      if (command.type === "pick") return { type: "picked", entities: [entity(1)] };
      if (command.type === "get_entity") return { type: "entity", entity: entity(1), components: { "ludenea::Pose": pose(1, 1) } };
      return undefined;
    });
    act(() => host.editor.selection.getState().select([entity(1)]));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    fireEvent.pointerMove(view, { clientX: 300, clientY: 100, pointerId: 1 });
    fireEvent.pointerCancel(view, { pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 300, clientY: 100, pointerId: 1 });
    await settle();
    expect(sentOf(game.sent, "set_field")).toEqual([]);
  });

  it("a_skipped_draw_is_drawn_again_while_paused", async () => {
    const { host } = await viewport();
    const renderer = renderers.at(-1)!;
    const draws = () => renderer.calls.filter((call) => call.method === "draw").length;
    await waitFor(() => expect(draws()).toBeGreaterThan(0));
    const before = draws();
    renderer.skips = 2;
    // Any change asks for a repaint; the two skipped ones are retried without another.
    act(() => host.editor.selection.getState().select([entity(9)]));
    await act(async () => {
      for (let frame = 0; frame < 3; frame++) await new Promise((resolve) => requestAnimationFrame(resolve));
    });
    expect(draws()).toBeGreaterThanOrEqual(before + 3);
  });

  it("a_pixel_ratio_change_resizes_the_drawing_buffer", async () => {
    await viewport();
    const resizes = () => renderers.at(-1)!.calls.filter((call) => call.method === "resize").map((call) => call.args);
    await waitFor(() => expect(resizes().at(-1)).toEqual([400, 300, 1]));
    vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
    await waitFor(() => expect(resizes().at(-1)).toEqual([400, 300, 2]));
  });

  it("the_renderer_is_freed_on_unmount", async () => {
    await viewport();
    const renderer = renderers.at(-1)!;
    cleanup();
    expect(renderer.calls.at(-1)?.method).toBe("free");
  });

  it("expected_misses_stay_out_of_the_error_log", async () => {
    let refuse = false;
    const { host, view } = await viewport((command) => (refuse && command.type === "get_debug_draw" ? { error: { code: "internal", message: "game closed" } } : undefined));
    // Nothing selected to frame.
    await act(() => host.editor.commands.run("viewport.frame-selection"));
    // A frame refused by the game being replaced.
    refuse = true;
    act(() => host.editor.timeline.setState({ hash: "0000000000000001" }));
    act(() => host.setControl(null));
    await settle();
    // Nudging and despawning without a game.
    fireEvent.keyDown(view, { key: "ArrowRight" });
    fireEvent.keyDown(view, { key: "Delete" });
    await settle();
    expect(host.editor.console.list()).toEqual([]);
  });

  it("a_refused_drag_and_a_refused_key_are_reported_in_the_editor_console", async () => {
    const { host, view } = await viewport((command) => {
      if (command.type === "pick") return { type: "picked", entities: [entity(1)] };
      if (command.type === "get_entity") return { type: "entity", entity: entity(1), components: { "ludenea::Pose": pose(1, 1) } };
      if (command.type === "set_field") return { error: { code: "invalid_command", message: "the entity is locked" } };
      return undefined;
    });
    host.editor.commands.register({
      id: "scene.despawn-selection",
      title: "Despawn selected entities",
      run() {
        throw new Error("select the entities to despawn first");
      },
    });
    act(() => host.editor.selection.getState().select([entity(1)]));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    fireEvent.pointerMove(view, { buttons: 1, clientX: 300, clientY: 150, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 300, clientY: 150, pointerId: 1 });
    await settle();
    fireEvent.keyDown(view, { key: "Delete" });
    await settle();

    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([
      ["error", "Moving the dragged entities in the viewport failed: the entity is locked"],
      ["error", "Despawn selected entities failed: select the entities to despawn first"],
    ]);
  });

  it("a_debug_draw_failing_every_frame_is_reported_once_until_it_works_again", async () => {
    let refuse = true;
    const { host, game } = await viewport((command) =>
      refuse && command.type === "get_debug_draw" ? { error: { code: "internal", message: "the debug draw panicked" } } : undefined,
    );
    const fetched = () => sentOf(game.sent, "get_debug_draw").length;
    const moreFrames = async () => {
      const before = fetched();
      await waitFor(() => expect(fetched()).toBeGreaterThan(before + 2));
    };
    const reported = "Fetching the debug draw failed: the debug draw panicked";
    // Playing fetches every frame.
    act(() => host.editor.timeline.setState({ playing: true }));
    await moreFrames();
    expect(host.editor.console.list().map(({ text }) => text)).toEqual([reported]);

    refuse = false;
    await moreFrames();
    refuse = true;
    await moreFrames();
    expect(host.editor.console.list().map(({ text }) => text)).toEqual([reported, reported]);
  });

  it("a_failing_overlay_is_reported_once_per_registration", async () => {
    const { host } = await viewport();
    const broken = {
      id: "my_game.broken",
      draw() {
        throw new Error("no font loaded");
      },
    };
    const reports = () => host.editor.console.list().map(({ text }) => text);
    const painted = () => drawn.calls.filter((call) => call.method === "clearRect").length;
    // Three more paints of the overlay canvas.
    const frames = async () => {
      for (const index of [1, 2, 3]) {
        const before = painted();
        act(() => host.editor.selection.getState().select([entity(index)]));
        await waitFor(() => expect(painted()).toBeGreaterThan(before));
      }
    };
    const registration = host.editor.overlays.register(broken);
    await frames();
    expect(reports()).toEqual(['Drawing the viewport overlay "my_game.broken" failed: no font loaded']);

    // Drawn again by a viewport mounted anew: still reported.
    cleanup();
    renderPanel(host, "ludenea.viewport");
    await frames();
    expect(reports()).toHaveLength(1);

    // Registered again: a new registration.
    registration.dispose();
    await frames();
    host.editor.overlays.register(broken);
    await frames();
    expect(reports()).toEqual(['Drawing the viewport overlay "my_game.broken" failed: no font loaded', 'Drawing the viewport overlay "my_game.broken" failed: no font loaded']);
  });

  it("nudging_waits_for_a_pause", async () => {
    const { host, game } = await viewport();
    act(() => host.editor.selection.getState().select([entity(1)]));
    act(() => host.editor.timeline.setState({ playing: true }));
    await expect(host.editor.commands.run("viewport.nudge-right")).rejects.toThrow(/pause/);
    expect(sentOf(game.sent, "get_entity")).toEqual([]);
  });
});
