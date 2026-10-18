import { copyFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { collectErrors, expectTick, openScene, serveProject, timeline, waitForGame, type ServedProject } from "./project";

declare global {
  interface Window {
    /** Globals that plugin code read or wrote directly, recorded by the probe below. */
    __pluginGlobalAccess: string[];
  }
}

/**
 * Wraps the browser globals a plugin could reach around its `editor` argument (network,
 * storage, workers, messaging, other windows, the global object itself) and records the ones
 * read or written by code in a plugin bundle (`/plugins/`). Access from the shared modules a
 * plugin calls is the editor's own and is not recorded. `window`, `document`, `location`,
 * and `top` cannot be redefined, so they are not probed: of the document only its cookies
 * are, and a plugin changing the page through `document` goes unseen. This checks that the
 * plugin API is enough for a plugin, not that a plugin cannot escape it.
 */
function installGlobalProbe() {
  const names = [
    "fetch", "XMLHttpRequest", "WebSocket", "EventSource", "localStorage", "sessionStorage", "indexedDB", "caches",
    "navigator", "history", "postMessage", "Worker", "SharedWorker", "BroadcastChannel", "MessageChannel", "open",
    "opener", "parent", "frames", "self", "globalThis",
  ];
  const accessed: string[] = [];
  window.__pluginGlobalAccess = accessed;
  // Stack line 0 is the message, 1 `record`, 2 the accessor, 3 the code that touched the global.
  const record = (name: string) => {
    if (new Error().stack?.split("\n")[3]?.includes("/plugins/")) accessed.push(name);
  };
  const probe = (target: object, name: string, recorded: string) => {
    const descriptor = Object.getOwnPropertyDescriptor(target, name);
    // Each name is expected to be probeable: one that is not would go unseen, so it fails the
    // test (as a page error) instead.
    if (!descriptor?.configurable) throw new Error(`the probe cannot wrap ${recorded}`);
    let value = descriptor.get ? undefined : descriptor.value;
    Object.defineProperty(target, name, {
      configurable: true,
      enumerable: descriptor.enumerable,
      get() {
        record(recorded);
        return descriptor.get ? descriptor.get.call(this) : value;
      },
      set(next) {
        record(recorded);
        if (descriptor.set) descriptor.set.call(this, next);
        else value = next;
      },
    });
  };
  for (const name of names) probe(window, name, name);
  probe(Document.prototype, "cookie", "document.cookie");
}

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("plugin_game", 4313);
});

test.afterAll(() => served?.stop());

test("a project plugin has the same API as built-ins", async ({ page }) => {
  const errors = collectErrors(page);
  await page.addInitScript(installGlobalProbe);
  await page.goto(served.url);
  await openScene(page, "main.scene.ron");
  await waitForGame(page);
  for (let step = 0; step < 3; step++) await page.getByRole("button", { name: "Step", exact: true }).click();
  await expectTick(page, 3);
  const before = await timeline(page);

  // Swap the template's plugin for the fixture while the game runs: the editor hot reloads it.
  copyFileSync(join(import.meta.dirname, "fixtures/test-plugin/index.tsx"), join(served.root, "editor/index.tsx"));
  rmSync(join(served.root, "editor/index.ts"));
  await expect(page.getByRole("tab", { name: "Test plugin" })).toBeVisible({ timeout: 60_000 });

  await page.getByRole("tab", { name: "Scene" }).click();
  // The second entity of the template scene is a dynamic box.
  await page.getByRole("listbox", { name: "Entities" }).getByRole("option").nth(1).click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  await expect(page.getByRole("region", { name: "ludenea::Body" })).toContainText("Custom body inspector: Dynamic");
  await page.getByRole("tab", { name: "Test plugin" }).click();
  await expect(page.getByText("Body read by the plugin: Dynamic")).toBeVisible();

  // The same game kept running: no restart, no restore.
  await page.getByRole("tab", { name: "Timeline" }).click();
  expect(await timeline(page)).toEqual(before);
  expect(await page.evaluate(() => window.__pluginGlobalAccess)).toEqual([]);
  expect(errors).toEqual([]);
});
