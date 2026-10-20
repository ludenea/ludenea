import { expect, test } from "@playwright/test";
import { start } from "./renderer";

// Only in the `webgpu` project (playwright.config.ts): WebGL2 raises its errors in the call
// that causes them, as native backends do (crates/ludenea-render/tests/gpu.rs), while the
// browser reports WebGPU's errors and the device's loss after the call that caused them.
test("a_gpu_error_raised_between_draws_is_returned_by_the_next_draw", async ({ page }) => {
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
