import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { expect, type Page } from "@playwright/test";

export const repository = resolve(import.meta.dirname, "../..");
const ludenea = join(repository, "target/debug/ludenea");
const editorDir = join(repository, "editor/apps/web/dist");
/** One target directory for every run and every spec, so only the first one builds the engine. */
export const targetDir = join(repository, "target/e2e");

/** A project made by `ludenea new`, served by `ludenea serve` with the built web editor. */
export interface ServedProject {
  /** The project's root directory. */
  root: string;
  url: string;
  stop(): void;
}

/** Creates the project `name` from the template, lets `prepare` change it, then serves it on `port`. */
export async function serveProject(name: string, port: number, prepare: (root: string) => void = () => {}): Promise<ServedProject> {
  expect(existsSync(ludenea), `${ludenea} is missing: run cargo build -p ludenea-serve`).toBe(true);
  expect(existsSync(join(editorDir, "index.html")), `${editorDir} is missing: run pnpm -C editor build`).toBe(true);
  const directory = mkdtempSync(join(tmpdir(), `ludenea-${name}-`));
  const root = join(directory, name);
  execFileSync(ludenea, ["new", root, "--engine-path", repository]);
  prepare(root);
  const server: ChildProcess = spawn(ludenea, ["serve", root, "--port", String(port), "--editor-dir", editorDir], {
    env: { ...process.env, CARGO_TARGET_DIR: targetDir },
    stdio: ["ignore", "pipe", "inherit"],
  });
  await new Promise<void>((resolve, reject) => {
    server.stdout!.on("data", (chunk: Buffer) => {
      if (chunk.toString().includes("listening on")) resolve();
    });
    server.on("exit", (code) => reject(new Error(`ludenea serve exited with ${code}`)));
  });
  return {
    root,
    url: `http://localhost:${port}/`,
    stop() {
      server.kill();
      rmSync(directory, { recursive: true, force: true });
    },
  };
}

/** Collects page errors and console errors, and echoes the console. */
export function collectErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
    console.log(`[page] ${message.text()}`);
  });
  return errors;
}

/**
 * Fails on the `errors` that `collectErrors` collected, then on the errors the editor reported
 * in its Console panel (failed commands, viewport and divergence actions, builds, the game),
 * which never reach the devtools console.
 */
export async function expectNoErrors(page: Page, errors: string[]): Promise<void> {
  expect(errors).toEqual([]);
  await page.getByRole("tab", { name: "Console", exact: true }).click();
  const lines = page.getByRole("list", { name: "Console" });
  await expect(lines).toBeVisible();
  expect(await lines.locator(".ludenea-console-error").allTextContents(), "errors in the editor console").toEqual([]);
}

/** Chromium's launch arguments: rendering through SwiftShader, so the viewport draws without a GPU. */
export const launchArgs = ["--use-angle=swiftshader"];

/** The first build of the engine for the browser takes minutes, more on a cold CI runner. */
export const FIRST_BUILD_MS = (process.env.CI ? 18 : 9) * 60 * 1000;

/** Opens the scene file `name` (in `scenes/`) from the asset browser. */
export async function openScene(page: Page, name: string): Promise<void> {
  await page.getByRole("tab", { name: "Assets" }).click();
  await page.getByRole("tree", { name: "Assets" }).getByRole("treeitem", { name: new RegExp(name.replaceAll(".", "\\.")) }).dblclick();
}

/** The timeline's current tick and head, once a game runs. */
export async function timeline(page: Page): Promise<{ current: number; head: number }> {
  const text = await page.getByText(/^Tick \d+ of \d+/).textContent();
  const [, current, head] = /^Tick (\d+) of (\d+)/.exec(text ?? "")!;
  return { current: Number(current), head: Number(head) };
}

/** Waits for the timeline to show tick `current` (and `head` when given). */
export async function expectTick(page: Page, current: number, head?: number): Promise<void> {
  const pattern = head === undefined ? new RegExp(`^Tick ${current} of `) : new RegExp(`^Tick ${current} of ${head},`);
  await expect(page.getByText(/^Tick \d+ of \d+/)).toHaveText(pattern);
}

/** Waits until a game runs and shows its timeline. */
export async function waitForGame(page: Page): Promise<void> {
  await page.getByRole("tab", { name: "Timeline" }).click();
  await expect(page.getByText(/^Tick \d+ of \d+/)).toBeVisible({ timeout: FIRST_BUILD_MS });
}

export const button = (page: Page, name: string) => page.getByRole("button", { name, exact: true });

/** Plays for `ms` milliseconds of wall time, or until tick `until` shows, then pauses and returns the tick reached. */
export async function play(page: Page, ms: number, until?: number): Promise<number> {
  await button(page, "Play").click();
  await expect(button(page, "Pause")).toBeVisible();
  if (until === undefined) await page.waitForTimeout(ms);
  else await expect.poll(async () => (await timeline(page)).current, { timeout: ms }).toBeGreaterThanOrEqual(until);
  await button(page, "Pause").click();
  await expect(button(page, "Play")).toBeVisible();
  return (await settledTimeline(page)).current;
}

/** The timeline once it stops changing (the refresh after `Paused` brings the head). */
export async function settledTimeline(page: Page): Promise<{ current: number; head: number }> {
  let last = "";
  await expect
    .poll(async () => {
      const now = JSON.stringify(await timeline(page));
      const settled = now === last;
      last = now;
      return settled;
    }, { intervals: [250] })
    .toBe(true);
  return JSON.parse(last);
}

export async function seek(page: Page, tick: number): Promise<void> {
  await page.getByRole("slider", { name: "Tick" }).fill(String(tick));
  await expectTick(page, tick);
}

/** Runs the editor command titled `title` from the command palette. */
export async function runCommand(page: Page, title: string): Promise<void> {
  await page.keyboard.press("Control+k");
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await palette.getByRole("combobox", { name: "Command" }).fill(title);
  // An option's name ends with its shortcut, when it has one.
  const escaped = title.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  await palette.getByRole("option", { name: new RegExp(`^${escaped}`) }).first().click();
  await expect(palette).not.toBeAttached();
}

/** Runs the project's own binary (native, built by cargo) with `args`; returns its output. */
export function cli(root: string, args: string[]): string {
  return execFileSync("cargo", ["run", "--quiet", "--locked", "--manifest-path", join(root, "Cargo.toml"), "--", ...args], {
    env: { ...process.env, CARGO_TARGET_DIR: targetDir },
    encoding: "utf8",
  });
}

/**
 * A project made by `ludenea new` whose game is the `boxes` example (its sources copied in,
 * under the crate name `boxes`, so its type names match), without the template's assets:
 * files the `boxes` app wrote open in it.
 */
export function serveBoxes(port: number, prepare: (root: string) => void = () => {}): Promise<ServedProject> {
  return serveProject("boxes", port, (root) => {
    for (const file of ["lib.rs", "scene.rs"]) copyFileSync(join(repository, "examples/boxes/src", file), join(root, "src", file));
    for (const directory of ["assets", "scenes"]) {
      rmSync(join(root, directory), { recursive: true, force: true });
      mkdirSync(join(root, directory));
    }
    prepare(root);
  });
}

/** The fixtures of the `boxes` app, written by `cargo run -p boxes --bin divergence-fixtures editor/e2e/fixtures/divergence`. */
export const DIVERGENCE_FIXTURES = join(import.meta.dirname, "fixtures/divergence");
