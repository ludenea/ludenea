import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { collectErrors, expectNoErrors, serveProject, viewportDrew, waitForGame, type ServedProject } from "./project";

declare global {
  interface Window {
    /** Each instance of the renderer's module the page loaded, oldest first, by its test hook. */
    __ludeneaRenderModules?: { panicForTesting(message: string): void }[];
  }
}

// The renderer built with its test hooks, which exports `panicForTesting`.
const PACKAGE = join(import.meta.dirname, "fixtures/renderer/pkg");

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("panic_game", 4324);
});

test.afterAll(() => served?.stop());

test("editor_viewport_draws_again_after_a_panic_and_a_retry", async ({ page }) => {
  expect(existsSync(join(PACKAGE, "ludenea_render.js")), `${PACKAGE} is missing: run cargo xtask wasm-render --test-hooks editor/e2e/fixtures/renderer/pkg`).toBe(true);
  // The editor's renderer module, served as its test build: each instance the page loads
  // records itself, so the test can make it panic.
  const glue = `${readFileSync(join(PACKAGE, "ludenea_render.js"), "utf8")}\n(window.__ludeneaRenderModules ??= []).push({ panicForTesting });\n`;
  await page.route(/\/assets\/ludenea_render-[^/]*\.js(\?.*)?$/, (route) => route.fulfill({ contentType: "text/javascript", body: glue }));
  await page.route(/\/assets\/ludenea_render_bg-[^/]*\.wasm$/, (route) => route.fulfill({ contentType: "application/wasm", body: readFileSync(join(PACKAGE, "ludenea_render_bg.wasm")) }));
  const errors = collectErrors(page);
  await page.goto(served.url);
  await waitForGame(page);
  await page.getByRole("tab", { name: "Viewport" }).click();
  const viewport = page.getByRole("application", { name: "Viewport" });
  await expect.poll(() => viewportDrew(page, viewport)).toBe(true);
  expect(await page.evaluate(() => window.__ludeneaRenderModules?.length)).toBe(1);

  // A panic aborts the module: the call that panicked traps.
  const trapped = await page.evaluate(() => {
    try {
      window.__ludeneaRenderModules![0]!.panicForTesting("the end-to-end test panics on purpose");
      return null;
    } catch (error) {
      return String(error);
    }
  });
  expect(trapped).toMatch(/unreachable/);
  const alert = viewport.getByRole("alert");
  await expect(alert).toContainText("The viewport stopped rendering: panicked at");
  await expect(alert).toContainText("the end-to-end test panics on purpose");
  const aborted = await viewport.locator("canvas").first().elementHandle();

  // Retried, the viewport draws again with a new instance of the module.
  await alert.getByRole("button", { name: "Retry" }).click();
  await expect(alert).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => window.__ludeneaRenderModules?.length)).toBe(2);
  await expect.poll(() => aborted!.evaluate((canvas) => canvas.isConnected)).toBe(false);
  await expect.poll(() => viewportDrew(page, viewport)).toBe(true);
  await expectNoErrors(page, errors);
});
