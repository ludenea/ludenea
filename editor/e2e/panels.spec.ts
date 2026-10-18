import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { collectErrors, openScene, serveProject, waitForGame, type ServedProject } from "./project";

const ENTITIES = 300;
const SCROLLED = 3000;

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("panels_game", 4315, (root) => {
    // The template's resources, and enough entities to scroll the scene tree.
    const path = join(root, "scenes/main.scene.ron");
    const resources = readFileSync(path, "utf8").split("entities: [")[0]!.replace(/next_id: \d+/, `next_id: ${ENTITIES}`);
    const pose = (id: number) => `(id: ${id}, components: {"ludenea::Pose": (position:(x:0.0,y:0.0),rotation:(cos:1.0,sin:0.0))}),`;
    writeFileSync(path, `${resources}entities: [\n${Array.from({ length: ENTITIES }, (_, id) => pose(id)).join("\n")}\n    ],\n)\n`);
  });
});

test.afterAll(() => served?.stop());

test("the scene tree keeps its scroll position while its tab is hidden", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto(served.url);
  await openScene(page, "main.scene.ron");
  await waitForGame(page);
  await page.getByRole("tab", { name: "Scene" }).click();
  const tree = page.getByRole("listbox", { name: "Entities" });
  await expect(tree.getByRole("option").nth(20)).toBeVisible();

  const list = page.locator(".ludenea-scene-tree .ludenea-entity-list");
  await list.evaluate((element, top) => (element.scrollTop = top), SCROLLED);
  const row = tree.getByRole("option", { name: /^Pose 150v0/ });
  await expect(row).toBeVisible();

  // The shell unmounts the hidden panel; showing it again mounts a fresh one.
  await page.getByRole("tab", { name: "Assets" }).click();
  await expect(tree).toBeHidden();
  await page.getByRole("tab", { name: "Scene" }).click();
  await expect(row).toBeVisible();
  await expect(row).toBeInViewport();
  expect(await list.evaluate((element) => element.scrollTop)).toBe(SCROLLED);
  expect(errors).toEqual([]);
});
