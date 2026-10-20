import { existsSync } from "node:fs";
import { join } from "node:path";
import { expect, type Page, test } from "@playwright/test";

// The browser renderer, which the fixture page loads from its `pkg/`. Each test runs in the
// `webgl2` and the `webgpu` projects (playwright.config.ts).
const RENDERER = join(import.meta.dirname, "fixtures/renderer/pkg/ludenea_render.js");

type Rgba = [number, number, number, number];
type Point = [number, number];
interface Drawn {
  report: { missing: unknown[] };
  pixels: Rgba[];
}

declare global {
  interface Window {
    /** Set by `fixtures/renderer/main.js`. */
    fixture: {
      start(): Promise<{ backend: string; errors: { id: string; path: string | null; message: string }[] }>;
      drawReference(points: Point[]): Drawn;
      drawSprites(count: number, placed: Record<number, Point>, points: Point[]): Drawn;
      drawError(): string | null;
      failOnTheDevice(): void;
      loseTheDevice(): void;
    };
  }
}

const BACKGROUND: Rgba = [0x12, 0x12, 0x14, 255];
// The texels of fixtures/renderer/sprites.png and tiles.png. None is 0 or 255 on every channel,
// so a colour encoded to sRGB twice shows.
const LEFT: Rgba = [0xc8, 0x64, 0x32, 255];
const RIGHT: Rgba = [0x32, 0x96, 0xc8, 255];
const FIXED: Rgba = [0x80, 0x30, 0xb0, 255];
const TILE: Rgba = [0x40, 0xa0, 0x60, 255];
const OUTLINE: Rgba = [0x30, 0x60, 0xf0, 255];
const HIGHLIGHT: Rgba = [0xff, 0xd4, 0x00, 255];

/** Opens the fixture page and its renderer, failing on any page error. */
async function start(page: Page): Promise<{ backend: string; errors: { id: string; path: string | null; message: string }[] }> {
  expect(existsSync(RENDERER), `${RENDERER} is missing: run cargo xtask wasm-render editor/e2e/fixtures/renderer/pkg`).toBe(true);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
    console.log(`[page] ${message.text()}`);
  });
  await page.goto("/renderer/index.html");
  await page.waitForFunction(() => window.fixture !== undefined);
  const started = await page.evaluate(() => window.fixture.start());
  expect(errors).toEqual([]);
  return started;
}

test("draws_the_reference_frame", async ({ page }) => {
  const { errors } = await start(page);
  // The broken texture is refused, naming it; the others load.
  expect(errors).toEqual([
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
    ["tile", [40, 104], TILE],
    ["empty cell", [56, 104], BACKGROUND],
    ["sprite on the layer fixed to the screen", [64, 80], FIXED],
    ["where it would be if it moved with the camera", [32, 80], BACKGROUND],
    ["debug outline", [96, 80], OUTLINE],
    ["inside the outline", [96, 96], BACKGROUND],
    ["selected entity", [32, 64], HIGHLIGHT],
  ];
  const { report, pixels } = await page.evaluate((points) => window.fixture.drawReference(points), samples.map(([, point]) => point));
  expect(report).toEqual({ missing: [] });
  for (const [index, [name, , expected]] of samples.entries()) expect(pixels[index], name).toEqual(expected);
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

// The browser reports WebGPU's errors and the device's loss after the call that caused them.
test("a_gpu_error_raised_between_draws_is_returned_by_the_next_draw", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "webgpu", "WebGL2 raises its errors in the call that causes them, as native backends do (tests/gpu.rs)");
  await start(page);
  expect(await page.evaluate(() => window.fixture.drawError())).toBeNull();
  await page.evaluate(() => window.fixture.failOnTheDevice());
  await expect.poll(() => page.evaluate(() => window.fixture.drawError())).toMatch(/^draw failed on the GPU: .*invalid on purpose/s);
  // Reported once.
  expect(await page.evaluate(() => window.fixture.drawError())).toBeNull();
  await page.evaluate(() => window.fixture.loseTheDevice());
  await expect.poll(() => page.evaluate(() => window.fixture.drawError())).toMatch(/^the GPU device was lost: /);
  // Every draw after the loss.
  expect(await page.evaluate(() => window.fixture.drawError())).toMatch(/^the GPU device was lost: /);
});
