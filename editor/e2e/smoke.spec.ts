import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { expect, test } from "@playwright/test";

declare global {
  interface Window {
    /** Counted by the shared React module each time it is evaluated. */
    __ludeneaReactInstances?: number;
  }
}

const repository = resolve(import.meta.dirname, "../..");
const ludenea = join(repository, "target/debug/ludenea");
const port = 4311;

// A project plugin using React hooks: they fail if the plugin gets a React of its own.
const PLUGIN = `import { definePlugin } from "@ludenea/editor-shell";
import { createElement, useState } from "react";

function SmokePanel() {
  const [text] = useState("the project plugin renders");
  return createElement("p", null, text);
}

export default definePlugin({
  name: "smoke_game",
  activate(editor) {
    editor.panels.register({ id: "smoke_game.panel", title: "Smoke", component: SmokePanel });
  },
});
`;

let directory: string;
let server: ChildProcess;

test.beforeAll(async () => {
  directory = mkdtempSync(join(tmpdir(), "ludenea-smoke-"));
  const project = join(directory, "smoke_game");
  execFileSync(ludenea, ["new", project, "--engine-path", repository]);
  writeFileSync(join(project, "editor/index.ts"), PLUGIN);
  server = spawn(ludenea, ["serve", project, "--port", String(port), "--editor-dir", join(repository, "editor/apps/web/dist")], {
    // One target directory for every run, so only the first one builds the engine.
    env: { ...process.env, CARGO_TARGET_DIR: join(repository, "target/e2e-smoke") },
    stdio: ["ignore", "pipe", "inherit"],
  });
  await new Promise<void>((resolve, reject) => {
    server.stdout!.on("data", (chunk: Buffer) => {
      if (chunk.toString().includes("listening on")) resolve();
    });
    server.on("exit", (code) => reject(new Error(`ludenea serve exited with ${code}`)));
  });
});

test.afterAll(() => {
  server?.kill();
  if (directory) rmSync(directory, { recursive: true, force: true });
});

test("web editor opens the template project", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
    console.log(`[page] ${message.text()}`);
  });
  const opened = Date.now();
  await page.goto(`http://localhost:${port}/`);
  await page.getByRole("tab", { name: "Assets" }).click();
  const assets = page.getByRole("tree", { name: "Assets" });
  await expect(assets.getByRole("treeitem", { name: /main\.scene\.ron/ })).toBeVisible({ timeout: 2000 });
  await expect(assets).toContainText("scenes/");
  // The shell is usable before any build.
  expect(Date.now() - opened).toBeLessThan(2000);

  await page.getByRole("tab", { name: "Smoke" }).click();
  await expect(page.getByText("the project plugin renders")).toBeVisible();
  expect(await page.evaluate(() => window.__ludeneaReactInstances)).toBe(1);

  await page.getByRole("tab", { name: "Viewport" }).click();
  const viewport = page.getByRole("application", { name: "Viewport" });
  await expect(viewport).toBeVisible();
  // The first build of the engine for the browser takes minutes.
  await expect(viewport).not.toContainText("No game is running.", { timeout: 9 * 60 * 1000 });
  await expect(viewport).not.toContainText("cannot render");
  expect(errors).toEqual([]);
});
