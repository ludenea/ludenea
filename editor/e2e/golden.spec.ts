import { expect, test } from "@playwright/test";

declare global {
  interface Window {
    /** Runs the boxes scenario in the worker; set by `golden/main.ts`. */
    runGolden(): Promise<{ actual: string[]; expected: string[] }>;
  }
}

test("boxes in a headless browser matches the golden trace", async ({ page }) => {
  page.on("console", (message) => console.log(`[page] ${message.text()}`));
  await page.goto("/");
  const { actual, expected } = await page.evaluate(() => window.runGolden());
  expect(expected).toHaveLength(10_001);
  expect(actual).toHaveLength(10_001);
  const mismatch = actual.findIndex((hash, tick) => hash !== expected[tick]);
  expect(mismatch, `first diverging tick: ${mismatch}`).toBe(-1);
});
