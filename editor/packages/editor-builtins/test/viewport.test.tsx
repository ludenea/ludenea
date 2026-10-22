/// <reference types="node" />
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { CameraItem, DebugDrawFrame, Entity, GameInfo, RenderFrame, TextureMeta } from "@ludenea/control-client";
import type { ToolContext, ToolPointer, ViewportTool } from "@ludenea/editor-shell";
import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { consolePlugin } from "../src/console";
import { viewportPlugin } from "../src/viewport";
import { shared } from "../src/shared";
import { tickOf } from "../src/viewport/frames";
import { mergeHits } from "../src/viewport/state";
import { bounds, fromCameraItem, screenToWorld, toCameraView, worldToScreen, zoomAt } from "../src/viewport/camera";
import { inits, named, panic, renderers, script, type WebRenderer } from "./fake-render-wasm";
import { renderFrame, renderPanel, settle, testEditor, testGame } from "./support";

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

type Answer = (command: Command) => object | undefined;

/** The viewport over a game answering `answer` (frames of tick 7 by default), in an editor whose project answers `project`. */
async function viewport(answer: Answer = () => undefined, info: Partial<GameInfo> = {}, project?: Answer) {
  const host = await testEditor([viewportPlugin], project);
  const game = await testGame((command) => answer(command) ?? (command.type === "get_render_frame" ? renderFrame(7) : { type: "done" }), {
    debug_layers: ["colliders", "contacts"],
    ...info,
  });
  act(() => host.setControl(game.client));
  renderPanel(host, "ludenea.viewport");
  await settle();
  return { host, game, view: screen.getByLabelText("Viewport") };
}

const sentOf = (sent: Command[], type: string) => sent.filter((command) => command.type === type);
/** The render frame of `tick`, with `extra` fields. */
const frameAt = (tick: number, extra: Partial<RenderFrame> = {}): RenderFrame => ({ ...renderFrame(tick).frame, ...extra });
/** `frame` as the fake renderer reads bytes: its tick as postcard starts a frame, then its JSON in UTF-8. */
const bytesOf = (frame: RenderFrame) => {
  const tick: number[] = [];
  for (let rest = frame.tick; ; rest = Math.floor(rest / 128)) {
    if (rest < 128) {
      tick.push(rest);
      break;
    }
    tick.push((rest % 128) | 0x80);
  }
  const json = new TextEncoder().encode(JSON.stringify(frame));
  const bytes = new Uint8Array(tick.length + json.length);
  bytes.set(tick);
  bytes.set(json, tick.length);
  return bytes.buffer;
};
/** The `render_frames` event of `frames` as a text transport carries it: each frame's bytes in base64. */
const textFrames = (progress: number, frames: RenderFrame[]) => ({ type: "render_frames", progress, frames: frames.map((frame) => Buffer.from(bytesOf(frame)).toString("base64")) });
const ticksPushed = (renderer: WebRenderer) => renderer.pushed.map((frame) => frame.tick);
const drawsOf = (renderer: WebRenderer) => renderer.calls.filter((call) => call.method === "draw").map((call) => call.args as [number, string | null]);
/** The commands that follow the pushed frames. */
const following = (sent: Command[]) => sent.filter((command) => ["set_pushed_layers", "subscribe", "unsubscribe"].includes(command.type));
/** Lets `count` animation frames run. */
const animationFrames = (count: number) =>
  act(async () => {
    for (let frame = 0; frame < count; frame++) await new Promise((resolve) => requestAnimationFrame(resolve));
  });
const circleAt = (index: number, x: number, y: number): DebugDrawFrame => ({
  layers: { colliders: [{ entity: { index, generation: 0 }, primitive: { type: "circle", center: { x, y }, radius: 0.5, color: 0xffffffff, fill: false } }] },
});
const ticked = (tick: number, rate: number) => ({ type: "ticked", tick, hash: `000000000000${String(tick).padStart(4, "0")}`, pending: false, playing: true, effective_rate: rate });

type Timeline = { editor: { timeline: { setState(state: object): void } } };
const play = (host: Timeline) => act(() => host.editor.timeline.setState({ playing: true }));
const pause = (host: Timeline) => act(() => host.editor.timeline.setState({ playing: false }));

/** Shows tick `current` of `hash` on the timeline, as a refresh after a step, seek, edit, or pause does. */
function atTick(host: { editor: { timeline: { setState(state: object): void } } }, current: number, hash = `00000000000000${String(current).padStart(2, "0")}`) {
  const timeline = { head: current, simulated: current, current, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, undo_label: null, redo_label: null, resimulating: false, branch: 0, branches: [], compare: null, scene_dirty: false, unsaved_assets: [] };
  act(() => host.editor.timeline.setState({ timeline, hash }));
}

const META: TextureMeta = { pixels_per_unit: 16, grid: null, pivot: null, filter: "Nearest" };
const UNREADABLE = "00000000000000000000000000000000";
/** A project of two textures and a scene; `read_binary` answers each PNG's path as its bytes. */
const textures = (entries: { path: string; id: string }[]): Answer => (request) => {
  if (request.type === "list_files") return { type: "files", entries: [...entries.map(({ path, id }) => ({ path, id, kind: { Texture: META } })), { path: "scenes/main.scene.ron", id: "00000000000000000000000000000c01", kind: "Scene" }] };
  if (request.type === "read_binary") return { type: "bytes", bytes: btoa(request.path as string) };
  return undefined;
};

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

  it("the_camera_view_shows_what_the_editor_camera_does", () => {
    const camera = { center: { x: 3, y: -2 }, pixelsPerMeter: 40 };
    expect(toCameraView(camera, size)).toEqual({ center: { x: 3, y: -2 }, view: { x: 10, y: 7.5 } });
    // The game camera's minimum view widened to the viewport's shape, at the same scale on both axes.
    const item: CameraItem = { entity: entity(1), center: { x: 1, y: 2 }, view: { x: 8, y: 8 } };
    expect(fromCameraItem(item, size)).toEqual({ center: { x: 1, y: 2 }, pixelsPerMeter: 37.5 });
    expect(toCameraView(fromCameraItem(item, size), size).view).toEqual({ x: 400 / 37.5, y: 8 });
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

describe("loadRenderModule", () => {
  /** The loader and the fake module as a page first loads them. */
  const freshLoader = async () => {
    vi.resetModules();
    const fake = await import("./fake-render-wasm");
    // Reset modules no longer see the mock of test/setup.ts: the same one, on the new fake.
    vi.doMock("../src/shared/render-module", () => ({
      async instantiate() {
        await fake.default();
        return fake;
      },
    }));
    return { fake, ...(await import("../src/shared")) };
  };

  it("a_failed_load_is_tried_again_on_the_next_call", async () => {
    const { fake, loadRenderModule } = await freshLoader();
    fake.script.initError = new Error("the module cannot be fetched");
    await expect(loadRenderModule()).rejects.toThrow("the module cannot be fetched");
    await expect(loadRenderModule()).resolves.toBe(fake);
    // Loaded once it worked: not again.
    await loadRenderModule();
    expect(fake.inits).toHaveLength(2);
  });

  it("a_panic_drops_the_module_and_the_next_load_instantiates_a_new_one", async () => {
    const { fake, loadRenderModule, onRenderPanic } = await freshLoader();
    const heard: string[] = [];
    const stop = onRenderPanic((message) => heard.push(message));
    const first = await loadRenderModule();
    expect(await loadRenderModule()).toBe(first);
    expect(fake.inits).toHaveLength(1);
    fake.panic("panicked at src/web.rs:1:1:\nbroken");
    expect(heard).toEqual(["panicked at src/web.rs:1:1:\nbroken"]);
    // The aborted instance is not used again: the next load instantiates the module anew.
    await loadRenderModule();
    expect(fake.inits).toHaveLength(2);
    await loadRenderModule();
    expect(fake.inits).toHaveLength(2);
    stop();
    fake.panic("again");
    expect(heard).toHaveLength(1);
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
    script.refused = [];
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
      if (command.type === "get_render_frame") return renderFrame(7, { layers: { colliders: [...circleOf(1, 1, 1).layers.colliders!, ...circleOf(2, -1, 1).layers.colliders!] } });
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
    await waitFor(() => expect(sentOf(game.sent, "get_render_frame").at(-1)).toEqual({ type: "get_render_frame", debug_layers: ["colliders", "contacts"] }));

    await act(() => host.editor.commands.run("viewport.toggle-layer.colliders"));
    await waitFor(() => expect(sentOf(game.sent, "get_render_frame").at(-1)).toEqual({ type: "get_render_frame", debug_layers: ["contacts"] }));
    // The layers the built-in plugins share are the viewport's: a change there is drawn.
    act(() => shared(host.editor).layers.set({ hidden: ["contacts"] }));
    await waitFor(() => expect(sentOf(game.sent, "get_render_frame").at(-1)).toEqual({ type: "get_render_frame", debug_layers: ["colliders"] }));
    await act(() => host.editor.commands.run("viewport.toggle-layer.colliders"));
    expect(shared(host.editor).layers.get().hidden).toEqual(["contacts", "colliders"]);

    const other = await testGame(() => renderFrame(0), { debug_layers: ["aabbs"] });
    act(() => host.setControl(other.client));
    expect(toggles()).toEqual(["viewport.toggle-layer.aabbs"]);
  });

  it("labels_are_drawn_on_overlay", async () => {
    const frame: DebugDrawFrame = {
      layers: { colliders: [{ entity: null, primitive: { type: "label", at: { x: 1, y: 1 }, text: "hello", color: 0xff0000ff } }] },
    };
    const overlay = vi.fn();
    const { host } = await viewport((command) => (command.type === "get_render_frame" ? renderFrame(7, frame) : undefined));
    host.editor.overlays.register({ id: "test.overlay", draw: overlay });
    act(() => host.editor.selection.getState().select([entity(3)]));

    await waitFor(() => expect(drawn.calls).toContainEqual({ method: "fillText", args: ["hello", 250, 100] }));
    await waitFor(() => expect(overlay).toHaveBeenCalled());
    const context = overlay.mock.calls.at(-1)![0];
    expect(context.tick).toBe(7);
    expect(context.selection).toEqual([entity(3)]);
    expect(context.worldToScreen({ x: 1, y: 1 })).toEqual({ x: 250, y: 100 });
    expect(renderers.at(-1)!.pushed.at(-1)!.debug).toEqual(frame);
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
  const camera = () => {
    const drawn = drawsOf(renderers.at(-1)!).at(-1)?.[1];
    if (!drawn) return undefined;
    const { center, view } = JSON.parse(drawn) as { center: { x: number; y: number }; view: { x: number; y: number } };
    return [center.x, center.y, size.width / view.x];
  };
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

  it("a_press_on_the_resimulating_notice_still_selects_and_pans", async () => {
    const { host, game } = await viewport((command) => (command.type === "pick" ? { type: "picked", entities: [entity(1)] } : undefined));
    const timeline = { head: 9, simulated: 3, current: 3, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, undo_label: null, redo_label: null, resimulating: true, branch: 0, branches: [], compare: null, scene_dirty: false, unsaved_assets: [] };
    act(() => host.editor.timeline.setState({ timeline }));
    // Text over the viewport is part of its surface, unlike its controls.
    const notice = screen.getByRole("status");
    fireEvent.pointerDown(notice, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(notice, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(sentOf(game.sent, "pick")).toEqual([{ type: "pick", point: { x: 1, y: 1 } }]);
    expect(host.editor.selection.getState().entities).toEqual([entity(1)]);
    fireEvent.pointerDown(notice, { button: 1, clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerMove(notice, { clientX: 150, clientY: 150, pointerId: 1 });
    fireEvent.pointerUp(notice, { button: 1, clientX: 150, clientY: 150, pointerId: 1 });
    await waitFor(() => expect(camera()).toEqual([1, 0, 50]));
  });

  it("wheel_zooms_at_the_cursor", async () => {
    const { view } = await viewport();
    fireEvent.wheel(view, { deltaY: -200, clientX: 200, clientY: 150 });
    await waitFor(() => expect(camera()?.[2]).toBeCloseTo(50 * Math.exp(0.3), 6));
    // Centered on the view's middle, the center stays.
    expect(camera()!.slice(0, 2)).toEqual([0, 0]);
  });

  it("pan_zoom_and_frame_commands_move_the_camera", async () => {
    const { host } = await viewport((command) => (command.type === "get_render_frame" ? renderFrame(7, circleOf(1, 4, -2)) : undefined));
    await act(() => host.editor.commands.run("viewport.pan-right"));
    await waitFor(() => expect(camera()).toEqual([64 / 50, 0, 50]));
    await act(() => host.editor.commands.run("viewport.zoom-in"));
    await waitFor(() => expect(camera()?.[2]).toBe(62.5));

    act(() => host.editor.selection.getState().select([entity(1)]));
    await act(() => host.editor.commands.run("viewport.frame-selection"));
    // A one-meter box in a 400 by 300 view, with a fifth of margin each side: 300 * 0.6.
    await waitFor(() => expect(camera()).toEqual([4, -2, 180]));
  });

  it("click_on_a_sprite_without_a_collider_selects_it", async () => {
    const { host, game, view } = await viewport((command) => (command.type === "pick" ? { type: "picked", entities: [] } : undefined));
    const renderer = renderers.at(-1)!;
    renderer.hits = [entity(5)];
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([entity(5)]);
    // The renderer picks in canvas pixels through the camera on screen, as the host does at its world point.
    expect(renderer.calls.filter((call) => call.method === "pick").at(-1)!.args).toEqual([250, 100, JSON.stringify({ center: { x: 0, y: 0 }, view: { x: 8, y: 6 } })]);
    expect(sentOf(game.sent, "pick").at(-1)).toEqual({ type: "pick", point: { x: 1, y: 1 } });
  });

  it("click_merges_collider_and_sprite_hits_without_duplicates", async () => {
    expect(mergeHits([entity(1), entity(2)], [entity(2), entity(3)])).toEqual([entity(1), entity(2), entity(3)]);
    const { host, view } = await viewport((command) => (command.type === "pick" ? { type: "picked", entities: [entity(1), entity(2)] } : undefined));
    renderers.at(-1)!.hits = [entity(2), entity(3)];
    const click = (shiftKey: boolean) => {
      fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1, shiftKey });
      fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1, shiftKey });
    };
    // The host's hits come first: its topmost is the one a click selects.
    click(false);
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([entity(1)]);
    // Shift adds the topmost hit, the host's, to the selection.
    act(() => host.editor.selection.getState().select([entity(3)]));
    click(true);
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([entity(3), entity(1)]);
  });

  it("frame_selection_fits_a_sprite_only_entity", async () => {
    const { host } = await viewport();
    const renderer = renderers.at(-1)!;
    renderer.box = { min: { x: 2, y: -1 }, max: { x: 4, y: 1 } };
    act(() => host.editor.selection.getState().select([entity(5)]));
    await act(() => host.editor.commands.run("viewport.frame-selection"));
    // A 2 by 2 meter box in a 400 by 300 view, with a fifth of margin each side: 150 * 0.6.
    await waitFor(() => expect(camera()).toEqual([3, 0, 90]));
    expect(renderer.calls.filter((call) => call.method === "bounds").at(-1)!.args).toEqual([[entity(5)]]);
  });

  it("frame_selection_fits_the_union_of_sprites_and_debug_shapes", async () => {
    const { host } = await viewport((command) => (command.type === "get_render_frame" ? renderFrame(7, circleOf(1, 4, -2)) : undefined));
    renderers.at(-1)!.box = { min: { x: 2, y: -1 }, max: { x: 4, y: 1 } };
    act(() => host.editor.selection.getState().select([entity(1)]));
    await act(() => host.editor.commands.run("viewport.frame-selection"));
    // From (2, -2.5) to (4.5, 1): 2.5 by 3.5 meters, the height decides: 300 / 3.5 * 0.6.
    await waitFor(() => expect(camera()?.slice(0, 2)).toEqual([3.25, -0.75]));
    expect(camera()![2]).toBeCloseTo((300 / 3.5) * 0.6, 6);
  });

  it("missing_texture_entity_is_still_selectable", async () => {
    script.refused = [{ id: "00000000000000000000000000000a01", path: "assets/gone.png", message: "the PNG cannot be decoded" }];
    const sprite = { entity: entity(6), layer: 0, order: 0, texture: "00000000000000000000000000000a01", frame: 0, position: { x: 1, y: 1 }, rotation: { cos: 1, sin: 0 }, flip_x: false, flip_y: false, tint: 0xffffffff };
    const { host, view } = await viewport(
      (command) => {
        if (command.type === "get_render_frame") return { type: "render_frame", frame: frameAt(7, { sprites: [sprite] }) };
        if (command.type === "pick") return { type: "picked", entities: [] };
        return undefined;
      },
      {},
      textures([{ path: "assets/gone.png", id: "00000000000000000000000000000a01" }]),
    );
    await waitFor(() => expect(host.editor.console.list().map(({ text }) => text)).toContain("assets/gone.png: the PNG cannot be decoded"));
    // The renderer draws it as its placeholder, and picks and bounds what it draws.
    const renderer = renderers.at(-1)!;
    renderer.hits = [entity(6)];
    renderer.box = { min: { x: 0.5, y: 0.5 }, max: { x: 1.5, y: 1.5 } };
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([entity(6)]);
    await act(() => host.editor.commands.run("viewport.frame-selection"));
    await waitFor(() => expect(camera()).toEqual([1, 1, 180]));
  });

  it("drag_outline_is_drawn_while_dragging", async () => {
    const { host, view } = await viewport((command) => {
      if (command.type === "get_render_frame") return renderFrame(7, circleOf(1, 1, 1));
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
    const draws = () => drawsOf(renderer).length;
    await waitFor(() => expect(draws()).toBeGreaterThan(0));
    await animationFrames(2);
    const before = draws();
    renderer.fail("NoSurfaceTexture", 2);
    // Any change asks for a repaint; the two skipped ones (no surface texture) are retried without another.
    act(() => host.editor.selection.getState().select([entity(9)]));
    await animationFrames(5);
    expect(draws()).toBe(before + 3);
    // Not a failure: nothing is reported.
    expect(host.editor.console.list()).toEqual([]);
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
    const { host, view } = await viewport((command) => (refuse && command.type === "get_render_frame" ? { error: { code: "internal", message: "game closed" } } : undefined));
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

  it("a_render_frame_failing_every_time_is_reported_once_until_it_works_again", async () => {
    let refuse = true;
    let tick = 0;
    const { host, game } = await viewport((command) => {
      if (command.type !== "get_render_frame") return undefined;
      return refuse ? { error: { code: "internal", message: "the extraction panicked" } } : renderFrame(tick);
    });
    const fetched = () => sentOf(game.sent, "get_render_frame").length;
    // Each step while paused fetches the tick on screen.
    const steps = async () => {
      for (let step = 0; step < 3; step++) {
        const before = fetched();
        atTick(host, ++tick);
        await waitFor(() => expect(fetched()).toBe(before + 1));
        await settle();
      }
    };
    const reported = "Fetching the render frame failed: the extraction panicked";
    await steps();
    expect(host.editor.console.list().map(({ text }) => text)).toEqual([reported]);

    refuse = false;
    await steps();
    refuse = true;
    await steps();
    expect(host.editor.console.list().map(({ text }) => text)).toEqual([reported, reported]);
  });

  it("a_failing_draw_is_reported_in_the_editor_console", async () => {
    const { host } = await viewport();
    vi.spyOn(renderers.at(-1)!, "draw").mockImplementation(() => {
      throw new Error("the surface is lost");
    });
    act(() => host.editor.selection.getState().select([entity(1)]));
    await waitFor(() => expect(host.editor.console.list()).toHaveLength(1));

    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([["error", "Drawing the viewport failed: the surface is lost"]]);
  });

  it("a_failing_sprite_pick_keeps_the_host_hits_and_is_reported", async () => {
    const { host, view } = await viewport((command) => (command.type === "pick" ? { type: "picked", entities: [entity(1)] } : undefined));
    renderers.at(-1)!.pickError = new Error("the camera is not a CameraView");
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(host.editor.selection.getState().entities).toEqual([entity(1)]);
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([
      ["error", "Picking the sprites and tilemaps in the viewport failed: the camera is not a CameraView"],
    ]);
  });

  it("a_refused_pick_is_reported_in_the_editor_console", async () => {
    const { host, view } = await viewport((command) => (command.type === "pick" ? { error: { code: "system_panicked", message: "the game panicked while picking" } } : undefined));
    const reported = ["error", "Picking an entity in the viewport failed: the game panicked while picking"];
    // A click: reported once, not again on release.
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([reported]);
    // A drag: the same.
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(view, { buttons: 1, clientX: 300, clientY: 150, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 300, clientY: 150, pointerId: 1 });
    await settle();
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([reported, reported]);
    expect(host.editor.selection.getState().entities).toEqual([]);
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

  /** A tool recording the events it hears, as `[kind, event]`, with the context of the last one. */
  const recordingTool = (available: ViewportTool["available"] = () => true) => {
    const heard: [string, ToolPointer | null][] = [];
    const contexts: ToolContext[] = [];
    const record = (kind: string) => (event: ToolPointer, context: ToolContext) => {
      heard.push([kind, event]);
      contexts.push(context);
    };
    const tool: ViewportTool = { id: "test.paint", title: "Paint", cursor: "crosshair", available, pointerDown: record("down"), pointerMove: record("move"), pointerUp: record("up") };
    tool.pointerCancel = (context) => {
      heard.push(["cancel", null]);
      contexts.push(context);
    };
    return { tool, heard, contexts };
  };

  it("an_active_tool_receives_pointer_events_in_world_units", async () => {
    const { host, game, view } = await viewport((command) => (command.type === "pick" ? { type: "picked", entities: [entity(1)] } : undefined));
    const { tool, heard, contexts } = recordingTool();
    tool.overlay = ({ canvas, worldToScreen }) => {
      const at = worldToScreen({ x: 1, y: 1 });
      canvas.fillText("brush", at.x, at.y);
      // On a layer of parallax 0.5, the point under the press drawn earlier.
      const far = worldToScreen({ x: 1.5, y: 1 }, 0.5);
      canvas.fillText("far brush", far.x, far.y);
    };
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.viewport.setTool("test.paint"));
    expect(view.style.cursor).toBe("crosshair");
    // A middle-drag of 50 pixels left: the view looks at (1, 0).
    fireEvent.pointerDown(view, { button: 1, clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerMove(view, { button: -1, clientX: 150, clientY: 150, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 1, clientX: 150, clientY: 150, pointerId: 1 });
    await waitFor(() => expect(camera()).toEqual([1, 0, 50]));
    expect(heard).toEqual([]);

    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1, shiftKey: true });
    fireEvent.pointerMove(view, { button: -1, clientX: 300, clientY: 150, pointerId: 1, altKey: true });
    fireEvent.pointerUp(view, { button: 0, clientX: 300, clientY: 150, pointerId: 1 });
    await settle();
    expect(heard).toEqual([
      ["down", { screen: { x: 250, y: 100 }, world: { x: 2, y: 1 }, button: 0, shift: true, alt: false }],
      ["move", { screen: { x: 300, y: 150 }, world: { x: 3, y: 0 }, button: -1, shift: false, alt: true }],
      ["up", { screen: { x: 300, y: 150 }, world: { x: 3, y: 0 }, button: 0, shift: false, alt: false }],
    ]);
    // The press was the tool's: nothing is picked, selected, or moved.
    expect(sentOf(game.sent, "pick")).toEqual([]);
    expect(host.editor.selection.getState().entities).toEqual([]);

    // A layer of parallax 0.5 moves half as much as the camera: the renderer maps it, with the camera it drew.
    const context = contexts[0]!;
    expect(context.editor).toBe(host.editor);
    const drawnView = JSON.stringify({ center: { x: 1, y: 0 }, view: { x: 8, y: 6 } });
    expect(context.screenToWorld({ x: 250, y: 100 }, 0.5)).toEqual({ x: 1.5, y: 1 });
    // And back.
    expect(context.worldToScreen({ x: 1.5, y: 1 }, 0.5)).toEqual({ x: 250, y: 100 });
    // A layer not mapped yet is read from the renderer at the view's corners, through the camera it drew with.
    renderers.at(-1)!.calls.length = 0;
    expect(context.screenToWorld({ x: 250, y: 100 }, 0.25)).toEqual({ x: 1.25, y: 1 });
    const mapped = renderers.at(-1)!.calls.filter((call) => call.method === "screen_to_world").map((call) => call.args);
    expect(mapped).toEqual([
      [0, 0, 0.25, drawnView],
      [400, 300, 0.25, drawnView],
    ]);
    // The other direction comes from the renderer's own inverse, at two world points.
    expect(context.worldToScreen({ x: 1.25, y: 1 }, 0.25)).toEqual({ x: 250, y: 100 });
    const mappedBack = renderers.at(-1)!.calls.filter((call) => call.method === "world_to_screen").map((call) => call.args);
    expect(mappedBack).toEqual([
      [0, 0, 0.25, drawnView],
      [1, 1, 0.25, drawnView],
    ]);

    // The tool's overlay is drawn while it is active, and redrawn on request.
    drawn.calls.length = 0;
    act(() => context.redraw());
    await animationFrames(2);
    expect(drawn.calls.filter((call) => call.method === "fillText").map((call) => call.args)).toEqual([
      ["brush", 200, 100],
      ["far brush", 250, 100],
    ]);
  });

  it("tool_mappings_follow_the_renderer_and_ask_it_once_per_layer_until_the_camera_moves", async () => {
    const { host, view } = await viewport();
    const { tool, contexts } = recordingTool();
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.viewport.setTool("test.paint"));
    fireEvent.pointerDown(view, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    const context = contexts[0]!;
    const renderer = renderers.at(-1)!;
    const asked = () => renderer.calls.filter((call) => call.method === "screen_to_world" || call.method === "world_to_screen").length;
    const near = ({ x, y }: { x: number; y: number }) => ({ x: expect.closeTo(x, 6), y: expect.closeTo(y, 6) });
    /** The renderer's own answers, asked directly, and what the tool context answers. */
    const check = (cameraJson: string) => {
      for (const parallax of [0, 0.5, 1, 2]) {
        for (const [x, y] of [[0, 0], [250, 100], [400, 300], [37, 211]] as const) {
          const world = JSON.parse(renderer.screen_to_world(x, y, parallax, cameraJson));
          expect(context.screenToWorld({ x, y }, parallax)).toEqual(near(world));
          expect(context.worldToScreen(world, parallax)).toEqual(near({ x, y }));
        }
      }
    };
    const view0 = JSON.stringify({ center: { x: 0, y: 0 }, view: { x: 8, y: 6 } });
    check(view0);
    // Mapping many points costs the renderer a fixed number of questions per layer, not one per point.
    renderer.calls.length = 0;
    for (let point = 0; point < 50; point++) {
      context.screenToWorld({ x: point, y: point }, 0.5);
      context.worldToScreen({ x: point, y: point }, 0.5);
    }
    expect(asked()).toBe(0);

    // A moved camera is mapped afresh.
    fireEvent.pointerDown(view, { button: 1, clientX: 200, clientY: 150, pointerId: 2 });
    fireEvent.pointerMove(view, { button: -1, clientX: 150, clientY: 150, pointerId: 2 });
    fireEvent.pointerUp(view, { button: 1, clientX: 150, clientY: 150, pointerId: 2 });
    await waitFor(() => expect(camera()).toEqual([1, 0, 50]));
    check(JSON.stringify({ center: { x: 1, y: 0 }, view: { x: 8, y: 6 } }));
  });

  it("a_tool_pointer_maps_through_the_camera_the_renderer_draws_with", async () => {
    const camera: CameraItem = { entity: entity(1), center: { x: 10, y: 5 }, view: { x: 8, y: 6 } };
    const { host, view } = await viewport((command) => (command.type === "get_render_frame" ? { type: "render_frame", frame: frameAt(7, { camera }) } : undefined));
    const { tool, heard, contexts } = recordingTool();
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.viewport.setTool("test.paint"));
    await act(() => host.editor.commands.run("viewport.toggle-game-camera"));
    await animationFrames(2);
    const renderer = renderers.at(-1)!;
    renderer.calls.length = 0;
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    // Through the game camera, the renderer maps with the frame's camera (null), as it draws.
    expect(renderer.calls.filter((call) => call.method === "screen_to_world").map((call) => call.args)).toEqual([
      [0, 0, 1, null],
      [400, 300, 1, null],
    ]);
    expect(heard[0]![1]!.world).toEqual({ x: 11, y: 6 });
    expect(heard[0]![1]!.world).toEqual(contexts[0]!.screenToWorld({ x: 250, y: 100 }, 1));
  });

  it("escape_returns_to_select", async () => {
    const { host, game, view } = await viewport((command) => (command.type === "pick" ? { type: "picked", entities: [entity(1)] } : undefined));
    const { tool, heard } = recordingTool();
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.viewport.setTool("test.paint"));
    fireEvent.keyDown(view, { key: "Escape" });
    expect(host.editor.viewport.activeTool.getState()).toBeNull();
    expect(view.style.cursor).toBe("");

    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(heard).toEqual([]);
    expect(sentOf(game.sent, "pick")).toEqual([{ type: "pick", point: { x: 1, y: 1 } }]);
    expect(host.editor.selection.getState().entities).toEqual([entity(1)]);
  });

  it("a_tool_that_becomes_unavailable_is_deactivated", async () => {
    const { host } = await viewport();
    // Paints the selected entity of a running game, unless the game resimulates.
    const { tool } = recordingTool(
      (editor) => editor.control !== null && editor.selection.getState().entities.length > 0 && !editor.timeline.getState().timeline?.resimulating,
    );
    act(() => void host.editor.viewport.tools.register(tool));
    const active = () => host.editor.viewport.activeTool.getState();
    expect(() => host.editor.viewport.setTool("test.paint")).toThrow('the viewport tool "test.paint" is not available now');

    act(() => host.editor.selection.getState().select([entity(1)]));
    act(() => host.editor.viewport.setTool("test.paint"));
    act(() => host.editor.selection.getState().clear());
    expect(active()).toBeNull();

    act(() => host.editor.selection.getState().select([entity(1)]));
    act(() => host.editor.viewport.setTool("test.paint"));
    atTick(host, 3);
    expect(active()).toBe("test.paint");
    const resimulating = { ...host.editor.timeline.getState().timeline!, resimulating: true };
    act(() => host.editor.timeline.setState({ timeline: resimulating }));
    expect(active()).toBeNull();

    atTick(host, 3);
    act(() => host.editor.viewport.setTool("test.paint"));
    act(() => host.setControl(null));
    expect(active()).toBeNull();

    // A tool whose answer throws is not available: deactivated, its failure reported.
    const game = await testGame((command) => (command.type === "get_render_frame" ? renderFrame(7) : { type: "done" }));
    act(() => host.setControl(game.client));
    act(() => host.editor.viewport.setTool("test.paint"));
    tool.available = () => {
      throw new Error("no tilemap");
    };
    act(() => host.editor.selection.getState().select([entity(2)]));
    expect(active()).toBeNull();
    expect(host.editor.console.list().map(({ text }) => text)).toContain('Asking whether the viewport tool "test.paint" is available failed: no tilemap');
  });

  it("a_failing_tool_is_reported_in_the_editor_console", async () => {
    const { host, view } = await viewport();
    const tool: ViewportTool = {
      id: "test.broken",
      title: "Broken",
      available: () => true,
      pointerDown() {
        throw new Error("no brush chosen");
      },
      overlay() {
        throw new Error("no font loaded");
      },
      pointerCancel() {
        throw new Error("no stroke to drop");
      },
    };
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.viewport.setTool("test.broken"));
    await animationFrames(2);
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await animationFrames(2);
    expect(host.editor.viewport.activeTool.getState()).toBe("test.broken");
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerCancel(view, { pointerId: 1 });
    // The overlay is reported once while it stays active, not on every frame.
    expect(host.editor.console.list().filter(({ level }) => level === "error").map(({ text }) => text)).toEqual([
      'Drawing the viewport overlay "test.broken" failed: no font loaded',
      'The viewport tool "test.broken" handling a pointer event failed: no brush chosen',
      'The viewport tool "test.broken" handling a pointer event failed: no brush chosen',
      'The viewport tool "test.broken" cancelling a press failed: no stroke to drop',
    ]);
  });

  it("a_tool_hears_once_that_its_press_was_abandoned", async () => {
    const { host, view } = await viewport();
    const { tool, heard } = recordingTool();
    const registration = host.editor.viewport.tools.register(tool);
    act(() => void host.editor.viewport.tools.register({ id: "test.erase", title: "Erase", available: () => true }));
    const kinds = () => heard.map(([kind]) => kind);
    const press = () => {
      act(() => host.editor.viewport.setTool("test.paint"));
      heard.length = 0;
      fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    };
    const release = () => fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });

    // The browser takes the pointer: the tool hears the cancel, and no release follows.
    press();
    fireEvent.pointerCancel(view, { pointerId: 1 });
    release();
    expect(kinds()).toEqual(["down", "cancel"]);

    // Escape in the middle of a press.
    press();
    fireEvent.keyDown(view, { key: "Escape" });
    release();
    expect(kinds()).toEqual(["down", "cancel"]);

    // Another tool activated in the middle of a press.
    press();
    act(() => host.editor.viewport.setTool("test.erase"));
    release();
    expect(kinds()).toEqual(["down", "cancel"]);

    // The tool unregistered (its plugin deactivated) in the middle of a press.
    press();
    act(() => registration.dispose());
    release();
    expect(kinds()).toEqual(["down", "cancel"]);
    expect(host.editor.viewport.activeTool.getState()).toBeNull();
  });

  it("a_tool_activated_during_a_drag_ends_the_drag", async () => {
    const { host, game, view } = await viewport((command) => {
      if (command.type === "pick") return { type: "picked", entities: [entity(1)] };
      if (command.type === "get_entity") return { type: "entity", entity: entity(1), components: { "ludenea::Pose": pose(1, 1) } };
      return undefined;
    });
    const { tool, heard } = recordingTool();
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.selection.getState().select([entity(1)]));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    fireEvent.pointerMove(view, { button: -1, clientX: 300, clientY: 150, pointerId: 1 });
    act(() => host.editor.viewport.setTool("test.paint"));
    fireEvent.pointerUp(view, { button: 0, clientX: 300, clientY: 150, pointerId: 1 });
    await settle();
    // The drag ended with the tool's arrival: nothing moves, and the tool hears only what follows it.
    expect(sentOf(game.sent, "set_field")).toEqual([]);
    expect(sentOf(game.sent, "get_entity")).toEqual([]);
    expect(heard.map(([kind]) => kind)).toEqual([]);
  });

  it("a_right_drag_reaches_the_tool_without_the_context_menu", async () => {
    const { host, game, view } = await viewport();
    const { tool, heard } = recordingTool();
    act(() => void host.editor.viewport.tools.register(tool));
    // Without a tool the browser's menu shows as usual.
    expect(fireEvent.contextMenu(view)).toBe(true);
    act(() => host.editor.viewport.setTool("test.paint"));
    fireEvent.pointerDown(view, { button: 2, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(view, { button: -1, clientX: 300, clientY: 150, pointerId: 1 });
    // fireEvent returns false when the event's default is prevented.
    expect(fireEvent.contextMenu(view, { button: 2, clientX: 300, clientY: 150 })).toBe(false);
    fireEvent.pointerUp(view, { button: 2, clientX: 300, clientY: 150, pointerId: 1 });
    expect(heard.map(([kind, event]) => [kind, event?.button])).toEqual([
      ["down", 2],
      ["move", -1],
      ["up", 2],
    ]);
    expect(sentOf(game.sent, "pick")).toEqual([]);
  });

  it("closing_the_viewport_during_a_press_cancels_it_once", async () => {
    const { host, view } = await viewport();
    const { tool, heard } = recordingTool();
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.viewport.setTool("test.paint"));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    cleanup();
    // The panel is gone: changing the tool afterwards cancels nothing more.
    act(() => host.editor.viewport.setTool(null));
    expect(heard.map(([kind]) => kind)).toEqual(["down", "cancel"]);
  });

  it("escape_without_a_press_cancels_nothing", async () => {
    const { host, view } = await viewport();
    const { tool, heard } = recordingTool();
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.viewport.setTool("test.paint"));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.keyDown(view, { key: "Escape" });
    expect(heard.map(([kind]) => kind)).toEqual(["down", "up"]);
  });

  it("pan_and_zoom_work_with_a_tool_active", async () => {
    const { host, view } = await viewport();
    const { tool, heard } = recordingTool();
    act(() => void host.editor.viewport.tools.register(tool));
    act(() => host.editor.viewport.setTool("test.paint"));

    fireEvent.keyDown(view, { key: " " });
    fireEvent.pointerDown(view, { button: 0, clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerMove(view, { clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.keyUp(view, { key: " " });
    await waitFor(() => expect(camera()).toEqual([-1, -1, 50]));

    fireEvent.pointerDown(view, { button: 1, clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerMove(view, { clientX: 150, clientY: 150, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 1, clientX: 150, clientY: 150, pointerId: 1 });
    await waitFor(() => expect(camera()).toEqual([0, -1, 50]));

    fireEvent.wheel(view, { deltaY: -200, clientX: 200, clientY: 150 });
    await waitFor(() => expect(camera()?.[2]).toBeCloseTo(50 * Math.exp(0.3), 6));
    // The pans were the viewport's: the tool heard none of their presses, moves, or releases.
    expect(heard).toEqual([]);
    expect(host.editor.viewport.activeTool.getState()).toBe("test.paint");
  });
});

describe("viewport failures", () => {
  beforeEach(() => {
    localStorage.clear();
    drawn = fakeContext();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => drawn.context as unknown as CanvasRenderingContext2D);
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    script.refused = [];
    script.createError = null;
    script.createWaits = null;
  });
  // The renderers of earlier tests are not counted.
  beforeEach(() => void renderers.splice(0));

  const STOPPED = "The viewport stopped rendering: ";
  const alert = () => screen.queryByRole("alert");
  const retry = () => act(async () => fireEvent.click(screen.getByRole("button", { name: "Retry" })));
  const canvasOf = (renderer: WebRenderer) => renderer.calls.find((call) => call.method === "create")!.args[0] as HTMLCanvasElement;
  const freed = (renderer: WebRenderer) => renderer.calls.some((call) => call.method === "free");
  /** A viewport that drew its first frame, and its renderer. */
  const drawing = async (...args: Parameters<typeof viewport>) => {
    const opened = await viewport(...args);
    const renderer = renderers.at(-1)!;
    await waitFor(() => expect(drawsOf(renderer).length).toBeGreaterThan(0));
    return { ...opened, renderer };
  };
  /** Asks the paused viewport to draw again. */
  let selected = 0;
  const repaint = (host: { editor: { selection: { getState(): { select(entities: Entity[]): void } } } }) =>
    act(() => host.editor.selection.getState().select([entity(++selected)]));
  /** Loses `renderer`'s device and waits for the viewport to replace it or to stop. */
  const loseTheDevice = async (host: Parameters<typeof repaint>[0], renderer: WebRenderer) => {
    const count = renderers.length;
    renderer.fail("DeviceLost");
    repaint(host);
    await waitFor(() => expect(renderers.length > count || alert() !== null).toBe(true));
    await settle();
  };

  it("a_draw_that_keeps_failing_offers_a_retry", async () => {
    const { host, renderer } = await drawing();
    renderer.fail("Gpu");
    play(host);
    await waitFor(() => expect(alert()?.textContent).toContain(`${STOPPED}drawing failed on the GPU: out of memory`));
    // Three failing draws in a row stop the loop: no fourth.
    const draws = drawsOf(renderer).length;
    await animationFrames(5);
    expect(drawsOf(renderer).length).toBe(draws);
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
    expect(screen.queryByText(/cannot render/)).toBeNull();
    expect(freed(renderer)).toBe(true);
    expect(host.editor.console.list().map(({ text }) => text)).toEqual(["Drawing the viewport failed: drawing failed on the GPU: out of memory"]);
  });

  it("two_failing_draws_then_one_that_works_keep_drawing", async () => {
    const { host, renderer } = await drawing();
    renderer.fail("Gpu", 2);
    play(host);
    await animationFrames(5);
    renderer.fail("Gpu", 2);
    await animationFrames(5);
    expect(alert()).toBeNull();
    expect(freed(renderer)).toBe(false);
  });

  it("retry_recreates_the_renderer_and_draws_again", async () => {
    const { host, renderer } = await drawing();
    renderer.fail("Gpu");
    play(host);
    await waitFor(() => expect(alert()).not.toBeNull());
    pause(host);
    await retry();
    await waitFor(() => expect(renderers.length).toBe(2));
    const recreated = renderers.at(-1)!;
    await waitFor(() => expect(drawsOf(recreated).length).toBeGreaterThan(0));
    expect(alert()).toBeNull();
    expect(ticksPushed(recreated).at(-1)).toBe(7);
    // On a canvas of its own: the one the failing renderer configured is gone.
    expect(canvasOf(recreated)).not.toBe(canvasOf(renderer));
    expect(canvasOf(renderer).isConnected).toBe(false);
    expect(canvasOf(recreated).isConnected).toBe(true);
  });

  it("a_lost_device_recreates_the_renderer_once_with_textures_and_frame", async () => {
    const BUG = "000000000000000000000000000b0601";
    const { host, renderer } = await drawing(undefined, {}, textures([{ path: "assets/bug.png", id: BUG }]));
    await waitFor(() => expect(renderer.calls.some((call) => call.method === "set_textures")).toBe(true));
    await loseTheDevice(host, renderer);
    expect(renderers).toHaveLength(2);
    const recreated = renderers.at(-1)!;
    expect(freed(renderer)).toBe(true);
    // A lost WebGPU context cannot be configured again: the renderer gets a new canvas.
    expect(canvasOf(recreated)).not.toBe(canvasOf(renderer));
    expect(canvasOf(renderer).isConnected).toBe(false);
    await waitFor(() => expect(recreated.calls.some((call) => call.method === "set_textures")).toBe(true));
    const [sources] = recreated.calls.find((call) => call.method === "set_textures")!.args as [{ id: string }[]];
    expect(sources.map(({ id }) => id)).toEqual([BUG]);
    expect(ticksPushed(recreated).at(-1)).toBe(7);
    await waitFor(() => expect(drawsOf(recreated).length).toBeGreaterThan(0));
    expect(recreated.calls.find((call) => call.method === "resize")?.args).toEqual([400, 300, 1]);
    expect(alert()).toBeNull();
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([
      ["warning", "The GPU device of the viewport was lost (the GPU device was lost: Destroyed): the viewport created its renderer again"],
    ]);
  });

  it("a_second_loss_within_a_minute_shows_the_alert", async () => {
    const now = vi.spyOn(performance, "now");
    const { host, renderer } = await drawing();
    await loseTheDevice(host, renderer);
    const recreated = renderers.at(-1)!;
    await waitFor(() => expect(drawsOf(recreated).length).toBeGreaterThan(0));
    await loseTheDevice(host, recreated);
    expect(renderers).toHaveLength(2);
    expect(alert()?.textContent).toContain(`${STOPPED}the GPU device was lost: Destroyed`);
    expect(freed(recreated)).toBe(true);

    // Retried, the viewport draws again; a loss a minute after the last recreation is recovered from.
    await retry();
    await waitFor(() => expect(renderers).toHaveLength(3));
    const retried = renderers.at(-1)!;
    await waitFor(() => expect(drawsOf(retried).length).toBeGreaterThan(0));
    const later = performance.now() + 61_000;
    now.mockReturnValue(later);
    await loseTheDevice(host, retried);
    expect(renderers).toHaveLength(4);
    expect(alert()).toBeNull();
  });

  it("pressing_the_alert_is_not_pressing_the_viewport", async () => {
    script.createError = named("NoAdapter", "the browser offers no GPU adapter");
    const { game } = await viewport();
    await waitFor(() => expect(alert()).not.toBeNull());
    const button = screen.getByRole("button", { name: "Retry" });
    // A press on the alert neither picks nor captures the pointer, which would take the click.
    fireEvent.pointerDown(button, { button: 0, pointerId: 1 });
    await settle();
    expect(sentOf(game.sent, "pick")).toEqual([]);
    // Space and Enter reach the button.
    expect(fireEvent.keyDown(button, { key: " " })).toBe(true);
  });

  it("a_failed_creation_shows_the_alert_with_retry", async () => {
    script.createError = named("NoAdapter", "the browser offers no GPU adapter: WebGPU and WebGL2 are both unavailable");
    await viewport();
    await waitFor(() => expect(alert()?.textContent).toContain(`${STOPPED}the browser offers no GPU adapter: WebGPU and WebGL2 are both unavailable`));
    expect(renderers).toHaveLength(0);
    await retry();
    await waitFor(() => expect(renderers).toHaveLength(1));
    await waitFor(() => expect(drawsOf(renderers[0]!).length).toBeGreaterThan(0));
    expect(alert()).toBeNull();
  });

  it("without_a_renderer_a_tool_maps_through_the_editor_camera", async () => {
    script.createError = named("NoAdapter", "the browser offers no GPU adapter");
    const { host, view } = await viewport();
    await waitFor(() => expect(alert()).not.toBeNull());
    const contexts: ToolContext[] = [];
    const points: ToolPointer[] = [];
    act(() => void host.editor.viewport.tools.register({ id: "test.probe", title: "Probe", available: () => true, pointerDown: (event, context) => void (points.push(event), contexts.push(context)) }));
    act(() => host.editor.viewport.setTool("test.probe"));
    await act(() => host.editor.commands.run("viewport.pan-right"));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    // The editor camera looks at (1.28, 0) at 50 pixels per meter; a layer of parallax 0.5 sees it at (0.64, 0).
    const [context] = contexts;
    const near = (x: number, y: number) => ({ x: expect.closeTo(x, 9), y: expect.closeTo(y, 9) });
    expect(points[0]!.world).toEqual(near(2.28, 1));
    expect(context!.screenToWorld({ x: 250, y: 100 }, 0.5)).toEqual(near(1.64, 1));
    expect(context!.worldToScreen({ x: 1.64, y: 1 }, 0.5)).toEqual(near(250, 100));
  });

  it("a_panic_shows_the_alert", async () => {
    const { host, renderer } = await drawing();
    const loads = inits.length;
    const message = "panicked at crates/ludenea-render/src/gpu/mod.rs:10:5:\nthe atlas overflowed";
    renderer.panicOnDraw(message);
    play(host);
    await waitFor(() => expect(alert()?.textContent).toContain(`${STOPPED}${message}`));
    const draws = drawsOf(renderer).length;
    await animationFrames(3);
    expect(drawsOf(renderer).length).toBe(draws);
    // The module stopped in the middle of a call: the renderer is dropped, not freed.
    expect(freed(renderer)).toBe(false);
    // The trap that ends the panicking draw is the panic the alert shows, not another failure.
    expect(host.editor.console.list()).toEqual([]);
    // Retried, the renderer is created in a new instance of the module.
    await retry();
    await waitFor(() => expect(renderers).toHaveLength(2));
    expect(inits.length).toBe(loads + 1);
    await waitFor(() => expect(drawsOf(renderers[1]!).length).toBeGreaterThan(0));
    expect(alert()).toBeNull();
  });

  it("a_renderer_created_by_a_module_that_panicked_meanwhile_is_dropped_unused", async () => {
    let finish = () => {};
    script.createWaits = new Promise((resolve) => (finish = resolve));
    await viewport();
    expect(renderers).toHaveLength(1);
    const pending = renderers[0]!;
    // Another built-in's call panics while the renderer is being created.
    act(() => panic("panicked at src/web.rs:1:1:\nelsewhere"));
    await waitFor(() => expect(alert()?.textContent).toContain(`${STOPPED}panicked at src/web.rs:1:1:\nelsewhere`));
    finish();
    await settle();
    // Created by the aborted instance: nothing is called on it, not even `free`.
    expect(pending.calls.map((call) => call.method)).toEqual(["create"]);
  });

  it("missing_surface_textures_never_show_the_alert", async () => {
    const { host, renderer } = await drawing();
    renderer.fail("NoSurfaceTexture", 10);
    play(host);
    await animationFrames(15);
    expect(alert()).toBeNull();
    expect(freed(renderer)).toBe(false);
    expect(host.editor.console.list()).toEqual([]);
  });

  it("the_session_keeps_playing_while_the_viewport_is_stopped", async () => {
    const { host, game, renderer } = await drawing();
    renderer.fail("Gpu");
    play(host);
    await waitFor(() => expect(alert()).not.toBeNull());
    const sent = game.sent.length;
    // The game keeps pushing its frames, and the editor keeps its timeline playing.
    act(() => game.pushFrames(0, [bytesOf(frameAt(8)), bytesOf(frameAt(9))]));
    await animationFrames(3);
    expect(host.editor.timeline.getState().playing).toBe(true);
    expect(game.sent.slice(sent)).toEqual([]);
    // Retried, the viewport draws the newest frame the game pushed meanwhile.
    await retry();
    await waitFor(() => expect(renderers).toHaveLength(2));
    expect(ticksPushed(renderers[1]!)).toEqual([8, 9]);
  });
});

describe("viewport frames", () => {
  beforeEach(() => {
    localStorage.clear();
    drawn = fakeContext();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => drawn.context as unknown as CanvasRenderingContext2D);
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    script.refused = [];
  });

  const BUG = "000000000000000000000000000b0601";

  it("the_tick_of_a_frame_in_postcard_is_read_without_decoding_it", () => {
    // The renderer's fixture, written by the engine's postcard encoding of the frame beside it.
    const fixtures = join(import.meta.dirname, "../../../e2e/fixtures/renderer");
    const frame = JSON.parse(readFileSync(join(fixtures, "frame.json"), "utf8")) as RenderFrame;
    expect(tickOf(new Uint8Array(readFileSync(join(fixtures, "frame.postcard"))))).toBe(frame.tick);
    // A tick past one byte, and one past 32 bits, as postcard writes a `u64`.
    expect(tickOf(new Uint8Array([0xac, 0x02, 0x00]))).toBe(300);
    expect(tickOf(new Uint8Array([0x80, 0x80, 0x80, 0x80, 0x80, 0x01]))).toBe(2 ** 35);
    expect(() => tickOf(new Uint8Array([0x80]))).toThrow("the 1 bytes end inside the tick of a render frame");
  });

  it("a_texture_whose_png_cannot_be_read_is_left_out_and_logged_once", async () => {
    const project = textures([{ path: "assets/bug.png", id: BUG }, { path: "assets/gone.png", id: "000000000000000000000000000b0602" }]);
    const { host } = await viewport(undefined, {}, (request) =>
      request.type === "read_binary" && request.path === "assets/gone.png" ? { error: { code: "io", message: "no such file" } } : project(request),
    );
    const renderer = renderers.at(-1)!;
    await waitFor(() => expect(renderer.calls.some((call) => call.method === "set_textures")).toBe(true));
    const [sources] = renderer.calls.find((call) => call.method === "set_textures")!.args as [{ path: string }[]];
    expect(sources.map(({ path }) => path)).toEqual(["assets/bug.png"]);
    cleanup();
    renderPanel(host, "ludenea.viewport");
    await waitFor(() => expect(renderers.at(-1)!.calls.some((call) => call.method === "set_textures")).toBe(true));
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([["warning", "assets/gone.png: no such file"]]);
  });

  it("textures_that_cannot_be_listed_are_reported_and_the_frame_still_drawn", async () => {
    const { host } = await viewport(undefined, {}, (request) => (request.type === "list_files" ? { error: { code: "io", message: "the project folder is gone" } } : undefined));
    await waitFor(() => expect(host.editor.console.list()).toHaveLength(1));
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([["error", "Loading the textures for the viewport failed: the project folder is gone"]]);
    expect(ticksPushed(renderers.at(-1)!)).toEqual([7]);
  });

  it("a_frame_the_renderer_refuses_is_reported_once", async () => {
    const { host, game } = await viewport();
    const renderer = renderers.at(-1)!;
    vi.spyOn(renderer, "push_frame_bytes").mockImplementation(() => {
      throw Object.assign(new Error("the 3 bytes are not a render frame in postcard"), { name: "InvalidInput" });
    });
    play(host);
    await settle();
    act(() => game.pushFrames(0, [bytesOf(frameAt(8)), bytesOf(frameAt(9))]));
    expect(host.editor.console.list().map(({ text }) => text)).toEqual(["Showing the render frame failed: the 3 bytes are not a render frame in postcard"]);
  });

  it("textures_are_uploaded_when_the_renderer_is_created", async () => {
    await viewport(undefined, {}, textures([{ path: "assets/bug.png", id: BUG }]));
    const renderer = renderers.at(-1)!;
    await waitFor(() => expect(renderer.calls.some((call) => call.method === "set_textures")).toBe(true));
    const [sources, pngs] = renderer.calls.find((call) => call.method === "set_textures")!.args as [unknown, Uint8Array[]];
    expect(sources).toEqual([{ id: BUG, path: "assets/bug.png", meta: META }]);
    expect(pngs.map((png) => new TextDecoder().decode(png))).toEqual(["assets/bug.png"]);
    // New textures blank the canvas: the frame on screen is pushed again after them.
    const methods = () => renderer.calls.map((call) => call.method);
    await waitFor(() => expect(methods().lastIndexOf("push_frame")).toBeGreaterThan(methods().indexOf("set_textures")));
    expect(renderer.pushed.at(-1)!.tick).toBe(7);
  });

  it("a_refused_texture_is_logged_once", async () => {
    script.refused = [{ id: BUG, path: "assets/bug.png", message: "the 16 by 16 grid does not fit the 8 by 8 image" }];
    const { host } = await viewport(undefined, {}, textures([{ path: "assets/bug.png", id: BUG }]));
    await waitFor(() => expect(host.editor.console.list()).toHaveLength(1));
    // A viewport mounted anew uploads the textures again: the refusal is not news.
    cleanup();
    renderPanel(host, "ludenea.viewport");
    await waitFor(() => expect(renderers.at(-1)!.calls.some((call) => call.method === "set_textures")).toBe(true));
    await settle();
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([["warning", "assets/bug.png: the 16 by 16 grid does not fit the 8 by 8 image"]]);
  });

  it("a_texture_with_an_unreadable_meta_is_left_out_with_its_warning", async () => {
    const host = await testEditor([viewportPlugin, consolePlugin], textures([{ path: "assets/bug.png", id: BUG }, { path: "assets/broken.png", id: UNREADABLE }]));
    const game = await testGame((command) => (command.type === "get_render_frame" ? renderFrame(7) : { type: "done" }));
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.console");
    renderPanel(host, "ludenea.viewport");
    // The project service reports the unreadable meta once.
    act(() => host.project.push({ type: "warning", message: "assets/broken.png.meta cannot be read: expected `Meta`" }));
    const renderer = renderers.at(-1)!;
    await waitFor(() => expect(renderer.calls.some((call) => call.method === "set_textures")).toBe(true));
    const [sources] = renderer.calls.find((call) => call.method === "set_textures")!.args as [{ path: string }[]];
    expect(sources.map(({ path }) => path)).toEqual(["assets/bug.png"]);
    expect(host.project.sent.filter((request) => request.type === "read_binary").map((request) => request.path)).toEqual(["assets/bug.png"]);
    await settle();
    expect(screen.getAllByText(/broken\.png/)).toHaveLength(1);
  });

  describe("hot reload of textures", () => {
    afterEach(() => {
      script.invalidTextures = {};
    });

    const OTHER = "000000000000000000000000000b0002";
    const NEWER = "000000000000000000000000000b0003";
    const bug = { path: "assets/bug.png", id: BUG };
    const files = (path: string, kind: object | string) => ({ type: "files_changed", changes: [{ Changed: { path, kind } }] });
    /** A project whose listing is `entries` now, which a test changes. */
    const mutable = (entries: { path: string; id: string }[]) => {
      const project = {
        entries,
        meta: META,
        answer: ((request) => {
          const answered = textures(project.entries)(request) as { entries?: { kind: object | string }[] } | undefined;
          answered?.entries?.forEach((entry) => typeof entry.kind === "object" && (entry.kind = { Texture: project.meta }));
          return answered;
        }) as Answer,
      };
      return project;
    };
    const uploads = (renderer: WebRenderer) => renderer.calls.filter((call) => call.method === "set_textures");
    const uploaded = async (renderer: WebRenderer, count: number) => {
      await waitFor(() => expect(uploads(renderer)).toHaveLength(count));
      return (uploads(renderer).at(-1)!.args as [{ id: string; path: string }[], Uint8Array[]])[0];
    };
    const pushed = (host: Awaited<ReturnType<typeof viewport>>["host"], event: object) => act(() => host.project.push(event));

    it("a_changed_png_is_uploaded_again_and_redrawn", async () => {
      const project = mutable([bug, { path: "assets/other.png", id: OTHER }]);
      const { host } = await viewport(undefined, {}, project.answer);
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      project.entries = [{ ...bug, id: NEWER }, { path: "assets/other.png", id: OTHER }];
      const pushes = renderer.pushed.length;
      pushed(host, files("assets/bug.png", { Texture: META }));
      const sources = await uploaded(renderer, 2);
      expect(sources).toEqual([{ id: NEWER, path: "assets/bug.png", meta: META }, { id: OTHER, path: "assets/other.png", meta: META }]);
      await waitFor(() => expect(renderer.pushed.length).toBeGreaterThan(pushes));
    });

    it("a_changed_meta_reloads_its_texture", async () => {
      const project = mutable([bug]);
      const { host } = await viewport(undefined, {}, project.answer);
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      const grid: TextureMeta = { ...META, pixels_per_unit: 32 };
      project.meta = grid;
      project.entries = [{ ...bug, id: NEWER }];
      pushed(host, files("assets/bug.png", { Texture: grid }));
      await uploaded(renderer, 2);
      expect((uploads(renderer).at(-1)!.args as [{ meta: TextureMeta }[]])[0][0]!.meta).toEqual(grid);
    });

    it("a_created_png_joins_the_textures", async () => {
      const project = mutable([bug]);
      const { host } = await viewport(undefined, {}, project.answer);
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      project.entries = [bug, { path: "assets/other.png", id: OTHER }];
      pushed(host, { type: "files_changed", changes: [{ Created: { path: "assets/other.png", kind: { Texture: META } } }] });
      expect((await uploaded(renderer, 2)).map(({ path }) => path)).toEqual(["assets/bug.png", "assets/other.png"]);
    });

    it("a_removed_png_drops_its_texture", async () => {
      const project = mutable([bug, { path: "assets/other.png", id: OTHER }]);
      const { host } = await viewport(undefined, {}, project.answer);
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      project.entries = [{ path: "assets/other.png", id: OTHER }];
      pushed(host, { type: "files_changed", changes: [{ Removed: { path: "assets/bug.png", kind: { Texture: META } } }] });
      const sources = await uploaded(renderer, 2);
      expect(sources.map(({ path }) => path)).toEqual(["assets/other.png"]);
    });

    it("a_broken_png_keeps_the_previous_texture_and_warns_once", async () => {
      const project = mutable([bug]);
      const { host } = await viewport(undefined, {}, project.answer);
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      script.invalidTextures = { "assets/bug.png": "the image is not a PNG" };
      project.entries = [{ ...bug, id: NEWER }];
      pushed(host, files("assets/bug.png", { Texture: META }));
      pushed(host, files("assets/bug.png", { Texture: META }));
      expect(await uploaded(renderer, 3)).toEqual([{ id: BUG, path: "assets/bug.png", meta: META }]);
      expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([["warning", "assets/bug.png: the image is not a PNG"]]);
    });

    it("an_invalid_meta_keeps_the_previous_texture_and_leaves_the_warning_to_the_project", async () => {
      const project = mutable([bug]);
      const host = await testEditor([viewportPlugin, consolePlugin], project.answer);
      const game = await testGame((command) => (command.type === "get_render_frame" ? renderFrame(7) : { type: "done" }));
      act(() => host.setControl(game.client));
      renderPanel(host, "ludenea.viewport");
      renderPanel(host, "ludenea.console");
      await settle();
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      // The project could not read the new meta: it warns once, and lists the file under the null identifier.
      project.entries = [{ ...bug, id: UNREADABLE }];
      pushed(host, { type: "warning", message: "assets/bug.png.meta cannot be read: expected `Meta`" });
      pushed(host, files("assets/bug.png", { Texture: META }));
      pushed(host, files("assets/bug.png", { Texture: META }));
      expect(await uploaded(renderer, 3)).toEqual([{ id: BUG, path: "assets/bug.png", meta: META }]);
      await settle();
      expect(screen.getAllByText(/bug\.png/)).toHaveLength(1);
      expect(screen.getByText(/bug\.png\.meta cannot be read/)).toBeTruthy();
    });

    it("a_failed_reload_is_reported_and_the_next_one_still_redraws", async () => {
      const project = mutable([bug]);
      let failing = true;
      const uploadsSeen = () => renderers.at(-1)!.calls.some((call) => call.method === "set_textures");
      const { host } = await viewport(undefined, {}, (request) =>
        failing && request.type === "list_files" && uploadsSeen() ? { error: { code: "io", message: "the project folder is busy" } } : project.answer(request),
      );
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      project.entries = [{ ...bug, id: NEWER }];
      pushed(host, files("assets/bug.png", { Texture: META }));
      await waitFor(() => expect(host.editor.console.list().map(({ text }) => text)).toEqual(["Reloading the textures for the viewport failed: the project folder is busy"]));
      failing = false;
      pushed(host, files("assets/bug.png", { Texture: META }));
      expect(await uploaded(renderer, 2)).toEqual([{ id: NEWER, path: "assets/bug.png", meta: META }]);
    });

    it("the_same_break_after_a_fix_warns_again", async () => {
      const project = mutable([bug]);
      const { host } = await viewport(undefined, {}, project.answer);
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      const change = async (id: string, count: number) => {
        project.entries = [{ ...bug, id }];
        pushed(host, files("assets/bug.png", { Texture: META }));
        await uploaded(renderer, count);
      };
      script.invalidTextures = { "assets/bug.png": "the image is not a PNG" };
      await change(NEWER, 2);
      script.invalidTextures = {};
      await change(OTHER, 3);
      script.invalidTextures = { "assets/bug.png": "the image is not a PNG" };
      await change(NEWER, 4);
      expect(host.editor.console.list().map(({ text }) => text)).toEqual(["assets/bug.png: the image is not a PNG", "assets/bug.png: the image is not a PNG"]);
    });

    it("a_created_png_with_an_unreadable_meta_is_left_to_the_projects_own_warning", async () => {
      const project = mutable([bug]);
      const host = await testEditor([viewportPlugin, consolePlugin], project.answer);
      const game = await testGame((command) => (command.type === "get_render_frame" ? renderFrame(7) : { type: "done" }));
      act(() => host.setControl(game.client));
      renderPanel(host, "ludenea.viewport");
      renderPanel(host, "ludenea.console");
      await settle();
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      project.entries = [bug, { path: "assets/new.png", id: UNREADABLE }];
      pushed(host, { type: "warning", message: "assets/new.png.meta cannot be read: expected `Meta`" });
      pushed(host, { type: "files_changed", changes: [{ Created: { path: "assets/new.png", kind: "Other" } }] });
      await settle();
      await settle();
      expect(uploads(renderer)).toHaveLength(1);
      // Exactly the project's own warning is on the console, none of the viewport's.
      expect(screen.getAllByText(/new\.png/)).toHaveLength(1);
      expect(screen.getByText(/new\.png\.meta cannot be read/)).toBeTruthy();
    });

    it("a_texture_changed_before_a_lost_device_is_uploaded_to_the_new_renderer", async () => {
      const project = mutable([bug]);
      const { host } = await viewport(undefined, {}, project.answer);
      const first = renderers.at(-1)!;
      await uploaded(first, 1);
      project.entries = [{ ...bug, id: NEWER }];
      pushed(host, files("assets/bug.png", { Texture: META }));
      await waitFor(() => expect((uploads(first).at(-1)!.args as [{ id: string }[]])[0][0]!.id).toBe(NEWER));
      first.fail("DeviceLost", 1);
      play(host);
      await waitFor(() => expect(renderers.at(-1)).not.toBe(first));
      expect(await uploaded(renderers.at(-1)!, 1)).toEqual([{ id: NEWER, path: "assets/bug.png", meta: META }]);
    });

    it("texture_reload_sends_no_control_command", async () => {
      const project = mutable([bug]);
      const { host, game } = await viewport(undefined, {}, project.answer);
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      const before = game.sent.length;
      project.entries = [{ ...bug, id: NEWER }];
      pushed(host, files("assets/bug.png", { Texture: META }));
      await uploaded(renderer, 2);
      await settle();
      // The frame on screen is fetched from the host's cache or not at all: only `get_render_frame` may follow.
      expect(game.sent.slice(before).filter((command) => command.type !== "get_render_frame")).toEqual([]);
    });

    it("data_asset_changes_still_reload_the_session", async () => {
      const project = mutable([bug]);
      const { host } = await viewport(undefined, {}, project.answer);
      const renderer = renderers.at(-1)!;
      await uploaded(renderer, 1);
      const reads = () => host.project.sent.filter((request) => request.type === "read_binary").length;
      const before = reads();
      pushed(host, files("assets/settings.ron", { Data: { type: "Settings" } }));
      pushed(host, files("scenes/main.scene.ron", "Scene"));
      await settle();
      // Not textures: nothing read again, nothing uploaded. The shell's own reload of data assets is covered by the assets tests.
      expect(reads()).toBe(before);
      expect(uploads(renderer)).toHaveLength(1);
    });
  });

  it("playing_subscribes_and_pushes_each_frame_in_order", async () => {
    const { host, game } = await viewport();
    const renderer = renderers.at(-1)!;
    await waitFor(() => expect(ticksPushed(renderer)).toEqual([7]));
    play(host);
    await settle();
    expect(following(game.sent)).toEqual([
      { type: "set_pushed_layers", debug_layers: ["colliders", "contacts"] },
      { type: "subscribe", topic: { type: "render_frames" } },
    ]);
    // Base64 over a text transport, then bytes as the worker sends them: each pushed in order, as bytes.
    act(() => game.push(textFrames(0.25, [frameAt(8), frameAt(9)])));
    act(() => game.pushFrames(0.5, [bytesOf(frameAt(10)), bytesOf(frameAt(11))]));
    expect(ticksPushed(renderer)).toEqual([7, 8, 9, 10, 11]);
    const pushes = renderer.calls.filter((call) => call.method.startsWith("push_frame")).slice(1);
    expect(pushes.map((call) => call.method)).toEqual(Array(4).fill("push_frame_bytes"));
    expect(pushes.map((call) => call.args[1])).toEqual(Array(4).fill({ debug_layers: ["colliders", "contacts"], selected: [] }));
    await animationFrames(2);
    expect(host.editor.console.list()).toEqual([]);
  });

  it("pausing_unsubscribes_and_draws_the_tick_on_screen_at_alpha_1", async () => {
    let onScreen = 7;
    const { host, game } = await viewport((command) => (command.type === "get_render_frame" ? renderFrame(onScreen) : undefined));
    const renderer = renderers.at(-1)!;
    play(host);
    await settle();
    act(() => game.push(textFrames(0.5, [frameAt(8), frameAt(9)])));
    onScreen = 9;
    pause(host);
    atTick(host, 9);
    await settle();
    expect(following(game.sent).at(-1)).toEqual({ type: "unsubscribe", topic: { type: "render_frames" } });
    // The tick on screen is fetched once paused (and again after the timeline's refresh).
    expect(sentOf(game.sent, "get_render_frame").length).toBeGreaterThan(1);
    const shown = ticksPushed(renderer);
    expect(shown.slice(0, 4)).toEqual([7, 8, 9, 9]);
    expect(new Set(shown.slice(2))).toEqual(new Set([9]));
    // Frames still on their way when play stopped are not the tick on screen.
    act(() => game.push(textFrames(0, [frameAt(10)])));
    expect(ticksPushed(renderer)).toEqual(shown);
    await animationFrames(2);
    expect(drawsOf(renderer).at(-1)![0]).toBe(1);
  });

  it("stepping_while_paused_draws_the_new_tick_at_alpha_1", async () => {
    let onScreen = 7;
    const { host, game } = await viewport((command) => (command.type === "get_render_frame" ? renderFrame(onScreen) : undefined));
    const renderer = renderers.at(-1)!;
    atTick(host, 7);
    await settle();
    onScreen = 8;
    atTick(host, 8);
    await waitFor(() => expect(ticksPushed(renderer).at(-1)).toBe(8));
    expect(following(game.sent)).toEqual([]);
    await animationFrames(2);
    expect(drawsOf(renderer).at(-1)![0]).toBe(1);
  });

  it("alpha_is_extrapolated_and_clamped_at_any_speed", async () => {
    const { host, game } = await viewport();
    const renderer = renderers.at(-1)!;
    let now = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => now);
    atTick(host, 7);
    play(host);
    const alphaAfter = async (elapsed: number, from: number) => {
      now = from + elapsed;
      await animationFrames(2);
      return drawsOf(renderer).at(-1)![0];
    };
    // A quarter speed: 15 ticks a second.
    act(() => game.push(ticked(8, 15)));
    act(() => game.push(textFrames(0.2, [frameAt(8)])));
    expect(await alphaAfter(20, 1000)).toBeCloseTo(0.5, 5);
    expect(await alphaAfter(100, 1000)).toBe(1);
    // Four times the speed: 240 ticks a second.
    now = 2000;
    act(() => game.push(ticked(10, 240)));
    act(() => game.push(textFrames(0.1, [frameAt(9), frameAt(10)])));
    expect(await alphaAfter(2, 2000)).toBeCloseTo(0.58, 5);
    expect(await alphaAfter(10, 2000)).toBe(1);
  });

  it("toggling_a_layer_while_playing_keeps_frames_coming", async () => {
    const { host, game } = await viewport();
    const renderer = renderers.at(-1)!;
    play(host);
    await settle();
    await act(() => host.editor.commands.run("viewport.toggle-layer.colliders"));
    await settle();
    expect(following(game.sent)).toEqual([
      { type: "set_pushed_layers", debug_layers: ["colliders", "contacts"] },
      { type: "subscribe", topic: { type: "render_frames" } },
      { type: "set_pushed_layers", debug_layers: ["contacts"] },
    ]);
    expect(sentOf(game.sent, "get_render_frame")).toHaveLength(1);
    act(() => game.push(textFrames(0, [frameAt(8)])));
    expect(ticksPushed(renderer).at(-1)).toBe(8);
    expect(renderer.calls.at(-1)!.args[1]).toEqual({ debug_layers: ["contacts"], selected: [] });
  });

  it("a_reply_older_than_the_newest_frame_is_dropped", async () => {
    let held: ((reply: object) => void) | null = null;
    let hold = false;
    let onScreen = 7;
    const { host, game } = await viewport((command) => {
      if (command.type !== "get_render_frame") return undefined;
      if (!hold) return renderFrame(onScreen);
      return new Promise((resolve) => (held = resolve));
    });
    const renderer = renderers.at(-1)!;
    atTick(host, 7);
    await settle();
    // A request sent while paused, answered after play pushed newer frames.
    hold = true;
    atTick(host, 7, "00000000000000ee");
    await settle();
    expect(held).not.toBeNull();
    play(host);
    await settle();
    act(() => game.push(textFrames(0, [frameAt(8), frameAt(9)])));
    held!(renderFrame(7));
    await settle();
    expect(ticksPushed(renderer).slice(-2)).toEqual([8, 9]);

    // While paused: a reply for a tick the timeline left before it came is dropped.
    hold = false;
    onScreen = 9;
    pause(host);
    atTick(host, 9);
    await settle();
    hold = true;
    held = null;
    onScreen = 3;
    atTick(host, 3);
    await settle();
    onScreen = 5;
    hold = false;
    atTick(host, 5);
    held!(renderFrame(3));
    await settle();
    expect(ticksPushed(renderer).slice(-2)).toEqual([9, 5]);
  });

  it("selecting_while_paused_pushes_the_frame_again", async () => {
    const { host, game } = await viewport();
    const renderer = renderers.at(-1)!;
    await waitFor(() => expect(ticksPushed(renderer)).toEqual([7]));
    const requests = game.sent.length;
    act(() => host.editor.selection.getState().select([entity(3)]));
    expect(ticksPushed(renderer).at(-1)).toBe(7);
    expect(renderer.calls.filter((call) => call.method === "push_frame").at(-1)!.args[1]).toEqual({ debug_layers: ["colliders", "contacts"], selected: [entity(3)] });
    expect(game.sent).toHaveLength(requests);
  });

  it("game_camera_toggle_keeps_overlay_and_pick_aligned", async () => {
    const camera: CameraItem = { entity: entity(1), center: { x: 10, y: 5 }, view: { x: 8, y: 6 } };
    const label: DebugDrawFrame = { layers: { colliders: [{ entity: null, primitive: { type: "label", at: { x: 10, y: 5 }, text: "camera", color: 0xffffffff } }] } };
    const { host, game, view } = await viewport((command) => {
      if (command.type === "get_render_frame") return { type: "render_frame", frame: frameAt(7, { camera, debug: label }) };
      if (command.type === "pick") return { type: "picked", entities: [] };
      return undefined;
    });
    const renderer = renderers.at(-1)!;
    await waitFor(() => expect(drawsOf(renderer).at(-1)?.[1]).toBe(JSON.stringify({ center: { x: 0, y: 0 }, view: { x: 8, y: 6 } })));

    await act(() => host.editor.commands.run("viewport.toggle-game-camera"));
    await animationFrames(2);
    expect(drawsOf(renderer).at(-1)![1]).toBeNull();
    // 50 pixels per meter around (10, 5): the label at the camera's center is drawn at the view's.
    await waitFor(() => expect(drawn.calls.filter((call) => call.method === "fillText").at(-1)?.args).toEqual(["camera", 200, 150]));
    fireEvent.pointerDown(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 0, clientX: 250, clientY: 100, pointerId: 1 });
    await settle();
    expect(sentOf(game.sent, "pick").at(-1)).toEqual({ type: "pick", point: { x: 11, y: 6 } });
    // Through the game camera the renderer draws and picks with the frame's camera, as shown.
    expect(renderer.calls.filter((call) => call.method === "pick").at(-1)!.args).toEqual([250, 100, null]);

    await act(() => host.editor.commands.run("viewport.toggle-game-camera"));
    await animationFrames(2);
    expect(drawsOf(renderer).at(-1)![1]).not.toBeNull();
    expect(host.editor.commands.list().find((command) => command.id === "viewport.toggle-game-camera")?.title).toBe("View through the game camera");
  });

  it("a_reply_sent_before_a_change_at_the_same_tick_is_dropped", async () => {
    const debug = (text: string): DebugDrawFrame => ({ layers: { contacts: [{ entity: null, primitive: { type: "label", at: { x: 0, y: 0 }, text, color: 0 } }] } });
    let held: ((reply: object) => void) | null = null;
    let hold = false;
    const { host, view } = await viewport((command) => {
      if (command.type === "get_entity") return { type: "entity", entity: entity(1), components: { "ludenea::Pose": pose(1, 1) } };
      if (command.type !== "get_render_frame") return undefined;
      if (hold) return new Promise((resolve) => (held = resolve));
      return { type: "render_frame", frame: frameAt(7, { debug: debug("current") }) };
    });
    const renderer = renderers.at(-1)!;
    atTick(host, 7);
    await settle();
    // A request at tick 7 held while the layers change and an edit lands at the same tick.
    hold = true;
    atTick(host, 7, "00000000000000ee");
    await settle();
    expect(held).not.toBeNull();
    hold = false;
    await act(() => host.editor.commands.run("viewport.toggle-layer.colliders"));
    act(() => host.editor.selection.getState().select([entity(1)]));
    fireEvent.keyDown(view, { key: "ArrowRight" });
    await settle();
    held!({ type: "render_frame", frame: frameAt(7, { debug: debug("stale") }) });
    await settle();
    await waitFor(() => expect(renderer.pushed.at(-1)!.debug).toEqual(debug("current")));
    expect(renderer.pushed.some((frame) => JSON.stringify(frame.debug) === JSON.stringify(debug("stale")))).toBe(false);
  });

  it("paused_the_tick_on_screen_is_drawn_even_behind_the_frames_pushed", async () => {
    let onScreen = 7;
    const { host, game } = await viewport((command) => (command.type === "get_render_frame" ? renderFrame(onScreen) : undefined));
    const renderer = renderers.at(-1)!;
    atTick(host, 7);
    play(host);
    await settle();
    // Play pushed tick 10 before the pause; the timeline stopped on tick 9.
    act(() => game.push(textFrames(0, [frameAt(8), frameAt(9)])));
    act(() => game.push(textFrames(0, [frameAt(10)])));
    onScreen = 9;
    pause(host);
    atTick(host, 9);
    await waitFor(() => expect(ticksPushed(renderer).at(-1)).toBe(9));
    // Stepping back, then forward again.
    onScreen = 8;
    atTick(host, 8);
    await waitFor(() => expect(ticksPushed(renderer).at(-1)).toBe(8));
    onScreen = 9;
    atTick(host, 9);
    await waitFor(() => expect(ticksPushed(renderer).at(-1)).toBe(9));
  });

  it("frames_all_dropped_leave_the_timing_as_it_was", async () => {
    const { host, game } = await viewport();
    const renderer = renderers.at(-1)!;
    let now = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => now);
    atTick(host, 9);
    play(host);
    act(() => game.push(ticked(9, 15)));
    act(() => game.push(textFrames(0.2, [frameAt(9)])));
    now = 1010;
    // Older than the newest shown: dropped, and its progress with it.
    act(() => game.push(textFrames(0.9, [frameAt(8)])));
    now = 1020;
    await animationFrames(2);
    expect(drawsOf(renderer).at(-1)![0]).toBeCloseTo(0.5, 5);
  });

  it("moving_the_view_leaves_the_game_camera_from_what_it_shows", async () => {
    const camera: CameraItem = { entity: entity(1), center: { x: 10, y: 5 }, view: { x: 8, y: 6 } };
    const { host, view } = await viewport((command) => (command.type === "get_render_frame" ? { type: "render_frame", frame: frameAt(7, { camera, debug: circleAt(1, 12, 5) }) } : undefined));
    const renderer = renderers.at(-1)!;
    const drawnCamera = () => JSON.parse(drawsOf(renderer).at(-1)![1] ?? "null") as { center: { x: number; y: number }; view: { x: number; y: number } } | null;
    const throughGame = async () => {
      await act(() => host.editor.commands.run("viewport.toggle-game-camera"));
      await animationFrames(2);
      expect(drawnCamera()).toBeNull();
    };
    // The game camera shows (10, 5) at 50 pixels per meter in the 400 by 300 view.
    await throughGame();
    await act(() => host.editor.commands.run("viewport.pan-right"));
    await waitFor(() => expect(drawnCamera()).toEqual({ center: { x: 10 + 64 / 50, y: 5 }, view: { x: 8, y: 6 } }));
    await throughGame();
    await act(() => host.editor.commands.run("viewport.zoom-in"));
    await waitFor(() => expect(drawnCamera()).toEqual({ center: { x: 10, y: 5 }, view: { x: 400 / 62.5, y: 300 / 62.5 } }));
    await throughGame();
    fireEvent.pointerDown(view, { button: 1, clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerMove(view, { clientX: 150, clientY: 150, pointerId: 1 });
    fireEvent.pointerUp(view, { button: 1, clientX: 150, clientY: 150, pointerId: 1 });
    await waitFor(() => expect(drawnCamera()).toEqual({ center: { x: 11, y: 5 }, view: { x: 8, y: 6 } }));
    await throughGame();
    fireEvent.wheel(view, { deltaY: 0, clientX: 200, clientY: 150 });
    await waitFor(() => expect(drawnCamera()).toEqual({ center: { x: 10, y: 5 }, view: { x: 8, y: 6 } }));
    await throughGame();
    act(() => host.editor.selection.getState().select([entity(1)]));
    await act(() => host.editor.commands.run("viewport.frame-selection"));
    await waitFor(() => expect(drawnCamera()?.center).toEqual({ x: 12, y: 5 }));
  });

  it("a_new_game_subscribes_again", async () => {
    const { host, game } = await viewport();
    const renderer = renderers.at(-1)!;
    play(host);
    await settle();
    const other = await testGame((command) => (command.type === "get_render_frame" ? renderFrame(0) : { type: "done" }), { debug_layers: ["colliders"] });
    act(() => host.setControl(other.client));
    await settle();
    // The game left is not told anything more; the new one, playing, is followed.
    expect(following(game.sent)).toHaveLength(2);
    act(() => other.push(ticked(1, 60)));
    await settle();
    expect(following(other.sent)).toEqual([
      { type: "set_pushed_layers", debug_layers: ["colliders"] },
      { type: "subscribe", topic: { type: "render_frames" } },
    ]);
    act(() => game.push(textFrames(0, [frameAt(50)])));
    act(() => other.pushFrames(0, [bytesOf(frameAt(2))]));
    expect(ticksPushed(renderer).at(-1)).toBe(2);
    expect(ticksPushed(renderer)).not.toContain(50);
  });

  it("drawing_between_frames_sends_no_request", async () => {
    const { host, game } = await viewport();
    const renderer = renderers.at(-1)!;
    play(host);
    await settle();
    act(() => game.pushFrames(0, [bytesOf(frameAt(8))]));
    const requests = game.sent.length;
    const draws = drawsOf(renderer).length;
    await animationFrames(5);
    // Every animation frame draws while playing, between the frames the host pushes.
    expect(drawsOf(renderer).length).toBeGreaterThanOrEqual(draws + 4);
    expect(game.sent).toHaveLength(requests);
  });
});
