import type { DebugDrawFrame, Entity, GameInfo } from "@ludenea/control-client";
import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { viewportPlugin } from "../src/viewport";
import { screenToWorld, worldToScreen, zoomAt } from "../src/viewport/camera";
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

  it("layer_toggle_commands_follow_game_info", async () => {
    const { host, game } = await viewport();
    const toggles = () => host.editor.commands.list().map((command) => command.id).filter((id) => id.startsWith("viewport.toggle-layer."));
    expect(toggles()).toEqual(["viewport.toggle-layer.colliders", "viewport.toggle-layer.contacts"]);
    await waitFor(() => expect(sentOf(game.sent, "get_debug_draw").at(-1)).toEqual({ type: "get_debug_draw", layers: ["colliders", "contacts"] }));

    await act(() => host.editor.commands.run("viewport.toggle-layer.colliders"));
    await waitFor(() => expect(sentOf(game.sent, "get_debug_draw").at(-1)).toEqual({ type: "get_debug_draw", layers: ["contacts"] }));

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
});
