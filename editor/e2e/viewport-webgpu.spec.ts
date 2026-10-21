import { expect, test, type Locator, type Page } from "@playwright/test";
import { collectErrors, expectNoErrors, serveProject, waitForGame, type ServedProject } from "./project";

declare global {
  interface Window {
    /** The GPU device requested last, kept by the init script below. */
    __ludeneaDevice?: { destroy(): void };
  }
}

/** The color the renderer clears to (`BACKGROUND` in crates/ludenea-render/src/gpu/mod.rs). */
const BACKGROUND = [0x12, 0x12, 0x14] as const;

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("device_game", 4323);
});

test.afterAll(() => served?.stop());

/** How many pixels of `viewport` show the background, and how many show something else. */
async function pixels(page: Page, viewport: Locator): Promise<{ background: number; drawn: number }> {
  const screenshot = (await viewport.screenshot()).toString("base64");
  // Decoded in the page.
  return page.evaluate(
    async ([png, [red, green, blue]]) => {
      const bytes = Uint8Array.from(atob(png as string), (char) => char.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
      const context = new OffscreenCanvas(bitmap.width, bitmap.height).getContext("2d")!;
      context.drawImage(bitmap, 0, 0);
      const data = context.getImageData(0, 0, bitmap.width, bitmap.height).data;
      let background = 0;
      for (let at = 0; at < data.length; at += 4) {
        if (data[at] === red && data[at + 1] === green && data[at + 2] === blue) background += 1;
      }
      return { background, drawn: data.length / 4 - background };
    },
    [screenshot, BACKGROUND] as const,
  );
}

// Only in the `webgpu` project (playwright.config.ts): a WebGL2 context is not lost on demand.
test("editor_viewport_recovers_from_a_destroyed_device", async ({ page }) => {
  await page.addInitScript(() => {
    const request = GPUAdapter.prototype.requestDevice;
    GPUAdapter.prototype.requestDevice = async function (this: GPUAdapter, ...args: Parameters<GPUAdapter["requestDevice"]>) {
      const device = await request.apply(this, args);
      window.__ludeneaDevice = device;
      return device;
    };
  });
  const errors = collectErrors(page);
  await page.goto(served.url);
  await waitForGame(page);
  await page.getByRole("tab", { name: "Viewport" }).click();
  const viewport = page.getByRole("application", { name: "Viewport" });
  // The renderer cleared to its background and drew the scene's colliders over it.
  const drew = async () => {
    const { background, drawn } = await pixels(page, viewport);
    return background > drawn && drawn > 0;
  };
  await expect.poll(drew).toBe(true);
  expect(await page.evaluate(() => window.__ludeneaDevice !== undefined)).toBe(true);
  const lost = await viewport.locator("canvas").first().elementHandle();

  await page.evaluate(() => window.__ludeneaDevice!.destroy());
  // The next draw finds the device lost: a small zoom asks for one.
  await viewport.hover();
  await page.mouse.wheel(0, -10);

  // A new renderer on a new canvas draws the scene again.
  await expect.poll(() => lost!.evaluate((canvas) => canvas.isConnected)).toBe(false);
  await expect.poll(drew).toBe(true);
  await expect(viewport.getByRole("alert")).toHaveCount(0);
  await expectNoErrors(page, errors);
  await page.getByRole("tab", { name: "Console", exact: true }).click();
  await expect(page.getByRole("list", { name: "Console" })).toContainText("The GPU device of the viewport was lost");
});
