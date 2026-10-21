import { expect, test } from "@playwright/test";
import { type Point, type Rgba, start } from "./renderer";

// Each test runs in the `webgl2` and the `webgpu` projects (playwright.config.ts).

const BACKGROUND: Rgba = [0x12, 0x12, 0x14, 255];
// The texels of fixtures/renderer/sprites.png and tiles.png. None is 0 or 255 on every channel,
// so a colour encoded to sRGB twice shows.
const LEFT: Rgba = [0xc8, 0x64, 0x32, 255];
const RIGHT: Rgba = [0x32, 0x96, 0xc8, 255];
const FIXED: Rgba = [0x80, 0x30, 0xb0, 255];
const TILE: Rgba = [0x40, 0xa0, 0x60, 255];
const OUTLINE: Rgba = [0x30, 0x60, 0xf0, 255];
const HIGHLIGHT: Rgba = [0xff, 0xd4, 0x00, 255];

test("draws_the_reference_frame", async ({ page }) => {
  const { refused } = await start(page);
  // The broken texture is refused, naming it; the others load.
  expect(refused).toEqual([
    {
      id: "00000000000000000000000000000003",
      path: "broken.png",
      message: expect.stringContaining("texture 00000000000000000000000000000003 (`broken.png`): cannot decode the PNG"),
    },
  ]);
  const samples: [string, Point, Rgba][] = [
    ["background", [8, 120], BACKGROUND],
    ["sprite, left texel", [27, 32], LEFT],
    ["sprite, right texel", [36, 32], RIGHT],
    ["flipped sprite, left", [91, 32], RIGHT],
    ["flipped sprite, right", [100, 32], LEFT],
    // Halfway from (-1, -3) to (0, -3): the tile at pixels 24 to 40 across, the empty cell
    // at 40 to 56, where the tile is at the current tick.
    ["tile, halfway", [28, 104], TILE],
    ["empty cell, halfway", [44, 104], BACKGROUND],
    ["sprite on the layer fixed to the screen", [64, 80], FIXED],
    ["where it would be if it moved with the camera", [32, 80], BACKGROUND],
    ["debug outline", [96, 80], OUTLINE],
    ["inside the outline", [96, 96], BACKGROUND],
    ["selected entity", [32, 64], HIGHLIGHT],
    // The world layer draws over the layer before it, whatever their orders.
    ["layer over the layer before it", [108, 56], LEFT],
    // Halfway from (0, 3.5) to (1, 3.5): pixels 32 to 48 across, and neither end.
    ["halfway sprite, left texel", [35, 8], LEFT],
    ["halfway sprite, right texel", [44, 8], RIGHT],
    ["where the sprite was", [26, 8], BACKGROUND],
    ["where the sprite is", [54, 8], BACKGROUND],
    // Halfway from 170 to -170 degrees along the shorter arc is 180, which swaps the halves
    // (pixels 64 to 80 across); the longer arc would pass 0 and keep them.
    ["half turn, left half", [66, 32], RIGHT],
    ["half turn, right half", [77, 32], LEFT],
  ];
  const { report, pixels } = await page.evaluate((points) => window.fixture.drawReference(points), samples.map(([, point]) => point));
  expect(report).toEqual({ missing: [] });
  for (const [index, [name, , expected]] of samples.entries()) expect(pixels[index], name).toEqual(expected);

  // Drawn again at alpha 1 without new frames, the sprite is where it moved to (pixels 40 to
  // 56 across) and the tilemap too (its tile at 32 to 48, its empty cell at 48 to 64).
  const moved = await page.evaluate(
    (points) => window.fixture.redraw(1, points),
    [[44, 8], [52, 8], [35, 8], [40, 104], [56, 104], [28, 104]] as Point[],
  );
  expect(moved).toEqual([LEFT, RIGHT, BACKGROUND, TILE, BACKGROUND, BACKGROUND]);
  // The free camera, 1 unit right of the frame's, moves the view without new frames: the
  // sprite 16 pixels left.
  const panned = await page.evaluate(
    (points) => window.fixture.redraw(1, points, { center: { x: 3, y: 0 }, view: { x: 8, y: 8 } }),
    [[28, 8], [36, 8], [52, 8]] as Point[],
  );
  expect(panned).toEqual([LEFT, RIGHT, BACKGROUND]);
});

test("a_frame_after_a_gap_in_ticks_is_drawn_without_blending", async ({ page }) => {
  await start(page);
  // Drawn halfway, but tick 2 does not follow tick 0: the sprite is where it is now (pixels 40
  // to 56 across), not halfway (32 to 48).
  const drawn = await page.evaluate((points) => window.fixture.drawAfterGap(points), [[44, 8], [52, 8], [35, 8]] as Point[]);
  expect(drawn).toEqual([LEFT, RIGHT, BACKGROUND]);
});

test("sixty_thousand_sprites_then_one_both_draw", async ({ page }) => {
  await start(page);
  // Sprite frame 0 is LEFT on its left half: each sprite is sampled there.
  const many = await page.evaluate(
    (points) => window.fixture.drawSprites(60_000, { 0: [0, 2], 30_000: [4, 2], 59_999: [4, -2] }, points),
    [[27, 32], [91, 32], [91, 96], [27, 96]] as Point[],
  );
  expect(many.report).toEqual({ missing: [] });
  expect(many.pixels).toEqual([LEFT, LEFT, LEFT, BACKGROUND]);
  const one = await page.evaluate((points) => window.fixture.drawSprites(1, { 0: [0, -2] }, points), [[27, 96], [27, 32]] as Point[]);
  expect(one.pixels).toEqual([LEFT, BACKGROUND]);
});

test("webgpu_project_has_a_webgpu_adapter", async ({ page }, testInfo) => {
  const { backend } = await start(page);
  const adapter = await page.evaluate(async () => {
    // WebGPU's types are not in TypeScript's DOM library.
    const gpu = (navigator as Navigator & { gpu?: { requestAdapter(): Promise<object | null> } }).gpu;
    return gpu !== undefined && (await gpu.requestAdapter()) !== null;
  });
  if (testInfo.project.name === "webgpu") {
    expect(adapter, "the webgpu project's browser has no WebGPU adapter: see launch arguments in playwright.config.ts").toBe(true);
    expect(backend).toBe("webgpu");
  } else {
    // Without an adapter, the renderer falls back to WebGL2, which this project checks.
    expect(adapter).toBe(false);
    expect(backend).toBe("webgl2");
  }
});

test("a_canvas_past_the_texture_limit_is_refused_and_a_resize_past_it_fails_the_draw", async ({ page }) => {
  const { pageErrors } = await start(page);
  // Past any device's `max_texture_dimension_2d`.
  const wide = 40_000;
  const refused = await page.evaluate((width) => window.fixture.createOn(width, 1), wide);
  expect(refused).toMatch(new RegExp(`^create needs ${wide} for \`max_texture_dimension_2d\`, but at most \\d+ is allowed$`));
  await page.evaluate((width) => window.fixture.resize(width, 1), wide);
  expect(await page.evaluate(() => window.fixture.drawError())).toMatch(
    new RegExp(`^draw needs ${wide} for \`max_texture_dimension_2d\`, but at most \\d+ is allowed$`),
  );
  // Back within the limit, it draws again.
  await page.evaluate(() => window.fixture.resize(128, 128));
  expect(await page.evaluate(() => window.fixture.drawError())).toBeNull();
  expect(pageErrors).toEqual([]);
});
