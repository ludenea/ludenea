import { copyFileSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { button, collectErrors, expectTick, openScene, repository, seek, serveProject, timeline, waitForGame, type ServedProject } from "./project";

declare global {
  interface Window {
    /** Frame durations in milliseconds, recorded by `requestAnimationFrame` while it runs. */
    __frameTimes?: number[];
    /** Each tick the timeline showed, with when it first showed it, sampled every frame. */
    __ticksShown?: { tick: number; at: number }[];
    __stopFrames?: () => void;
    /** How many times the shown contact cache changed while frames were recorded. */
    __cacheChanges?: number;
  }
}

// Written by `cargo run -p boxes --bin editor-bench` (from the repository root).
const BENCH_SCENE = join(resolve(repository, process.env.CARGO_TARGET_DIR ?? "target"), "editor-bench.scene.ron");
const PLAY_MS = 5000;
// Assertions hold on the reference machine (an Apple M-series); elsewhere the numbers are only reported.
const strict = process.env.PERF_STRICT === "1";

let served: ServedProject;

// A project each: a stored session of the bench scene would bring its branches back.
test.beforeEach(async () => {
  expect(existsSync(BENCH_SCENE), `${BENCH_SCENE} is missing: run cargo run -p boxes --bin editor-bench`).toBe(true);
  served = await serveProject("bench_game", 4314, (root) => copyFileSync(BENCH_SCENE, join(root, "scenes/bench.scene.ron")));
});

test.afterEach(() => served?.stop());

const percentile = (values: number[], fraction: number) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(fraction * sorted.length))] ?? Infinity;
};

/**
 * Opens the bench scene, selects a dynamic body in the inspector (as while editing), forks a
 * branch by moving it at a past tick, shows the parent branch as the ghost, then plays for
 * `PLAY_MS` while recording frame times. `beforePlay` runs just before Play.
 */
async function playWithBranchAndGhost(page: Page, beforePlay: () => Promise<void> = async () => {}) {
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

  // That body moved at a past tick forks a branch; the parent branch is the ghost.
  for (let step = 0; step < 3; step++) await button(page, "Step").click();
  await expectTick(page, 3);
  await seek(page, 1);
  await page.getByRole("tab", { name: "Inspector" }).click();
  const x = page.getByRole("region", { name: "ludenea::Pose" }).getByLabel("position.x");
  await x.fill("1.5");
  await x.press("Enter");
  await page.getByRole("tab", { name: "Timeline" }).click();
  const lanes = page.getByRole("list", { name: "Branches" }).getByRole("listitem");
  await expect(lanes).toHaveCount(2);
  await button(page, "Compare with").click();
  const compare = page.getByRole("group", { name: "Compare branch" });
  await compare.getByLabel("Branch").selectOption({ index: 0 });
  await compare.getByRole("button", { name: "Compare" }).click();
  await expect(lanes.nth(0)).toContainText("compare");

  await beforePlay();
  const start = await timeline(page);
  await page.evaluate(() => {
    const times: number[] = [];
    const ticks: { tick: number; at: number }[] = [];
    // Queried each frame: a re-render may replace the label element.
    const shownTick = () => {
      for (const element of document.querySelectorAll("p")) {
        const match = /^Tick (\d+) of/.exec(element.textContent ?? "");
        if (match) return Number(match[1]);
      }
      return undefined;
    };
    // Counts the changes of the contact cache's shown value (its RON text), when the inspector
    // shows it; queried on each mutation: a re-render may replace the element.
    window.__cacheChanges = 0;
    const cache = document.querySelector('section[aria-label="ludenea::ContactCache"]');
    const ron = () => cache?.querySelector('[aria-label="ludenea::ContactCache as RON"]')?.textContent;
    let shown = ron();
    const observer = new MutationObserver(() => {
      if (ron() === shown) return;
      shown = ron();
      window.__cacheChanges!++;
    });
    if (cache) observer.observe(cache, { childList: true, subtree: true, characterData: true });
    let last = performance.now();
    let running = true;
    const frame = (now: number) => {
      if (!running) return;
      times.push(now - last);
      last = now;
      const tick = shownTick();
      if (tick !== undefined && tick !== ticks.at(-1)?.tick) ticks.push({ tick, at: now });
      requestAnimationFrame(frame);
    };
    requestAnimationFrame((now) => {
      last = now;
      requestAnimationFrame(frame);
    });
    window.__frameTimes = times;
    window.__ticksShown = ticks;
    window.__stopFrames = () => {
      running = false;
      observer.disconnect();
    };
  });
  await button(page, "Play").click();
  await page.waitForTimeout(PLAY_MS);
  const { frames, ticks, cacheChanges } = await page.evaluate(() => {
    window.__stopFrames!();
    return { frames: window.__frameTimes!, ticks: window.__ticksShown!, cacheChanges: window.__cacheChanges! };
  });
  await button(page, "Pause").click();
  await expect(button(page, "Play")).toBeVisible();

  const p95 = percentile(frames, 0.95);
  // From the first tick shown after Play to the last one shown while sampling, so neither the
  // click nor the pause counts.
  const first = ticks.find(({ tick }) => tick > start.current);
  const final = ticks.at(-1);
  const rate = first && final && final.at > first.at ? ((final.tick - first.tick) * 1000) / (final.at - first.at) : 0;
  const report = `interactive in ${interactiveMs} ms; ${frames.length} frames, 95th percentile ${p95.toFixed(1)} ms, worst ${Math.max(...frames).toFixed(1)} ms; simulation at ${rate.toFixed(1)} ticks per second`;
  console.log(`performance: ${report}`);
  test.info().annotations.push({ type: "performance", description: report });

  expect(frames.length).toBeGreaterThan(0);
  expect(errors).toEqual([]);
  if (strict) expect(p95, report).toBeLessThanOrEqual(17);
  return { interactiveMs, report, cacheChanges };
}

test("the UI stays at 60 fps with 100,000 entities, a branch, and the ghost", async ({ page }) => {
  const { interactiveMs, report } = await playWithBranchAndGhost(page);
  if (strict) expect(interactiveMs, report).toBeLessThan(2000);
});

test("the UI stays at 60 fps while the inspector follows the contact cache", async ({ page }) => {
  const cache = page.getByRole("region", { name: "ludenea::ContactCache" });
  const { cacheChanges } = await playWithBranchAndGhost(page, async () => {
    await page.getByRole("tab", { name: "Inspector" }).click();
    await page.getByRole("button", { name: "ContactCache", expanded: false }).click();
    await expect(cache.getByLabel("ludenea::ContactCache as RON")).toBeVisible();
  });
  // The shown values change while playing, not only once paused.
  expect(cacheChanges).toBeGreaterThan(1);
});
