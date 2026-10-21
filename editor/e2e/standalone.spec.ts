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
  // Reported, not asserted: how often the page draws while the swarm plays.
  const draws = async () => Number(await page.locator("body").getAttribute("data-draws"));
  const [before, from] = [await draws(), Date.now()];
  await page.waitForTimeout(3000);
  const rate = ((await draws()) - before) / ((Date.now() - from) / 1000);
  console.log(`standalone: the page draws ${rate.toFixed(1)} frames per second`);
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
  // A modifier is a key like any other: Control claims the left half.
  await page.keyboard.press("ControlLeft");
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

/** A message the page posted to the worker. */
interface Posted {
  type: string;
  devices?: { keys: string[]; gamepads: unknown[] };
}

/** The pad `withPad` puts in slot 0, as the page sends it: button 0 held. */
const PAD = { buttons: [1], axes: [0, 0] };

/**
 * Records every message the page posts to the worker, from the next navigation on; with
 * `withPad`, the browser reports one gamepad, in slot 0, holding its first button.
 */
async function recordPosts(page: Page, { withPad = false } = {}): Promise<void> {
  if (withPad) {
    await page.addInitScript(() => {
      const pad = { connected: true, index: 0, buttons: [{ value: 1, pressed: true }], axes: [0, 0] };
      Object.defineProperty(navigator, "getGamepads", { configurable: true, value: () => [pad, null, null, null] });
    });
  }
  await page.addInitScript(() => {
    const posted: unknown[] = [];
    (window as unknown as { posted: unknown[] }).posted = posted;
    const post = Worker.prototype.postMessage;
    Worker.prototype.postMessage = function (this: Worker, message: unknown, ...rest: never[]) {
      posted.push(message);
      return post.call(this, message, ...rest);
    };
  });
}

/** The keys of the last devices the page sent. */
function lastKeys(page: Page): Promise<string[] | undefined> {
  return page.evaluate(() => (window as unknown as { posted: Posted[] }).posted.filter((message) => message.type === "devices").at(-1)?.devices?.keys);
}

const RELEASED = { type: "devices", devices: { keys: [], gamepads: [] } };

/** Dispatches a cancelable key event to the page; false when the page kept it from the browser. */
function dispatchKey(page: Page, type: "keydown" | "keyup", code: string, modifiers: { ctrlKey?: boolean; metaKey?: boolean } = {}): Promise<boolean> {
  return page.evaluate(([type, code, modifiers]) => dispatchEvent(new KeyboardEvent(type, { code, cancelable: true, ...modifiers })), [type, code, modifiers] as const);
}

test("losing_focus_releases_the_held_keys_at_once", async ({ page }) => {
  await recordPosts(page, { withPad: true });
  const errors = await open(page, "?players=1");
  await reach(page, 1);
  await page.keyboard.down("KeyD");
  await expect.poll(() => lastKeys(page)).toEqual(["KeyD"]);
  // Sent in the blur itself, not on a later animation frame, which a hidden tab never runs.
  // A blurred page still reads the gamepads: the pad stays held.
  const afterBlur = await page.evaluate(() => {
    dispatchEvent(new Event("blur"));
    return (window as unknown as { posted: Posted[] }).posted.at(-1);
  });
  expect(afterBlur).toEqual({ type: "devices", devices: { keys: [], gamepads: [PAD] } });
  // A hidden tab cannot read them: the keys and the pad are released.
  await page.keyboard.up("KeyD");
  await page.keyboard.down("KeyA");
  await expect.poll(() => lastKeys(page)).toEqual(["KeyA"]);
  const afterHiding = await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
    return (window as unknown as { posted: Posted[] }).posted.at(-1);
  });
  expect(afterHiding).toEqual(RELEASED);
  await page.keyboard.up("KeyA");
  expect(errors).toEqual([]);
});

test("control_with_a_game_key_reaches_the_game", async ({ page }) => {
  await recordPosts(page);
  const errors = await open(page, "?players=1");
  await reach(page, 1);
  // Kept from the browser (Control with D bookmarks the page) and held for the game; Control
  // with R still reloads.
  expect(await dispatchKey(page, "keydown", "ControlLeft", { ctrlKey: true })).toBe(false);
  expect(await dispatchKey(page, "keydown", "KeyD", { ctrlKey: true })).toBe(false);
  await expect.poll(() => lastKeys(page)).toEqual(["ControlLeft", "KeyD"]);
  expect(await dispatchKey(page, "keydown", "KeyR", { ctrlKey: true })).toBe(true);
  for (const code of ["KeyR", "KeyD", "ControlLeft"]) await dispatchKey(page, "keyup", code);
  await expect.poll(() => lastKeys(page)).toEqual([]);
  expect(errors).toEqual([]);
});

test("releasing_meta_releases_the_keys_pressed_with_it", async ({ page }) => {
  await recordPosts(page);
  const errors = await open(page, "?players=1");
  await reach(page, 1);
  // macOS sends no keyup for D released while Meta is down.
  await dispatchKey(page, "keydown", "ShiftLeft");
  await dispatchKey(page, "keydown", "MetaLeft", { metaKey: true });
  await dispatchKey(page, "keydown", "KeyD", { metaKey: true });
  await expect.poll(() => lastKeys(page)).toEqual(["KeyD", "MetaLeft", "ShiftLeft"]);
  // Sent in Meta's keyup itself; the other modifiers report their own keyup.
  const afterMeta = await page.evaluate(() => {
    dispatchEvent(new KeyboardEvent("keyup", { code: "MetaLeft" }));
    return (window as unknown as { posted: Posted[] }).posted.at(-1);
  });
  expect(afterMeta).toEqual({ type: "devices", devices: { keys: ["ShiftLeft"], gamepads: [] } });
  await dispatchKey(page, "keyup", "ShiftLeft");
  expect(errors).toEqual([]);
});

test("a_draw_that_cannot_happen_stops_the_page_and_the_match", async ({ page }) => {
  await recordPosts(page);
  await open(page, "?players=1");
  await reach(page, 1);
  await page.keyboard.down("KeyD");
  await expect.poll(() => lastKeys(page)).toEqual(["KeyD"]);
  // A canvas past every device's texture limit: each draw fails with the limit error.
  await page.evaluate(() => {
    document.getElementById("game")!.style.width = "100000px";
    dispatchEvent(new Event("resize"));
  });
  await expect(page.locator("#status")).toContainText("the game cannot be drawn");
  await expect(page.locator("#status")).toContainText("max_texture_dimension_2d");
  await expect(page.locator("body")).toHaveAttribute("data-failed", "true");
  // The held key is released, then play stops: no tick runs after it.
  const last = await page.evaluate(() => (window as unknown as { posted: Posted[] }).posted.slice(-2));
  expect(last).toEqual([RELEASED, { type: "stop" }]);
  await page.waitForTimeout(500);
  const stoppedAt = await page.locator("body").getAttribute("data-tick");
  await page.waitForTimeout(1000);
  await expect(page.locator("body")).toHaveAttribute("data-tick", stoppedAt!);
  await page.keyboard.up("KeyD");
  // The game did not fail: its replay is saved and verifies.
  const downloaded = page.waitForEvent("download");
  await page.getByRole("button", { name: "Save replay" }).click();
  const file = join(mkdtempSync(join(tmpdir(), "ludenea-standalone-")), "swarm.replay");
  await (await downloaded).saveAs(file);
  expect(swarm(["verify", file])).toContain(`${Number(stoppedAt) + 1} hashes match`);
});
