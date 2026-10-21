import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { expect, type Page, test } from "@playwright/test";
import { repository } from "./project";

// The swarm game built as a standalone web page, which vite serves under `/standalone/`.
const STANDALONE = join(import.meta.dirname, "fixtures/standalone");
const BUILD = "cargo run --locked -p ludenea-serve -- build examples/swarm --web --out editor/e2e/fixtures/standalone";
// The swarm's own command line tool, built by `cargo build --locked -p swarm`.
const SWARM = join(process.env.CARGO_TARGET_DIR ?? join(repository, "target"), "debug", process.platform === "win32" ? "swarm.exe" : "swarm");
const SCENE = join(repository, "examples/swarm/assets/swarm.scene.ron");
// The body of the bug sprite (examples/swarm/src/assets.rs), drawn untinted and unfiltered.
const BUG_BODY = [0x2e, 0x7d, 0x32];

/** Opens the page with `query`, collecting its errors. */
async function open(page: Page, query: string): Promise<string[]> {
  expect(existsSync(join(STANDALONE, "game.json")), `${STANDALONE} is missing: run ${BUILD}`).toBe(true);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => console.log(`[page] ${message.text()}`));
  await page.goto(`/standalone/index.html${query}`);
  return errors;
}

/** Waits for the page to reach `tick` and returns the state hash it shows there. */
async function reach(page: Page, tick: number): Promise<string> {
  await expect(page.locator("body")).not.toHaveAttribute("data-failed");
  await page.waitForFunction((tick) => Number(document.body.dataset.tick) >= tick || document.body.dataset.failed !== undefined, tick, { timeout: 120_000 });
  await expect(page.locator("body"), await page.locator("#status").innerText()).not.toHaveAttribute("data-failed");
  return (await page.locator("body").getAttribute("data-hash"))!;
}

function swarm(args: string[]): string {
  expect(existsSync(SWARM), `${SWARM} is missing: run cargo build --locked -p swarm`).toBe(true);
  return execFileSync(SWARM, args, { encoding: "utf8" });
}

test("draws_the_swarm", async ({ page }) => {
  const errors = await open(page, "?players=1");
  await reach(page, 30);
  const screenshot = (await page.locator("#game").screenshot()).toString("base64");
  // Decoded in the page: the pixels of the bug's body.
  const bodies = await page.evaluate(
    async ([png, [red, green, blue]]) => {
      const bytes = Uint8Array.from(atob(png as string), (char) => char.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
      const context = new OffscreenCanvas(bitmap.width, bitmap.height).getContext("2d")!;
      context.drawImage(bitmap, 0, 0);
      const pixels = context.getImageData(0, 0, bitmap.width, bitmap.height).data;
      let count = 0;
      for (let at = 0; at < pixels.length; at += 4) {
        if (pixels[at] === red && pixels[at + 1] === green && pixels[at + 2] === blue) count += 1;
      }
      return count;
    },
    [screenshot, BUG_BODY] as const,
  );
  // 10,000 bugs fill the view: far more body pixels than a few stray sprites would draw.
  expect(bodies).toBeGreaterThan(10_000);
  await expect(page.locator("#status")).toHaveText("Playing with 1 player");
  expect(errors).toEqual([]);
});

test("players_query_matches_headless_hashes", async ({ page }) => {
  const errors = await open(page, "?players=1&ticks=300");
  const hash = await reach(page, 300);
  // Play stops exactly at the tick asked.
  await page.waitForTimeout(500);
  await expect(page.locator("body")).toHaveAttribute("data-tick", "300");
  expect(swarm(["run", SCENE, "--ticks", "300", "--hash"]).trim()).toBe(`tick 300 hash ${hash}`);
  expect(errors).toEqual([]);
});

test("lobby_claims_two_keyboard_halves_and_starts", async ({ page }) => {
  const errors = await open(page, "");
  const lobby = page.locator("#lobby");
  await expect(lobby).toBeVisible();
  const slots = page.locator("#slots li");
  await expect(slots).toHaveText(["Free", "Free", "Free", "Free"]);
  await page.keyboard.press("KeyA");
  await page.keyboard.press("KeyL");
  await expect(slots).toHaveText(["Player 1: left keyboard half", "Player 2: right keyboard half", "Free", "Free"]);
  // Escape gives the keyboard's slots back; claimed again, they come back in order.
  await page.keyboard.press("Escape");
  await expect(slots).toHaveText(["Free", "Free", "Free", "Free"]);
  await page.keyboard.press("KeyQ");
  await page.keyboard.press("ArrowLeft");
  await expect(slots).toHaveText(["Player 1: left keyboard half", "Player 2: right keyboard half", "Free", "Free"]);
  await page.keyboard.press("Enter");
  await expect(lobby).toBeHidden();
  await expect(page.locator("#status")).toHaveText("Playing with 2 players");
  await reach(page, 10);
  expect(errors).toEqual([]);
});

test("save_replay_downloads_a_replay_that_verifies", async ({ page }) => {
  const errors = await open(page, "?players=2&ticks=120");
  // Player 1 moves right on D, player 2 up on the arrow, held while the match plays.
  await reach(page, 1);
  await page.keyboard.down("KeyD");
  await page.keyboard.down("ArrowUp");
  await reach(page, 120);
  await page.keyboard.up("KeyD");
  await page.keyboard.up("ArrowUp");
  const downloaded = page.waitForEvent("download");
  await page.getByRole("button", { name: "Save replay" }).click();
  const download = await downloaded;
  expect(download.suggestedFilename()).toBe("swarm.replay");
  const file = join(mkdtempSync(join(tmpdir(), "ludenea-standalone-")), "swarm.replay");
  await download.saveAs(file);
  // Exits with an error, which throws, unless every recorded hash matches.
  expect(swarm(["verify", file])).toContain("121 hashes match");
  expect(errors).toEqual([]);
});

test("file_url_says_to_serve_over_http", async ({ page }) => {
  expect(existsSync(join(STANDALONE, "index.html")), `${STANDALONE} is missing: run ${BUILD}`).toBe(true);
  await page.goto(pathToFileURL(join(STANDALONE, "index.html")).href);
  await expect(page.locator("#status")).toContainText("serve its folder over HTTP");
  await expect(page.locator("body")).toHaveAttribute("data-failed", "true");
});

test("players_out_of_range_is_refused_naming_the_range", async ({ page }) => {
  await open(page, "?players=5");
  await expect(page.locator("#status")).toContainText("a game holds 1 to 4 local players");
  await expect(page.locator("body")).toHaveAttribute("data-failed", "true");
});
