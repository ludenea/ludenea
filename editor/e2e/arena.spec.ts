import { execFileSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { button, collectErrors, expectNoErrors, play, repository, runCommand, seek, serveArena, timeline, waitForGame, type ServedProject } from "./project";

// The arena's own command line tool, built by `cargo build --locked -p arena`: the headless
// reference the editor's runs are checked against.
const ARENA = join(process.env.CARGO_TARGET_DIR ?? join(repository, "target"), "debug", process.platform === "win32" ? "arena.exe" : "arena");
const SCENE = join(repository, "examples/arena/assets/arena.scene.ron");

let served: ServedProject;

// A project per test: a page opened again on a project another page already played does not
// load its scene into the game.
test.beforeEach(async () => {
  served = await serveArena(4326);
});

test.afterEach(() => served?.stop());

function arena(args: string[]): string {
  expect(existsSync(ARENA), `${ARENA} is missing: run cargo build --locked -p arena`).toBe(true);
  return execFileSync(ARENA, args, { encoding: "utf8" });
}

/** The hash the timeline shows at `tick`, once no edit waits to be simulated. */
async function hashAt(page: Page, tick: number): Promise<string> {
  await seek(page, tick);
  await expect(page.getByLabel("Hash")).not.toContainText("pending");
  return (await page.getByLabel("Hash").textContent())!;
}

/** How many pixels differ between the screenshots `a` and `b` where `a` is not the renderer's background; decoded in the page. */
async function differingPixels(page: Page, a: Buffer, b: Buffer): Promise<number> {
  return page.evaluate(
    async ([a, b]) => {
      const pixels = async (png: string) => {
        const bytes = Uint8Array.from(atob(png), (char) => char.charCodeAt(0));
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
        const context = new OffscreenCanvas(bitmap.width, bitmap.height).getContext("2d")!;
        context.drawImage(bitmap, 0, 0);
        return context.getImageData(0, 0, bitmap.width, bitmap.height).data;
      };
      const [first, second] = [await pixels(a), await pixels(b)];
      // `BACKGROUND` in crates/ludenea-render/src/gpu/mod.rs.
      const background = [0x12, 0x12, 0x14];
      let count = 0;
      for (let at = 0; at < first.length; at += 4) {
        const differs = [0, 1, 2].some((channel) => first[at + channel] !== second[at + channel]);
        const drawn = [0, 1, 2].some((channel) => first[at + channel] !== background[channel]);
        if (differs && drawn) count += 1;
      }
      return count;
    },
    [a.toString("base64"), b.toString("base64")] as const,
  );
}

/**
 * Waits until the game holds the arena's scene, which opens as the project's default one: the
 * game starts on its built world, without entities, and the scene loads after.
 */
async function waitForArena(page: Page): Promise<void> {
  await waitForGame(page);
  const tilemap = page.getByRole("listbox", { name: "Entities" }).getByRole("option").filter({ hasText: "Tilemap" });
  await expect(async () => {
    await page.getByRole("tab", { name: "Scene" }).click();
    await expect(tilemap).toHaveCount(1, { timeout: 2000 });
  }).toPass({ timeout: 60_000 });
  await page.getByRole("tab", { name: "Timeline" }).click();
}

test("editor_play_matches_headless_hashes", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto(served.url);
  await waitForArena(page);
  await runCommand(page, "Set the player count to 1");
  // Nothing has the focus that captures the keys: 300 ticks of no input.
  await play(page, 120_000, 300);
  const hash = await hashAt(page, 300);
  expect(arena(["run", SCENE, "--ticks", "300", "--hash"]).trim()).toBe(`tick 300 hash ${hash}`);
  await expectNoErrors(page, errors);
});

test("editor_two_player_replay_verifies_headless", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto(served.url);
  await waitForArena(page);
  await runCommand(page, "Set the player count to 2");
  await page.getByRole("tab", { name: "Viewport" }).click();
  const viewport = page.getByRole("application", { name: "Viewport" });
  // The viewport holds the focus, so the keys reach the game: F5 plays, then player 1 runs
  // right on D (the left half) and player 2 left on the left arrow (the right half).
  await viewport.focus();
  await page.keyboard.press("F5");
  await expect(button(page, "Pause")).toBeVisible();
  await page.keyboard.down("KeyD");
  await page.keyboard.down("ArrowLeft");
  await expect.poll(async () => (await timeline(page)).current, { timeout: 60_000 }).toBeGreaterThanOrEqual(90);
  await page.keyboard.down("KeyW");
  await expect.poll(async () => (await timeline(page)).current, { timeout: 60_000 }).toBeGreaterThanOrEqual(150);
  for (const key of ["KeyW", "ArrowLeft", "KeyD"]) await page.keyboard.up(key);
  await button(page, "Pause").click();
  await expect(button(page, "Play")).toBeVisible();

  await page.getByRole("tab", { name: "Timeline" }).click();
  await button(page, "Save as replay").click();
  const folder = join(served.root, "assets/replays");
  await expect.poll(() => (existsSync(folder) ? readdirSync(folder).filter((file) => file.endsWith(".replay")) : [])).toHaveLength(1);
  const replay = readdirSync(folder).find((file) => file.endsWith(".replay"))!;
  // Exits with an error, which throws, unless every recorded hash matches.
  expect(arena(["verify", join(folder, replay)])).toContain("hashes match");
  // The keys reached the game: player 1's fighter, the first spawned, ran right of its spawn
  // point (x -6) by the 90 ticks of D at least.
  await page.getByRole("tab", { name: "Explorer" }).click();
  await page.getByRole("tablist", { name: "Explorer" }).getByRole("tab", { name: "Entities" }).click();
  await page.getByLabel("Entity query").fill("With<arena::Fighter>");
  await page.getByLabel("Entity query").press("Enter");
  const fighters = page.getByRole("listbox", { name: "Matching entities" }).getByRole("option");
  await expect(fighters).toHaveCount(2);
  await fighters.first().click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  const x = page.getByRole("region", { name: "ludenea::Pose" }).getByLabel("position.x");
  await expect.poll(async () => Number(await x.inputValue())).toBeGreaterThan(-6 + 5);
  await expectNoErrors(page, errors);
});

test("editor_draws_the_painted_level", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto(served.url);
  await waitForArena(page);
  await page.getByRole("tab", { name: "Viewport" }).click();
  await runCommand(page, "View through the game camera");
  const viewport = page.getByRole("application", { name: "Viewport" });
  const level = await viewport.screenshot();

  // Without the tilemap, only the painted tiles disappear from the view.
  await page.getByRole("tab", { name: "Scene" }).click();
  await page.getByRole("listbox", { name: "Entities" }).getByRole("option").filter({ hasText: "Tilemap" }).click();
  await runCommand(page, "Despawn selected entities");
  await page.getByRole("tab", { name: "Viewport" }).click();
  // The ground spans the width of the view and a quarter of its height.
  const { width, height } = (await viewport.boundingBox())!;
  await expect.poll(async () => differingPixels(page, level, await viewport.screenshot())).toBeGreaterThan((width * height) / 8);
  await expectNoErrors(page, errors);
});
