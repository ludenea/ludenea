import { copyFileSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { expect, test } from "@playwright/test";
import { collectErrors, openScene, repository, serveProject, timeline, waitForGame, type ServedProject } from "./project";

declare global {
  interface Window {
    /** Frame durations in milliseconds, recorded by `requestAnimationFrame` while it runs. */
    __frameTimes?: number[];
    __stopFrames?: () => void;
  }
}

// Written by `cargo run -p boxes --bin editor-bench` (from the repository root).
const BENCH_SCENE = join(resolve(repository, process.env.CARGO_TARGET_DIR ?? "target"), "editor-bench.scene.ron");
const PLAY_MS = 5000;
// Assertions hold on the reference machine (an Apple M-series); elsewhere the numbers are only reported.
const strict = process.env.PERF_STRICT === "1";

let served: ServedProject;

test.beforeAll(async () => {
  expect(existsSync(BENCH_SCENE), `${BENCH_SCENE} is missing: run cargo run -p boxes --bin editor-bench`).toBe(true);
  served = await serveProject("bench_game", 4314, (root) => copyFileSync(BENCH_SCENE, join(root, "scenes/bench.scene.ron")));
});

test.afterAll(() => served?.stop());

const percentile = (values: number[], fraction: number) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(fraction * sorted.length))] ?? Infinity;
};

test("the UI stays at 60 fps with 100,000 entities", async ({ page }) => {
  const errors = collectErrors(page);
  const opened = Date.now();
  await page.goto(served.url);
  await page.getByRole("tab", { name: "Assets" }).click();
  await expect(page.getByRole("tree", { name: "Assets" }).getByRole("treeitem", { name: /bench\.scene\.ron/ })).toBeVisible();
  const interactiveMs = Date.now() - opened;

  await openScene(page, "bench.scene.ron");
  await waitForGame(page);
  await page.getByRole("tab", { name: "Scene" }).click();
  // The scene tree lists the bench scene, not the template's four entities.
  await expect(page.getByRole("listbox", { name: "Entities" }).getByRole("option").nth(20)).toBeVisible({ timeout: 60_000 });
  // A dynamic body in the inspector, as while editing.
  await page.getByRole("listbox", { name: "Entities" }).getByRole("option").nth(10).click();
  await page.getByRole("tab", { name: "Viewport" }).click();
  await page.getByRole("tab", { name: "Timeline" }).click();

  const start = await timeline(page);
  await page.evaluate(() => {
    const times: number[] = [];
    let last = performance.now();
    let running = true;
    const frame = (now: number) => {
      if (!running) return;
      times.push(now - last);
      last = now;
      requestAnimationFrame(frame);
    };
    requestAnimationFrame((now) => {
      last = now;
      requestAnimationFrame(frame);
    });
    window.__frameTimes = times;
    window.__stopFrames = () => {
      running = false;
    };
  });
  const played = Date.now();
  await page.getByRole("button", { name: "Play", exact: true }).click();
  await page.waitForTimeout(PLAY_MS);
  const frames = await page.evaluate(() => {
    window.__stopFrames!();
    return window.__frameTimes!;
  });
  const playedSeconds = (Date.now() - played) / 1000;
  await page.getByRole("button", { name: "Pause", exact: true }).click();
  await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();
  const end = await timeline(page);

  const p95 = percentile(frames, 0.95);
  const rate = (end.current - start.current) / playedSeconds;
  const report = `interactive in ${interactiveMs} ms; ${frames.length} frames, 95th percentile ${p95.toFixed(1)} ms, worst ${Math.max(...frames).toFixed(1)} ms; simulation at ${rate.toFixed(1)} ticks per second`;
  console.log(`performance: ${report}`);
  test.info().annotations.push({ type: "performance", description: report });

  expect(frames.length).toBeGreaterThan(0);
  expect(errors).toEqual([]);
  if (strict) {
    expect(p95, report).toBeLessThanOrEqual(17);
    expect(interactiveMs, report).toBeLessThan(2000);
  }
});
