import { copyFileSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { devices, expect, test, type Page } from "@playwright/test";
import { button, collectErrors, expectNoErrors, expectTick, launchArgs, openScene, repository, runCommand, seek, serveProject, serveSwarm, timeline, waitForGame, type ServedProject } from "./project";

declare global {
  interface Window {
    /** Frame durations in milliseconds, recorded by `requestAnimationFrame` while it runs. */
    __frameTimes?: number[];
    /** Each tick the timeline showed, with when it first showed it, sampled every frame. */
    __ticksShown?: { tick: number; at: number }[];
    __stopFrames?: () => void;
    /** How many times the shown contact cache changed while frames were recorded. */
    __cacheChanges?: number;
    /** The render frames the game's worker posted as bytes, and the frame fetches posted to it while `playing`. */
    __renderFrames?: { frames: number; bytes: number; fetches: number; playing: boolean };
  }
}

// Written by `cargo run -p boxes --bin editor-bench` (from the repository root).
const BENCH_SCENE = join(resolve(repository, process.env.CARGO_TARGET_DIR ?? "target"), "editor-bench.scene.ron");
const PLAY_MS = 5000;
// Assertions hold on the reference machine (an Apple M-series); elsewhere the numbers are only reported.
const strict = process.env.PERF_STRICT === "1";

// The configuration's launch arguments, with scrollbars shown as users see them (headless
// Chromium hides them), so a test can give the page scrollbars that take room.
test.use({ launchOptions: { args: launchArgs, ignoreDefaultArgs: ["--hide-scrollbars"] } });

let served: ServedProject | undefined;

// A project each: a stored session of the bench scene would bring its branches back.
async function serveBench(): Promise<ServedProject> {
  expect(existsSync(BENCH_SCENE), `${BENCH_SCENE} is missing: run cargo run -p boxes --bin editor-bench`).toBe(true);
  served = await serveProject("bench_game", 4314, (root) => copyFileSync(BENCH_SCENE, join(root, "scenes/bench.scene.ron")));
  return served;
}

test.afterEach(() => {
  served?.stop();
  served = undefined;
});

const percentile = (values: number[], fraction: number) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(fraction * sorted.length))] ?? Infinity;
};

/**
 * Starts recording the duration of each frame, each tick the timeline shows with when it first
 * showed it, and how many times the shown contact cache changes. The returned function stops
 * the recording and returns what it recorded.
 */
async function recordFrames(page: Page) {
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
  return () =>
    page.evaluate(() => {
      window.__stopFrames!();
      return { frames: window.__frameTimes!, ticks: window.__ticksShown!, cacheChanges: window.__cacheChanges! };
    });
}

/**
 * Logs and annotates the report `describe` makes of the frames' statistics, checks that frames
 * were recorded and, on the reference machine, that their 95th percentile is within `budget`
 * milliseconds; returns the report.
 */
function reportFrames(frames: number[], describe: (stats: string) => string, budget = 17): string {
  const p95 = percentile(frames, 0.95);
  const report = describe(`${frames.length} frames, 95th percentile ${p95.toFixed(1)} ms, worst ${Math.max(...frames).toFixed(1)} ms`);
  console.log(`performance: ${report}`);
  test.info().annotations.push({ type: "performance", description: report });
  expect(frames.length).toBeGreaterThan(0);
  if (strict) expect(p95, report).toBeLessThanOrEqual(budget);
  return report;
}

/**
 * Opens the bench scene, selects a dynamic body in the inspector (as while editing), forks a
 * branch by moving it at a past tick, shows the parent branch as the ghost, then plays for
 * `PLAY_MS` while recording frame times. `beforePlay` runs just before Play.
 */
async function playWithBranchAndGhost(page: Page, beforePlay: () => Promise<void> = async () => {}) {
  const { url } = await serveBench();
  const errors = collectErrors(page);
  const opened = Date.now();
  await page.goto(url);
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
  const stop = await recordFrames(page);
  await button(page, "Play").click();
  await page.waitForTimeout(PLAY_MS);
  const { frames, ticks, cacheChanges } = await stop();
  await button(page, "Pause").click();
  await expect(button(page, "Play")).toBeVisible();

  // From the first tick shown after Play to the last one shown while sampling, so neither the
  // click nor the pause counts.
  const first = ticks.find(({ tick }) => tick > start.current);
  const final = ticks.at(-1);
  const rate = first && final && final.at > first.at ? ((final.tick - first.tick) * 1000) / (final.at - first.at) : 0;
  const report = reportFrames(frames, (stats) => `interactive in ${interactiveMs} ms; ${stats}; simulation at ${rate.toFixed(1)} ticks per second`);
  await expectNoErrors(page, errors);
  return { interactiveMs, report, cacheChanges };
}

test("the UI stays at 60 fps with 100,000 entities, a branch, and the ghost", async ({ page }) => {
  const { interactiveMs, report } = await playWithBranchAndGhost(page);
  if (strict) expect(interactiveMs, report).toBeLessThan(2000);
});

test("the_explorer_lists_100000_entities_with_only_the_visible_rows_while_playing", async ({ page }) => {
  const { url } = await serveBench();
  const errors = collectErrors(page);
  await page.goto(url);
  await openScene(page, "bench.scene.ron");
  await waitForGame(page);
  await page.getByRole("tab", { name: "Explorer" }).click();
  await page.getByRole("tablist", { name: "Explorer" }).getByRole("tab", { name: "Entities" }).click();
  const list = page.getByRole("listbox", { name: "Matching entities" });
  // The bench scene, not the template's four entities.
  await expect(list.getByRole("option").nth(20)).toBeVisible({ timeout: 60_000 });

  // Only the rows the panel shows are in the page, and the list fits in the panel: it scrolls.
  const layout = () =>
    page.evaluate(() => {
      const panel = document.querySelector(".ludenea-explorer")!;
      const scroller = panel.querySelector(".ludenea-entity-list")!;
      const listbox = panel.querySelector('[role="listbox"]')!;
      const option = listbox.querySelector('[role="option"]')!;
      return {
        options: listbox.querySelectorAll('[role="option"]').length,
        scrollerHeight: scroller.clientHeight,
        viewportHeight: window.innerHeight,
        listboxWidth: listbox.getBoundingClientRect().width,
        panelWidth: panel.getBoundingClientRect().width,
        optionHeight: option.getBoundingClientRect().height,
        contentHeight: option.firstElementChild!.getBoundingClientRect().height,
        contentCut: option.firstElementChild!.scrollWidth > option.firstElementChild!.clientWidth,
      };
    });
  const shown = await layout();
  expect(shown.options).toBeGreaterThan(0);
  expect(shown.options).toBeLessThan(100);
  expect(shown.scrollerHeight).toBeLessThan(shown.viewportHeight);
  expect(shown.listboxWidth).toBeGreaterThan(shown.panelWidth / 2);
  // A row's entity and hash stay on its one line, cut short in a narrow panel.
  expect(shown.contentHeight).toBeLessThanOrEqual(shown.optionHeight);
  const size = page.viewportSize()!;
  await page.setViewportSize({ width: 230, height: size.height });
  await expect.poll(async () => (await layout()).panelWidth).toBeLessThanOrEqual(230);
  const narrow = await layout();
  expect(narrow.contentHeight).toBeLessThanOrEqual(narrow.optionHeight);
  expect(narrow.contentCut).toBe(true);
  await page.setViewportSize(size);
  await expect.poll(async () => (await layout()).panelWidth).toBeGreaterThan(230);

  // Each column title sits over its cells, also beside a scrollbar that takes room, as on
  // Windows, Linux, or macOS with a mouse.
  const scrollbars = await page.addStyleTag({ content: "::-webkit-scrollbar { width: 15px; height: 15px; }" });
  const columns = () =>
    page.evaluate(() => {
      const panel = document.querySelector(".ludenea-explorer")!;
      const scroller = panel.querySelector<HTMLElement>(".ludenea-entity-list")!;
      const edges = (element: Element) => element.getBoundingClientRect();
      const headers = [...panel.querySelectorAll('[role="columnheader"]')].map(edges);
      const cells = [...panel.querySelector('[role="option"]')!.children].map(edges);
      return {
        scrollbar: scroller.offsetWidth - scroller.clientWidth,
        columns: headers.length,
        offsets: headers.map((header, index) => [Math.round(header.left - cells[index]!.left), Math.round(header.right - cells[index]!.right)]),
      };
    });
  await expect.poll(columns).toEqual({ scrollbar: 15, columns: 2, offsets: [[0, 0], [0, 0]] });
  await scrollbars.evaluate((style) => style.parentNode?.removeChild(style));

  // The timeline shares the bottom dock with the explorer: played from the command palette,
  // which has closed when recording starts, so its frames are not counted.
  await runCommand(page, "Play or pause");
  const stop = await recordFrames(page);
  await page.waitForTimeout(PLAY_MS);
  const { frames } = await stop();
  const playing = await layout();
  await runCommand(page, "Play or pause");
  await page.getByRole("tab", { name: "Timeline" }).click();
  await expect(button(page, "Play")).toBeVisible();
  expect((await timeline(page)).current).toBeGreaterThan(0);

  expect(playing.options).toBeLessThan(100);
  reportFrames(frames, (stats) => `explorer while playing: ${stats}`);
  await expectNoErrors(page, errors);
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

// On the reference machine the swarm draws with the machine's GPU, as the editor does for its
// users; elsewhere with SwiftShader, without a GPU, which only reports the numbers: drawing
// 10,000 sprites in software is far over the budget.
test("playing_the_swarm_keeps_p95_under_20_ms", async ({ page: configured, playwright }) => {
  const gpu = strict ? await playwright.chromium.launch({ args: ["--enable-gpu"] }) : null;
  try {
    await playTheSwarm(gpu ? await gpu.newPage(devices["Desktop Chrome"]) : configured);
  } finally {
    await gpu?.close();
  }
});

/** Plays the swarm in the editor, viewed through its game camera, while recording frame times. */
async function playTheSwarm(page: Page) {
  const { url } = (served = await serveSwarm(4322));
  // Counts what crosses between the page and the game's worker: the frames it posts as bytes
  // (transferred buffers), and any frame the page fetches while the swarm plays.
  await page.addInitScript(() => {
    const counts = { frames: 0, bytes: 0, fetches: 0, playing: false };
    window.__renderFrames = counts;
    const post = Worker.prototype.postMessage as (this: Worker, ...args: unknown[]) => void;
    Worker.prototype.postMessage = function (this: Worker, ...args: unknown[]) {
      if (counts.playing && typeof args[0] === "string" && args[0].includes('"get_render_frame"')) counts.fetches++;
      post.apply(this, args);
    };
    window.Worker = class extends Worker {
      constructor(...args: ConstructorParameters<typeof Worker>) {
        super(...args);
        this.addEventListener("message", ({ data }: MessageEvent<{ type?: string; frames?: unknown[] }>) => {
          if (data?.type !== "render_frames") return;
          for (const frame of data.frames ?? []) {
            if (!(frame instanceof ArrayBuffer)) continue;
            counts.frames++;
            counts.bytes += frame.byteLength;
          }
        });
      }
    };
  });
  const errors = collectErrors(page);
  await page.goto(url);
  // The swarm's scene opens as the project's default one.
  await waitForGame(page);
  await page.getByRole("tab", { name: "Scene" }).click();
  await expect(page.getByRole("listbox", { name: "Entities" }).getByRole("option").nth(20)).toBeVisible({ timeout: 60_000 });
  await page.getByRole("tab", { name: "Viewport" }).click();
  await page.getByRole("tab", { name: "Timeline" }).click();
  // The whole swarm, as its camera sees it.
  await runCommand(page, "View through the game camera");
  const viewport = page.getByRole("application", { name: "Viewport" });
  const paused = await viewport.screenshot();

  await page.evaluate(() => (window.__renderFrames!.playing = true));
  const stop = await recordFrames(page);
  await button(page, "Play").click();
  await page.waitForTimeout(PLAY_MS);
  const { frames } = await stop();
  // Still playing, after the measure: what the viewport shows moves.
  const [playing, later] = [await viewport.screenshot(), await page.waitForTimeout(250).then(() => viewport.screenshot())];
  const pushed = await page.evaluate(() => {
    window.__renderFrames!.playing = false;
    return { ...window.__renderFrames! };
  });
  await button(page, "Pause").click();
  await expect(button(page, "Play")).toBeVisible();

  const report = reportFrames(
    frames,
    (stats) => `swarm: ${stats}; ${pushed.frames} frames pushed as bytes, ${Math.round(pushed.bytes / Math.max(pushed.frames, 1))} bytes each`,
    20,
  );
  // The frames come from the host as bytes, never fetched while playing, and are drawn.
  expect(pushed.frames, report).toBeGreaterThan(PLAY_MS / 100);
  expect(pushed.fetches, report).toBe(0);
  expect(playing.equals(paused), "the viewport shows the swarm moving").toBe(false);
  expect(later.equals(playing), "the viewport shows the swarm moving").toBe(false);
  await expectNoErrors(page, errors);
}
