import type { Entity, FileEntry, TextureMeta } from "@ludenea/control-client";
import type { OverlayContext, ToolContext, ToolPointer, ViewportTool } from "@ludenea/editor-shell";
import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { tilemapPlugin } from "../src/tilemap";
import { areaBetween, cellAt, type Pose, type Tilemap } from "../src/tilemap/paint";
import { script } from "./fake-render-wasm";
import { renderFrame, renderPanel, settle, testEditor, testGame } from "./support";

type Command = { type: string; [key: string]: unknown };

const TILESET = "000000000000000000000000000000a1";
const META: TextureMeta = { pixels_per_unit: 16, grid: { frame_width: 16, frame_height: 16, margin: 0, spacing: 0 }, pivot: null, filter: "Nearest" };
const SMALL_TILESET = "000000000000000000000000000000a2";
const files: FileEntry[] = [
  { path: "assets/tiles.png", id: TILESET, kind: { Texture: META } },
  { path: "assets/small.png", id: SMALL_TILESET, kind: { Texture: META } },
];
/** What the fake renderer cuts `assets/tiles.png` into: three frames of 16 by 16 pixels side by side. */
const TILE_FRAMES = {
  width: 48,
  height: 16,
  frames: [0, 16, 32].map((x) => ({ x, y: 0, width: 16, height: 16, pivot: [8, 8] as [number, number] })),
};

const MAP = { index: 4, generation: 1 };
const OTHER = { index: 5, generation: 0 };
const tilemap = (extra: Partial<Tilemap> = {}): Tilemap => ({
  tileset: TILESET,
  cell_size: { x: 1, y: 1 },
  width: 16,
  height: 9,
  tiles: Array.from({ length: 16 * 9 }, () => null),
  layer: "background",
  order: 0,
  ...extra,
});
const pose = (x: number, y: number, rotation = { cos: 1, sin: 0 }): Pose => ({ position: { x, y }, rotation });
const json = (value: unknown) => ({ type: "json", value });
/** The game's render layers: the tilemap's `background` moves at half the camera's speed. */
const LAYERS = () => [
  { name: "background", parallax: backgroundParallax },
  { name: "world", parallax: 1 },
];
let backgroundParallax = 0.5;

/**
 * An editor with the tilemap plugin over a game holding the tilemap `MAP` (at `(0, 0)`, on
 * `background`) and the entity `OTHER` (a pose only), on a project holding its tileset; the
 * Tile palette shown. `refuse` gives the game's answer to `paint_tiles` as a refusal.
 */
async function painting(options: { refuse?: string; read?: (command: Command) => object | undefined } = {}) {
  const host = await testEditor([tilemapPlugin], (request) => {
    if (request.type === "list_files") return { type: "files", entries: files };
    if (request.type === "read_binary") return { type: "bytes", bytes: btoa(request.path as string) };
    return undefined;
  });
  const game = await testGame((command: Command) => {
    const read = options.read?.(command);
    if (read) return read;
    if (command.type === "get_entity") {
      const entity = command.entity as Entity;
      const components = entity.index === MAP.index ? { "ludenea::Tilemap": json(tilemap()), "ludenea::Pose": json(pose(0, 0)) } : { "ludenea::Pose": json(pose(3, 3)) };
      return { type: "entity", entity, components };
    }
    if (command.type === "get_render_frame") return { type: "render_frame", frame: { ...renderFrame(0).frame, layers: LAYERS() } };
    if (command.type === "paint_tiles" && options.refuse) return { error: { code: "invalid_command", message: options.refuse } };
    return { type: "done" };
  });
  act(() => host.setControl(game.client));
  renderPanel(host, "ludenea.tile-palette");
  await settle();
  return { host, game };
}

async function select(host: Awaited<ReturnType<typeof painting>>["host"], entity: Entity) {
  act(() => host.editor.selection.getState().select([entity]));
  await settle();
  await settle();
}

/** A tool context mapping a screen point to world units a tenth of its pixels (y up, as the world), recording the parallax asked. */
function toolContext(host: Awaited<ReturnType<typeof painting>>["host"]) {
  const parallaxes: number[] = [];
  let redraws = 0;
  const context: ToolContext = {
    editor: host.editor,
    screenToWorld(screen, parallax) {
      parallaxes.push(parallax);
      return { x: screen.x / 10, y: screen.y / 10 };
    },
    worldToScreen(point, parallax) {
      parallaxes.push(parallax);
      return { x: point.x * 10, y: point.y * 10 };
    },
    redraw: () => void redraws++,
  };
  return { context, parallaxes, redraws: () => redraws };
}

const pointer = (x: number, y: number, button = 0): ToolPointer => ({ screen: { x, y }, world: { x: x / 10, y: y / 10 }, button, shift: false, alt: false });
const tool = (host: Awaited<ReturnType<typeof painting>>["host"], id: string): ViewportTool => host.editor.viewport.tools.list().find((tool) => tool.id === id)!;
const sentOf = (sent: Command[], type: string) => sent.filter((command) => command.type === type);

/** Drags with `id` from screen `(from)` to `(to)`: a press, a move, and a release, with `button`. */
function drag(host: Awaited<ReturnType<typeof painting>>["host"], id: string, from: [number, number], to: [number, number], button = 0) {
  const { context } = toolContext(host);
  const active = tool(host, id);
  active.pointerDown!(pointer(...from, button), context);
  active.pointerMove!(pointer(...to, -1), context);
  active.pointerUp!(pointer(...to, button), context);
}

/** A 2D context recording the rectangles drawn. */
function recordingCanvas() {
  const rectangles: number[][] = [];
  const canvas = new Proxy({} as Record<string | symbol, unknown>, {
    get: (target, key) => (key in target ? target[key] : key === "fillRect" ? (...args: number[]) => void rectangles.push(args) : () => {}),
    set: (target, key, value) => ((target[key] = value), true),
  });
  return { canvas: canvas as unknown as CanvasRenderingContext2D, rectangles };
}

describe("tilemap painting", () => {
  beforeEach(() => {
    backgroundParallax = 0.5;
    script.frames = { "assets/tiles.png": TILE_FRAMES, "assets/small.png": { width: 16, height: 16, frames: [TILE_FRAMES.frames[0]!] } };
  });
  afterEach(() => {
    cleanup();
    script.frames = {};
  });

  it("cell_at_maps_world_points_to_cells", () => {
    const map = tilemap();
    expect(cellAt({ x: 0.5, y: 0.5 }, pose(0, 0), map)).toEqual({ x: 0, y: 0 });
    expect(cellAt({ x: 0, y: 0 }, pose(0, 0), map)).toEqual({ x: 0, y: 0 });
    expect(cellAt({ x: 15.99, y: 8.99 }, pose(0, 0), map)).toEqual({ x: 15, y: 8 });
    // The grid's far edges, and below or left of it, are outside.
    expect(cellAt({ x: 16, y: 0.5 }, pose(0, 0), map)).toBeNull();
    expect(cellAt({ x: 0.5, y: 9 }, pose(0, 0), map)).toBeNull();
    expect(cellAt({ x: -0.01, y: 0.5 }, pose(0, 0), map)).toBeNull();
    expect(cellAt({ x: 0.5, y: -0.01 }, pose(0, 0), map)).toBeNull();
    // Cells of 2 by 0.5 units.
    expect(cellAt({ x: 3, y: 1.2 }, pose(0, 0), tilemap({ cell_size: { x: 2, y: 0.5 } }))).toEqual({ x: 1, y: 2 });
    // An empty grid has no cell.
    expect(cellAt({ x: 0, y: 0 }, pose(0, 0), tilemap({ width: 0, height: 0, tiles: [] }))).toBeNull();
  });

  it("cell_at_follows_the_pose", () => {
    const map = tilemap();
    expect(cellAt({ x: 10.5, y: -4.5 }, pose(10, -5), map)).toEqual({ x: 0, y: 0 });
    expect(cellAt({ x: 25.5, y: 3.5 }, pose(10, -5), map)).toEqual({ x: 15, y: 8 });
    expect(cellAt({ x: 9.9, y: -4.5 }, pose(10, -5), map)).toBeNull();
    expect(cellAt({ x: 0.5, y: 0.5 }, pose(10, -5), map)).toBeNull();
    // The renderer draws a tilemap at its position only: a rotation does not turn the grid.
    expect(cellAt({ x: 10.5, y: -4.5 }, pose(10, -5, { cos: 0, sin: 1 }), map)).toEqual({ x: 0, y: 0 });
  });

  it("area_between_is_clamped_to_the_grid", () => {
    const map = tilemap();
    expect(areaBetween({ x: 2, y: 3 }, { x: 5, y: 1 }, map)).toEqual({ x: 2, y: 1, width: 4, height: 3 });
    expect(areaBetween({ x: 5, y: 1 }, { x: 2, y: 3 }, map)).toEqual({ x: 2, y: 1, width: 4, height: 3 });
    expect(areaBetween({ x: 7, y: 7 }, { x: 7, y: 7 }, map)).toEqual({ x: 7, y: 7, width: 1, height: 1 });
    // Past each edge of the 16 by 9 grid.
    expect(areaBetween({ x: 2, y: 3 }, { x: -3, y: 20 }, map)).toEqual({ x: 0, y: 3, width: 3, height: 6 });
    expect(areaBetween({ x: 14, y: 1 }, { x: 40, y: -8 }, map)).toEqual({ x: 14, y: 0, width: 2, height: 2 });
  });

  it("the_palette_shows_the_tilesets_frames_and_keeps_the_chosen_tile", async () => {
    const { host } = await painting();
    expect(screen.getByText(/Select one entity with a ludenea::Tilemap and a ludenea::Pose/)).toBeTruthy();
    await select(host, MAP);
    const tiles = screen.getByRole("group", { name: "Tiles of assets/tiles.png" });
    const buttons = within(tiles).getAllByRole("button");
    expect(buttons.map((button) => button.getAttribute("aria-label"))).toEqual(["Tile 0", "Tile 1", "Tile 2"]);
    // Each shows its own frame of the PNG the editor read.
    expect(buttons.map((button) => button.querySelector("svg")!.getAttribute("viewBox"))).toEqual(["0 0 16 16", "16 0 16 16", "32 0 16 16"]);
    expect(buttons[0]!.querySelector("image")!.getAttribute("href")).toBe(`data:image/png;base64,${btoa("assets/tiles.png")}`);
    expect(buttons.map((button) => button.getAttribute("aria-pressed"))).toEqual(["false", "false", "false"]);

    fireEvent.click(screen.getByRole("button", { name: "Tile 2" }));
    expect(screen.getByRole("button", { name: "Tile 2" }).getAttribute("aria-pressed")).toBe("true");
    // Kept across selections.
    await select(host, OTHER);
    expect(screen.queryByRole("group", { name: "Tiles of assets/tiles.png" })).toBeNull();
    await select(host, MAP);
    expect(screen.getByRole("button", { name: "Tile 2" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("dragging_sends_one_paint_tiles_on_release", async () => {
    const { host, game } = await painting();
    await select(host, MAP);
    fireEvent.click(screen.getByRole("button", { name: "Tile 2" }));
    await act(() => host.editor.commands.run("tilemap.paint"));
    expect(host.editor.viewport.activeTool.getState()).toBe("tilemap.paint");

    const { context, parallaxes, redraws } = toolContext(host);
    const paint = tool(host, "tilemap.paint");
    // From cell (2, 3) to cell (5, 1), on the tilemap's layer.
    paint.pointerDown!(pointer(25, 35), context);
    paint.pointerMove!(pointer(55, 15, -1), context);
    // Previewed in the overlay, on the tilemap's layer: cells 2 to 5 by 1 to 3.
    const { canvas, rectangles } = recordingCanvas();
    const overlay: OverlayContext = { canvas, worldToScreen: context.worldToScreen, tick: 0, selection: [MAP] };
    paint.overlay!(overlay);
    expect(rectangles).toEqual([[20, 10, 40, 30]]);
    expect(sentOf(game.sent, "paint_tiles")).toEqual([]);

    paint.pointerUp!(pointer(55, 15), context);
    await settle();
    expect(sentOf(game.sent, "paint_tiles")).toEqual([{ type: "paint_tiles", entity: MAP, area: { x: 2, y: 1, width: 4, height: 3 }, tile: 2 }]);
    expect(new Set(parallaxes)).toEqual(new Set([0.5]));
    expect(redraws()).toBe(3);
    // The preview goes with the release.
    rectangles.length = 0;
    paint.overlay!(overlay);
    expect(rectangles).toEqual([]);
  });

  it("a_press_outside_the_grid_or_cancelled_paints_nothing", async () => {
    const { host, game } = await painting();
    await select(host, MAP);
    fireEvent.click(screen.getByRole("button", { name: "Tile 1" }));
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    // Outside the grid.
    drag(host, "tilemap.paint", [-5, 5], [25, 25]);
    // The middle button pans.
    drag(host, "tilemap.paint", [5, 5], [25, 25], 1);
    // Cancelled before its release.
    const { context } = toolContext(host);
    const paint = tool(host, "tilemap.paint");
    paint.pointerDown!(pointer(5, 5), context);
    paint.pointerCancel!(context);
    paint.pointerUp!(pointer(25, 25), context);
    await settle();
    expect(sentOf(game.sent, "paint_tiles")).toEqual([]);

    // A drag leaving the grid is clamped to it.
    drag(host, "tilemap.paint", [155, 85], [400, -50]);
    await settle();
    expect(sentOf(game.sent, "paint_tiles")).toEqual([{ type: "paint_tiles", entity: MAP, area: { x: 15, y: 0, width: 1, height: 9 }, tile: 1 }]);
  });

  it("painting_without_a_chosen_tile_says_so", async () => {
    const { host, game } = await painting();
    await select(host, MAP);
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    drag(host, "tilemap.paint", [5, 5], [25, 25]);
    await settle();
    expect(sentOf(game.sent, "paint_tiles")).toEqual([]);
    expect(host.editor.console.list().map((entry) => entry.text)).toEqual(["Painting tiles failed: choose a tile in the Tile palette first"]);
  });

  it("right_drag_erases", async () => {
    const { host, game } = await painting();
    await select(host, MAP);
    fireEvent.click(screen.getByRole("button", { name: "Tile 2" }));
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    drag(host, "tilemap.paint", [5, 5], [15, 5], 2);
    await settle();
    // The erase tool erases with the main button.
    await act(() => host.editor.commands.run("tilemap.erase"));
    expect(host.editor.viewport.activeTool.getState()).toBe("tilemap.erase");
    drag(host, "tilemap.erase", [35, 35], [35, 35]);
    await settle();
    expect(sentOf(game.sent, "paint_tiles")).toEqual([
      { type: "paint_tiles", entity: MAP, area: { x: 0, y: 0, width: 2, height: 1 }, tile: null },
      { type: "paint_tiles", entity: MAP, area: { x: 3, y: 3, width: 1, height: 1 }, tile: null },
    ]);
  });

  it("tools_are_unavailable_while_playing", async () => {
    const { host } = await painting();
    await select(host, MAP);
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    act(() => host.editor.timeline.setState({ playing: true }));
    expect(host.editor.viewport.activeTool.getState()).toBeNull();
    expect(() => host.editor.viewport.setTool("tilemap.paint")).toThrow('the viewport tool "tilemap.paint" is not available now');
    expect(() => host.editor.viewport.setTool("tilemap.erase")).toThrow('the viewport tool "tilemap.erase" is not available now');
    act(() => host.editor.timeline.setState({ playing: false }));
    act(() => host.editor.viewport.setTool("tilemap.erase"));
    expect(host.editor.viewport.activeTool.getState()).toBe("tilemap.erase");
  });

  it("tools_follow_the_selected_tilemap", async () => {
    const { host, game } = await painting();
    // Nothing selected, an entity without a tilemap, or two entities: nothing to paint.
    expect(() => host.editor.viewport.setTool("tilemap.paint")).toThrow(/not available/);
    await select(host, OTHER);
    expect(() => host.editor.viewport.setTool("tilemap.paint")).toThrow(/not available/);
    act(() => host.editor.selection.getState().select([MAP, OTHER]));
    await settle();
    expect(() => host.editor.viewport.setTool("tilemap.paint")).toThrow(/not available/);

    await select(host, MAP);
    fireEvent.click(screen.getByRole("button", { name: "Tile 0" }));
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    // The tilemap moved: cells follow its pose as the game reports it.
    act(() => game.push({ type: "entity_changed", entity: MAP, components: { "ludenea::Pose": json(pose(2, 0)) }, removed: [] }));
    drag(host, "tilemap.paint", [25, 5], [25, 5]);
    await settle();
    expect(sentOf(game.sent, "paint_tiles")).toEqual([{ type: "paint_tiles", entity: MAP, area: { x: 0, y: 0, width: 1, height: 1 }, tile: 0 }]);
    // Its tilemap removed, the tool leaves.
    act(() => game.push({ type: "entity_changed", entity: MAP, components: {}, removed: ["ludenea::Tilemap"] }));
    expect(host.editor.viewport.activeTool.getState()).toBeNull();
    expect(() => host.editor.viewport.setTool("tilemap.paint")).toThrow(/not available/);
    // And the game gone, so does the tilemap.
    act(() => game.push({ type: "entity_changed", entity: MAP, components: { "ludenea::Tilemap": json(tilemap()) }, removed: [] }));
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    act(() => host.setControl(null));
    expect(host.editor.viewport.activeTool.getState()).toBeNull();
    expect(() => host.editor.viewport.setTool("tilemap.paint")).toThrow(/not available/);
  });

  it("painting_paused_past_tick_0_sends_the_same_command", async () => {
    const { host, game } = await painting();
    await select(host, MAP);
    fireEvent.click(screen.getByRole("button", { name: "Tile 1" }));
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    drag(host, "tilemap.paint", [5, 5], [25, 15]);
    await settle();
    act(() => host.editor.timeline.setState(({ timeline }) => ({ playing: false, timeline: timeline && { ...timeline, current: 30 } })));
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    drag(host, "tilemap.paint", [5, 5], [25, 15]);
    await settle();
    const [atZero, atThirty] = sentOf(game.sent, "paint_tiles");
    expect(atZero).toEqual({ type: "paint_tiles", entity: MAP, area: { x: 0, y: 0, width: 3, height: 2 }, tile: 1 });
    expect(atThirty).toEqual(atZero);
  });

  it("a_refused_paint_is_shown_in_the_console", async () => {
    const refusal = "entity 4v1, component `ludenea::Tilemap`, paint_tiles: area x 0, y 0, 1 by 1 is outside the grid of 0 by 0 cells";
    const { host } = await painting({ refuse: refusal });
    await select(host, MAP);
    fireEvent.click(screen.getByRole("button", { name: "Tile 2" }));
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    drag(host, "tilemap.paint", [5, 5], [5, 5]);
    await settle();
    drag(host, "tilemap.paint", [5, 5], [5, 5], 2);
    await settle();
    expect(host.editor.console.list().map((entry) => entry.text)).toEqual([`Painting tiles failed: ${refusal}`, `Erasing tiles failed: ${refusal}`]);
  });

  it("a_failed_read_is_shown_in_the_console", async () => {
    const failing = { entity: { code: "internal", message: "the game closed" }, layers: null as { code: string; message: string } | null };
    const { host } = await painting({
      read: (command) => {
        if (command.type === "get_entity" && (command.entity as Entity).index === OTHER.index) return { error: failing.entity };
        if (command.type === "get_entity" && (command.entity as Entity).index === 9) return { error: { code: "no_such_entity", message: "entity 9v0 is not alive" } };
        if (command.type === "get_render_frame" && failing.layers) return { error: failing.layers };
        return undefined;
      },
    });
    await select(host, OTHER);
    // A selection of an entity gone says nothing here: the inspector clears it.
    await select(host, { index: 9, generation: 0 });
    failing.layers = { code: "internal", message: "the extraction panicked" };
    await select(host, MAP);
    expect(() => host.editor.viewport.setTool("tilemap.paint")).toThrow(/not available/);
    expect(host.editor.console.list().map((entry) => entry.text)).toEqual([
      "Reading the selected entity 5v0 for tilemap painting failed: the game closed",
      "Reading the selected entity 4v1 for tilemap painting failed: the extraction panicked",
    ]);
    // The layers are read again on the next selection.
    failing.layers = null;
    await select(host, OTHER);
    await select(host, MAP);
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    expect(host.editor.viewport.activeTool.getState()).toBe("tilemap.paint");
  });

  it("a_despawned_tilemap_leaves_nothing_to_paint", async () => {
    const { host, game } = await painting();
    await select(host, MAP);
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    // An undo of its spawn, a seek before it, or another branch: the entity is gone.
    act(() => game.push({ type: "entity_despawned", entity: MAP }));
    expect(host.editor.viewport.activeTool.getState()).toBeNull();
    expect(() => host.editor.viewport.setTool("tilemap.paint")).toThrow(/not available/);
    expect(screen.queryByRole("group", { name: "Tiles of assets/tiles.png" })).toBeNull();
    expect(screen.getByText(/Select one entity with a ludenea::Tilemap/)).toBeTruthy();
    // Alive again (a redo of its spawn), it reports all its components.
    act(() => game.push({ type: "entity_changed", entity: MAP, components: { "ludenea::Tilemap": json(tilemap()), "ludenea::Pose": json(pose(0, 0)) }, removed: [] }));
    await settle();
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    expect(host.editor.viewport.activeTool.getState()).toBe("tilemap.paint");
  });

  it("a_chosen_tile_past_the_tilesets_frames_says_so", async () => {
    const { host, game } = await painting();
    await select(host, MAP);
    fireEvent.click(screen.getByRole("button", { name: "Tile 2" }));
    // The tileset changes to one of a single frame: tile 2 is not in it, and the palette says so.
    act(() => game.push({ type: "entity_changed", entity: MAP, components: { "ludenea::Tilemap": json(tilemap({ tileset: SMALL_TILESET })) }, removed: [] }));
    await settle();
    await settle();
    expect(within(screen.getByRole("group", { name: "Tiles of assets/small.png" })).getAllByRole("button")).toHaveLength(1);
    expect(screen.getByText("Tile 2 (not in this tileset)")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Tile 0" }));
    expect(screen.queryByText(/not in this tileset/)).toBeNull();
  });

  it("the_layer_parallax_is_read_again_on_each_selection", async () => {
    const { host } = await painting();
    await select(host, MAP);
    backgroundParallax = 0.25;
    await select(host, OTHER);
    await select(host, MAP);
    fireEvent.click(screen.getByRole("button", { name: "Tile 0" }));
    act(() => host.editor.viewport.setTool("tilemap.paint"));
    const { context, parallaxes } = toolContext(host);
    const paint = tool(host, "tilemap.paint");
    paint.pointerDown!(pointer(5, 5), context);
    paint.pointerUp!(pointer(5, 5), context);
    expect(new Set(parallaxes)).toEqual(new Set([0.25]));
  });

  it("b_and_e_choose_the_tools", async () => {
    const { host } = await painting();
    expect(host.registry.shortcuts.get("B")).toBe("tilemap.paint");
    expect(host.registry.shortcuts.get("E")).toBe("tilemap.erase");
    // Through the commands, which say why when the tool cannot be used.
    await expect(host.editor.commands.run("tilemap.paint")).rejects.toThrow('the viewport tool "tilemap.paint" is not available now');
  });
});
