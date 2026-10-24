import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { type Browser, type BrowserContext, expect, type Page, test } from "@playwright/test";
import { launchArgs, repository } from "./project";

/** The arena built as a standalone web page, which vite serves under `/arena/`. */
const ARENA_PAGE = join(import.meta.dirname, "fixtures", "arena");
const BUILD = "cargo run --locked -p ludenea-serve -- build examples/arena --web --out editor/e2e/fixtures/arena";
const binary = (name: string) => join(process.env.CARGO_TARGET_DIR ?? join(repository, "target"), "debug", process.platform === "win32" ? `${name}.exe` : name);
/** The engine's command line tool and the arena's own, built by `cargo build --locked -p ludenea-serve -p arena`. */
const LUDENEA = binary("ludenea");
const ARENA = binary("arena");
const ARENA_SCENE = join(repository, "examples/arena/assets/arena.scene.ron");
/** The characters of a room code. */
const ROOM_ALPHABET = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";
/** How long two peers may take to meet, connect and start. */
const MEET_MS = 60_000;

/** The signaling listener of this file's tests, and its URL. */
let rooms: ChildProcess | undefined;
let signaling = "";

/** The first line of `child`'s standard output that matches `pattern`, or a failure when the
 * child exits or `timeout` passes first. Every line is echoed to the test's log. */
function lineOf(child: ChildProcess, pattern: RegExp, name: string, timeout = MEET_MS): Promise<RegExpMatchArray> {
  return new Promise((resolve, reject) => {
    const lines = createInterface({ input: child.stdout! });
    const timer = setTimeout(() => reject(new Error(`${name} printed no line matching ${pattern} within ${timeout} ms`)), timeout);
    lines.on("line", (line) => {
      console.log(`[${name}] ${line}`);
      const found = line.match(pattern);
      if (found) {
        clearTimeout(timer);
        resolve(found);
      }
    });
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`${name} exited with ${code} before printing a line matching ${pattern}`));
    });
  });
}

/** Echoes `child`'s standard error to the test's log. */
function echoErrors(child: ChildProcess, name: string): void {
  createInterface({ input: child.stderr! }).on("line", (line) => console.log(`[${name}] ${line}`));
}

test.beforeAll(async ({ baseURL }) => {
  expect(existsSync(LUDENEA), `${LUDENEA} is missing: run cargo build --locked -p ludenea-serve`).toBe(true);
  // The page is served by vite, on another origin than the listener's.
  rooms = spawn(LUDENEA, ["rooms", "--listen", "127.0.0.1", "--port", "0", "--allow-origin", new URL(baseURL!).origin]);
  echoErrors(rooms, "rooms");
  signaling = (await lineOf(rooms, /^signaling (\S+)$/, "rooms"))[1]!;
});

test.afterAll(() => {
  rooms?.kill();
});

/** The contexts a test opened: each page keeps its game and link running until closed. */
const contexts: BrowserContext[] = [];

test.afterEach(async () => {
  for (const context of contexts.splice(0)) await context.close();
});

/** Opens the arena's page in a context of its own with `query`, collecting its errors; `init`
 * runs in the page before its own scripts. */
async function open(browser: Browser, query: string, init?: () => void): Promise<{ page: Page; errors: string[] }> {
  expect(existsSync(join(ARENA_PAGE, "game.json")), `${ARENA_PAGE} is missing: run ${BUILD}`).toBe(true);
  const context = await browser.newContext({ acceptDownloads: true });
  contexts.push(context);
  if (init !== undefined) await context.addInitScript(init);
  const page = await context.newPage();
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => console.log(`[page ${query}] ${message.text()}`));
  await page.goto(`/arena/index.html${query}`);
  return { page, errors };
}

/** The page's attribute `name` on the body, once it is set; fails when the page fails first. */
async function attribute(page: Page, name: string, timeout = MEET_MS): Promise<string> {
  await page.waitForFunction((name) => document.body.getAttribute(name) !== null || document.body.dataset.failed !== undefined, name, { timeout });
  await expect(page.locator("body"), await page.locator("#status").innerText()).not.toHaveAttribute("data-failed");
  return (await page.locator("body").getAttribute(name))!;
}

/** Waits for the page's match to be in `state`; fails when the page fails first. */
async function reachState(page: Page, state: string, timeout = MEET_MS): Promise<void> {
  await page.waitForFunction((state) => document.body.dataset.online === state || document.body.dataset.failed !== undefined, state, { timeout });
  await expect(page.locator("body"), await page.locator("#status").innerText()).not.toHaveAttribute("data-failed");
  await expect(page.locator("body")).toHaveAttribute("data-online", state);
}

/** Waits for the page to reach `tick`. */
async function reach(page: Page, tick: number): Promise<void> {
  await page.waitForFunction((tick) => Number(document.body.dataset.tick) >= tick || document.body.dataset.failed !== undefined, tick, { timeout: 120_000 });
  await expect(page.locator("body"), await page.locator("#status").innerText()).not.toHaveAttribute("data-failed");
}

/** Saves the page's replay into a file of its own and returns the file. */
async function saveReplay(page: Page): Promise<string> {
  const downloaded = page.waitForEvent("download");
  await page.getByRole("button", { name: "Save replay" }).click();
  const download = await downloaded;
  expect(download.suggestedFilename()).toBe("arena.replay");
  const file = join(mkdtempSync(join(tmpdir(), "ludenea-online-")), "arena.replay");
  await download.saveAs(file);
  return file;
}

function arena(args: string[]): string {
  expect(existsSync(ARENA), `${ARENA} is missing: run cargo build --locked -p arena`).toBe(true);
  return execFileSync(ARENA, args, { encoding: "utf8" });
}

/** A room code no other test uses. */
function randomRoom(): string {
  return Array.from({ length: 6 }, () => ROOM_ALPHABET[Math.floor(Math.random() * ROOM_ALPHABET.length)]).join("");
}

test("two pages play the arena and save the same replay", async ({ browser }) => {
  const host = await open(browser, `?host=2&players=1&stun=none&signaling=${signaling}`);
  const room = await attribute(host.page, "data-room");
  await expect(host.page.locator("#room")).toContainText(room);
  const joiner = await open(browser, `?join=${room}&players=1&stun=none&signaling=${signaling}`);
  await reachState(host.page, "running");
  await reachState(joiner.page, "running");
  await expect(host.page.locator("#leave")).toBeEnabled();
  // The host's player runs right for a second.
  await host.page.bringToFront();
  await host.page.keyboard.down("ArrowRight");
  await host.page.waitForTimeout(1000);
  await host.page.keyboard.up("ArrowRight");
  await reach(host.page, 300);
  await reach(joiner.page, 300);
  await host.page.locator("#leave").click();
  for (const { page } of [host, joiner]) {
    await reachState(page, "ended");
    await expect(page.locator("body")).toHaveAttribute("data-end", "left");
  }
  // Both ended at the same tick, in the same state.
  const ended = async (page: Page) => [await page.locator("body").getAttribute("data-tick"), await page.locator("body").getAttribute("data-hash")];
  const [tick, hash] = await ended(host.page);
  expect(await ended(joiner.page)).toEqual([tick, hash]);
  expect(Number(tick)).toBeGreaterThanOrEqual(300);
  const hosted = await saveReplay(host.page);
  const joined = await saveReplay(joiner.page);
  expect(readFileSync(joined).equals(readFileSync(hosted)), "the two replays differ").toBe(true);
  for (const file of [hosted, joined]) expect(arena(["verify", file])).toContain("hashes match");
  // "Save desync snapshot", offered for a desync only, saves the session's state at the end's
  // tick, named after it and each page's peer: the same state on both pages. The end names
  // the last tick played, the tick before the state the page shows.
  const endTick = Number(tick) - 1;
  const snapshots: Buffer[] = [];
  for (const [peer, { page }] of [host, joiner].entries()) {
    await page.evaluate(() => {
      document.getElementById("snapshot")!.hidden = false;
    });
    const downloaded = page.waitForEvent("download");
    await page.getByRole("button", { name: "Save desync snapshot" }).click();
    const download = await downloaded;
    expect(download.suggestedFilename()).toBe(`desync-tick-${endTick}-peer-${peer}.snap`);
    const file = join(mkdtempSync(join(tmpdir(), "ludenea-online-")), download.suggestedFilename());
    await download.saveAs(file);
    snapshots.push(readFileSync(file));
  }
  expect(snapshots[1]!.equals(snapshots[0]!), "the two snapshots differ").toBe(true);
  // Once the others were told, each page closes its link, and the room leaves the listener.
  for (const { page } of [host, joiner]) await expect(page.locator("body")).toHaveAttribute("data-link", "closed");
  const late = await open(browser, `?join=${room}&players=1&stun=none&signaling=${signaling}`);
  await expect(late.page.locator("body")).toHaveAttribute("data-failed", "true", { timeout: MEET_MS });
  await expect(late.page.locator("#status")).toContainText(`cannot join room ${room}`);
  // The host's keys reached the match: an idle match of two players is elsewhere at that tick.
  const idle = await open(browser, `?players=2&ticks=${tick}`);
  await reach(idle.page, Number(tick));
  await expect(idle.page.locator("body")).toHaveAttribute("data-tick", tick!);
  expect(await idle.page.locator("body").getAttribute("data-hash")).not.toBe(hash);
  for (const { errors } of [host, joiner, idle]) expect(errors).toEqual([]);
});

test("a native peer and a page play under bad conditions", async ({ browser }) => {
  // The page reaches the native peer with the browser's own candidates, mDNS names included.
  expect(launchArgs.join(" ")).not.toMatch(/mdns/i);
  const room = randomRoom();
  const record = join(mkdtempSync(join(tmpdir(), "ludenea-online-")), "peer.replay");
  expect(existsSync(ARENA), `${ARENA} is missing: run cargo build --locked -p arena`).toBe(true);
  const peer = spawn(ARENA, [
    "peer", ARENA_SCENE, "--host", "--room", room, "--players", "1", "--random-inputs", "7", "--ticks", "600",
    "--no-stun", "--latency-ms", "50", "--jitter-ms", "10", "--loss-percent", "5", "--record", record, "--signaling", signaling,
  ]);
  echoErrors(peer, "peer");
  const exited = new Promise<number | null>((resolve) => peer.once("exit", resolve));
  try {
    await lineOf(peer, new RegExp(`^room ${room}$`), "peer");
    const page = await open(browser, `?join=${room}&players=1&stun=none&latency=50&jitter=10&loss=5&signaling=${signaling}`);
    await reachState(page.page, "running");
    expect(await Promise.race([exited, new Promise((resolve) => setTimeout(() => resolve("still running"), 180_000))])).toBe(0);
    await reachState(page.page, "ended");
    await expect(page.page.locator("body")).toHaveAttribute("data-end", "left");
    const file = await saveReplay(page.page);
    expect(readFileSync(file).equals(readFileSync(record)), "the page's replay differs from the peer's").toBe(true);
    for (const replay of [file, record]) expect(arena(["verify", replay])).toContain("hashes match");
    expect(page.errors).toEqual([]);
  } finally {
    peer.kill();
  }
});

test("an unknown room says so", async ({ browser }) => {
  const room = randomRoom();
  const { page } = await open(browser, `?join=${room}&players=1&stun=none&signaling=${signaling}`);
  await expect(page.locator("body")).toHaveAttribute("data-failed", "true", { timeout: MEET_MS });
  await expect(page.locator("#status")).toContainText(`cannot join room ${room}`);
});

test("bad parameters fail the page naming them", async ({ browser }) => {
  const refused: [string, string][] = [
    ["?host=9", "?host: a room holds 2 to 4 peers, not 9"],
    ["?join=0O0O0O", "?join: '0' (character 1) is not in a room code"],
    // Without a room, an online parameter alone is refused too.
    ["?loss=150", "?loss"],
    ["?stun=http://x", "?stun"],
    ["?join=ABCDEF&loss=150", "?loss: a peer loses 0 to 100 percent of its packets, not 150"],
    ["?host=2&stun=http://x", "?stun: the STUN URL 'http://x' must start with stun: or stuns:"],
  ];
  for (const [query, named] of refused) {
    const { page } = await open(browser, query);
    await expect(page.locator("body")).toHaveAttribute("data-failed", "true");
    await expect(page.locator("#status")).toContainText(named);
  }
});

test("a join the game refuses leaves the room", async ({ browser }) => {
  const host = await open(browser, `?host=2&players=1&stun=none&signaling=${signaling}`);
  const room = await attribute(host.page, "data-room");
  // The joiner's page hands its worker one claim too many, which the game refuses.
  const joiner = await open(browser, `?join=${room}&players=1&stun=none&signaling=${signaling}`, () => {
    const post = Worker.prototype.postMessage;
    Worker.prototype.postMessage = function (this: Worker, message: { type?: string; claims?: unknown[] }, ...rest: unknown[]) {
      const sent = message?.type === "join" ? { ...message, claims: [...message.claims!, ...message.claims!] } : message;
      return Reflect.apply(post, this, [sent, ...rest]);
    };
  });
  await expect(joiner.page.locator("body")).toHaveAttribute("data-failed", "true", { timeout: MEET_MS });
  await expect(joiner.page.locator("#status")).toContainText("the game refused join_match: 2 claims for 1 local players");
  // The page left the room: it no longer pumps a link the match never used.
  await expect(joiner.page.locator("body")).toHaveAttribute("data-link", "closed");
  expect(joiner.errors).toEqual([]);
});
