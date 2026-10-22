import type { FileEntry, TextureMeta, TypeSchema } from "@ludenea/control-client";
import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { builtinPlugins } from "../src";
import { inspectorPlugin } from "../src/inspector";
import { texturesPlugin } from "../src/textures";
import { script } from "./fake-render-wasm";
import { renderPanel, settle, testEditor, testGame } from "./support";

const HERO = "0000000000000000000000000000000d";
const SHEET = "0000000000000000000000000000000e";
const META: TextureMeta = { pixels_per_unit: 16, grid: { frame_width: 8, frame_height: 8, margin: 0, spacing: 0 }, pivot: [2, 6], filter: "Nearest" };
const files: FileEntry[] = [
  { path: "assets/hero.png", id: HERO, kind: { Texture: META } },
  { path: "assets/hero.sheet.ron", id: SHEET, kind: { Data: { type: "ludenea::SpriteSheet" } } },
];
/** What the fake renderer cuts `assets/hero.png` into: two frames of 8 by 8 pixels side by side. */
const HERO_FRAMES = {
  width: 16,
  height: 8,
  frames: [
    { x: 0, y: 0, width: 8, height: 8, pivot: [2, 6] as [number, number] },
    { x: 8, y: 0, width: 8, height: 8, pivot: [2, 6] as [number, number] },
  ],
};
const SHEET_SCHEMA: TypeSchema = { kind: "struct", name: "ludenea_render::sheet::SpriteSheet", fields: [{ name: "texture", schema: { kind: "asset_id" } }] };

/**
 * The editor with every built-in plugin on a project of `files` (or `listed.files` once set),
 * whose `read_binary` answers a path's text as its bytes; the asset browser and the inspector
 * shown, and a game when `clips` gives the sprite sheet's clips (or `value` its whole value).
 */
async function inspecting(options: { clips?: object; value?: unknown } = {}) {
  const listed = { files };
  const host = await testEditor(builtinPlugins, (request) => {
    if (request.type === "list_files") return { type: "files", entries: listed.files };
    if (request.type === "read_binary") return { type: "bytes", bytes: btoa(request.path as string) };
    if (request.type === "read_asset_texts") return { type: "asset_texts", assets: [], paths: [] };
    if (request.type === "load_stored_session") return { type: "stored_session", bytes: null };
    if (request.type === "info") return undefined;
    return { type: "done" };
  });
  if (options.clips || options.value) {
    const value = options.value ?? { texture: HERO, clips: options.clips };
    const game = await testGame((command) => (command.type === "get_asset" ? { type: "asset", asset: "ludenea::SpriteSheet", value: { type: "json", value } } : { type: "done" }), {
      assets: [{ name: "ludenea::SpriteSheet", schema: SHEET_SCHEMA }],
      tick_rate: 60,
    });
    act(() => host.setControl(game.client));
  }
  renderPanel(host, "ludenea.assets");
  renderPanel(host, "ludenea.inspector");
  await settle();
  return { host, listed };
}

async function select(name: string) {
  fireEvent.click(screen.getByText(name));
  await settle();
  await settle();
}

const preview = () => screen.getByRole("img", { name: "assets/hero.png" });

describe("textures", () => {
  beforeEach(() => {
    script.frames = { "assets/hero.png": HERO_FRAMES };
    script.invalidTextures = {};
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    script.frames = {};
    script.invalidTextures = {};
  });

  it("selecting_a_texture_previews_its_frames", async () => {
    const { host } = await inspecting({ clips: {} });
    await select("hero.png");

    expect(host.editor.selection.getState().asset).toEqual({ id: HERO, kind: "Texture" });
    // Zoomed by a whole factor (16) and drawn with nearest filtering.
    const image = preview();
    expect([image.getAttribute("width"), image.getAttribute("height"), image.getAttribute("viewBox")]).toEqual(["256", "128", "0 0 16 8"]);
    expect((image.querySelector("image") as SVGImageElement).style.imageRendering).toBe("pixelated");
    expect(image.querySelector("image")!.getAttribute("href")).toBe(`data:image/png;base64,${btoa("assets/hero.png")}`);
    const outline = (index: number) => {
      const rect = within(image).getByLabelText(`Frame ${index}`).querySelector("rect")!;
      return ["x", "y", "width", "height"].map((name) => rect.getAttribute(name));
    };
    expect([outline(0), outline(1)]).toEqual([
      ["0", "0", "8", "8"],
      ["8", "0", "8", "8"],
    ]);
    expect(within(image).getByLabelText("Frame 1").querySelector("text")!.textContent).toBe("1");
    expect(screen.getByText("16 × 8 pixels, 2 frames, 16 pixels per unit")).toBeTruthy();
    // A texture's inspector stands alone: no generic fields, no resources.
    expect(screen.queryByText("Resources")).toBeNull();
  });

  it("preview_marks_the_pivot", async () => {
    await inspecting();
    await select("hero.png");

    // The pivot in pixels from each frame's top-left corner.
    const pivot = within(preview()).getByLabelText("Pivot of frame 1");
    expect([pivot.getAttribute("cx"), pivot.getAttribute("cy")]).toEqual(["10", "6"]);
    expect(within(preview()).getByLabelText("Pivot of frame 0").getAttribute("cx")).toBe("2");
  });

  it("broken_texture_preview_shows_the_error", async () => {
    script.invalidTextures = { "assets/hero.png": "a grid of 8x8 frames with margin 0 and spacing 0 does not cut the 12x8 image into whole frames" };
    await inspecting();
    await select("hero.png");

    expect(screen.getByRole("alert").textContent).toBe("assets/hero.png: a grid of 8x8 frames with margin 0 and spacing 0 does not cut the 12x8 image into whole frames");
    expect(screen.queryByRole("img", { name: "assets/hero.png" })).toBeNull();
  });

  it("texture_preview_works_without_a_game", async () => {
    await inspecting();
    expect(screen.getByText("No game is running.")).toBeTruthy();
    await select("hero.png");

    expect(preview()).toBeTruthy();
    expect(screen.queryByText("No game is running.")).toBeNull();
  });

  it("a_texture_whose_meta_cannot_be_read_says_so", async () => {
    const { host, listed } = await inspecting();
    listed.files = [{ path: "assets/hero.png", id: "00000000000000000000000000000000", kind: { Texture: { pixels_per_unit: 100, grid: null, pivot: null, filter: "Nearest" } } }];
    act(() => host.editor.selection.getState().selectAsset("00000000000000000000000000000000", "Texture"));
    await settle();

    expect(screen.getByRole("alert").textContent).toBe("The texture's meta cannot be read: the console shows the project's warning, which names the file and the cause.");
  });

  it("the_preview_follows_a_change_of_the_texture_on_disk", async () => {
    const { host } = await inspecting();
    await select("hero.png");
    // Subscribed to the project's textures through `editor.textures`, as any plugin would be.
    expect(host.editor.textures.list()).toEqual([{ id: HERO, path: "assets/hero.png", meta: META }]);

    script.frames = { "assets/hero.png": { ...HERO_FRAMES, frames: HERO_FRAMES.frames.slice(0, 1) } };
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: "assets/hero.png", kind: { Texture: META } } }] }));
    await settle();
    await settle();

    expect(within(preview()).queryByLabelText("Frame 1")).toBeNull();
    expect(screen.getByText("16 × 8 pixels, 1 frame, 16 pixels per unit")).toBeTruthy();
  });

  it("a_preview_lists_the_project_once_reads_its_png_once_and_ignores_other_textures", async () => {
    const OTHER = "0000000000000000000000000000000f";
    const listed = [...files, { path: "assets/other.png", id: OTHER, kind: { Texture: META } }];
    const host = await testEditor([inspectorPlugin, texturesPlugin], (request) => {
      if (request.type === "list_files") return { type: "files", entries: listed };
      if (request.type === "read_binary") return { type: "bytes", bytes: btoa(request.path as string) };
      return undefined;
    });
    const sent = (type: string) => host.project.sent.filter((request) => request.type === type);
    renderPanel(host, "ludenea.inspector");
    act(() => host.editor.selection.getState().selectAsset(HERO, "Texture"));
    await settle();
    await settle();

    expect(preview()).toBeTruthy();
    expect([sent("list_files").length, sent("read_binary")]).toEqual([1, [{ type: "read_binary", path: "assets/hero.png" }]]);

    // Another texture changed: the list follows, this preview stays as it is.
    act(() => host.project.push({ type: "files_changed", changes: [{ Changed: { path: "assets/other.png", kind: { Texture: META } } }] }));
    await settle();
    await settle();
    expect([sent("list_files").length, sent("read_binary").length]).toEqual([2, 1]);
  });

  it("a_preview_shows_why_the_textures_cannot_be_listed", async () => {
    const host = await testEditor([inspectorPlugin, texturesPlugin], (request) => (request.type === "list_files" ? { error: { code: "io", message: "the project folder is gone" } } : undefined));
    renderPanel(host, "ludenea.inspector");
    act(() => host.editor.selection.getState().selectAsset(HERO, "Texture"));
    await settle();

    expect(screen.getByRole("alert").textContent).toBe("The project's textures cannot be listed: the project folder is gone");
  });

  it("clip_player_advances_one_frame_every_ticks_per_frame", async () => {
    // 60 ticks per second and 6 ticks per frame: one frame every 100 milliseconds.
    await inspecting({ clips: { walk: { first: 0, count: 2, ticks_per_frame: 6 } } });
    await select("hero.sheet.ron");
    const clip = within(screen.getByRole("region", { name: "Clip walk" }));
    expect(clip.getByLabelText("Frame").textContent).toBe("0");

    vi.useFakeTimers();
    fireEvent.click(clip.getByRole("button", { name: "Play" }));
    act(() => vi.advanceTimersByTime(99));
    expect(clip.getByLabelText("Frame").textContent).toBe("0");
    act(() => vi.advanceTimersByTime(1));
    expect(clip.getByLabelText("Frame").textContent).toBe("1");
    // A clip loops.
    act(() => vi.advanceTimersByTime(100));
    expect(clip.getByLabelText("Frame").textContent).toBe("0");

    fireEvent.click(clip.getByRole("button", { name: "Pause" }));
    act(() => vi.advanceTimersByTime(1000));
    expect(clip.getByLabelText("Frame").textContent).toBe("0");
    // The frame shown is the texture's, cut out of the image.
    expect(clip.getByRole("img", { name: "Frame 0 of walk" }).getAttribute("viewBox")).toBe("0 0 8 8");
  });

  it("clip_past_the_texture_frames_is_reported", async () => {
    await inspecting({ clips: { run: { first: 1, count: 4, ticks_per_frame: 1 }, idle: { first: 0, count: 1, ticks_per_frame: 1 } } });
    await select("hero.sheet.ron");

    const run = within(screen.getByRole("region", { name: "Clip run" }));
    expect(run.getByRole("alert").textContent).toBe("The clip shows frames 1 to 4, but assets/hero.png has 2 frames (0 to 1).");
    expect(run.queryByRole("button", { name: "Play" })).toBeNull();
    // The other clips still play.
    expect(within(screen.getByRole("region", { name: "Clip idle" })).getByRole("button", { name: "Play" })).toBeTruthy();
  });

  it("a_clip_without_frames_or_ticks_per_frame_does_not_play", async () => {
    await inspecting({ clips: { still: { first: 0, count: 1, ticks_per_frame: 0 } } });
    await select("hero.sheet.ron");

    const still = within(screen.getByRole("region", { name: "Clip still" }));
    expect(still.getByRole("alert").textContent).toBe("The clip needs at least one frame and one tick per frame.");
    expect(still.queryByRole("button", { name: "Play" })).toBeNull();
  });

  it("a_sprite_sheet_without_clips_says_so", async () => {
    await inspecting({ clips: {} });
    await select("hero.sheet.ron");
    expect(screen.getByText("This sprite sheet has no clips.")).toBeTruthy();
  });

  it("a_sprite_sheet_value_without_texture_and_clips_is_reported", async () => {
    await inspecting({ value: { texture: 7 } });
    await select("hero.sheet.ron");
    expect(screen.getByRole("alert").textContent).toBe("This sprite sheet's value has no texture and clips to play.");
  });
});
