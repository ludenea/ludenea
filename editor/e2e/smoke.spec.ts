import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { collectErrors, expectNoErrors, FIRST_BUILD_MS, serveProject, type ServedProject } from "./project";

declare global {
  interface Window {
    /** Counted by the shared React module each time it is evaluated. */
    __ludeneaReactInstances?: number;
  }
}

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

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("smoke_game", 4311, (root) => writeFileSync(join(root, "editor/index.ts"), PLUGIN));
});

test.afterAll(() => served?.stop());

test("web editor opens the template project", async ({ page }) => {
  const errors = collectErrors(page);
  const opened = Date.now();
  await page.goto(served.url);
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
  await expect(viewport).not.toContainText("No game is running.", { timeout: FIRST_BUILD_MS });
  await expect(viewport).not.toContainText("stopped rendering");
  // The project opened its default scene: the scene tree lists its entities without a double click.
  await page.getByRole("tab", { name: "Scene" }).click();
  await expect(page.getByRole("listbox", { name: "Entities" }).getByRole("option").first()).toBeVisible();
  await expectNoErrors(page, errors);
});
