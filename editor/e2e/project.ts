import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
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
