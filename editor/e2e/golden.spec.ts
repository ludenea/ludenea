import { existsSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

// The boxes game for the browser, which the golden page loads from `/pkg/`.
const GAME = join(import.meta.dirname, "golden/pkg/game.js");

declare global {
  interface Window {
    /** Runs the boxes scenario in the worker; set by `golden/main.ts`. */
    runGolden(): Promise<{ actual: string[]; expected: string[] }>;
    /** Opens `golden/spawner.replay` in the worker; set by `golden/main.ts`. */
    runReplay(): Promise<{ simulated: number; recorded: number; diverged: number[] }>;
  }
}

test("boxes in a headless browser matches the golden trace", async ({ page }) => {
  expect(existsSync(GAME), `${GAME} is missing: run cargo xtask wasm-game boxes editor/e2e/golden/pkg`).toBe(true);
  page.on("console", (message) => console.log(`[page] ${message.text()}`));
  await page.goto("/");
  const { actual, expected } = await page.evaluate(() => window.runGolden());
  expect(expected).toHaveLength(10_001);
  expect(actual).toHaveLength(10_001);
  const mismatch = actual.findIndex((hash, tick) => hash !== expected[tick]);
  expect(mismatch, `first diverging tick: ${mismatch}`).toBe(-1);
});

// The replay holds a prefab, a data asset, and past edits: the browser build replays all of them.
test("spawner_replay_matches_in_browser", async ({ page }) => {
  expect(existsSync(GAME), `${GAME} is missing: run cargo xtask wasm-game boxes editor/e2e/golden/pkg`).toBe(true);
  page.on("console", (message) => console.log(`[page] ${message.text()}`));
  await page.goto("/");
  const { simulated, recorded, diverged } = await page.evaluate(() => window.runReplay());
  // Every recorded hash, from tick 0 to tick 3,000, compared by the game as it resimulates.
  expect(recorded).toBe(3001);
  expect(simulated).toBe(3000);
  expect(diverged, `first diverging tick: ${diverged[0]}`).toEqual([]);
});
