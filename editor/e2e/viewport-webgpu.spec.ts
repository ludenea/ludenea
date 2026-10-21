import { expect, test } from "@playwright/test";
import { collectErrors, expectNoErrors, serveProject, viewportDrew, waitForGame, type ServedProject } from "./project";

declare global {
  interface Window {
    /** The GPU device requested last, kept by the init script below. */
    __ludeneaDevice?: { destroy(): void };
  }
}

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("device_game", 4323);
});

test.afterAll(() => served?.stop());

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
  const drew = () => viewportDrew(page, viewport);
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
