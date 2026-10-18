import { expect, test } from "@playwright/test";
import { collectErrors, openScene, play, runCommand, serveProject, settledTimeline, waitForGame, type ServedProject } from "./project";

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("prefab_game", 4317);
});

test.afterAll(() => served?.stop());

test("place_override_and_edit_prefab", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto(served.url);
  await openScene(page, "main.scene.ron");
  await waitForGame(page);

  // A third ball, its radius overridden.
  await runCommand(page, "Place prefab");
  await page.getByRole("button", { name: "Place", exact: true }).click();
  const balls = page.getByRole("group", { name: "ball.prefab.ron" });
  await expect(balls).toHaveCount(3);
  await balls.nth(2).getByRole("button").click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  const collider = page.getByRole("region", { name: "ludenea::Collider" });
  const radius = collider.getByLabel(/radius$/);
  await radius.fill("0.75");
  await radius.press("Enter");
  await expect(collider.getByLabel(/radius overridden$/)).toBeVisible();
  await play(page, 60_000, 120);
  const recorded = await settledTimeline(page);

  // Its prefab, heavier.
  await runCommand(page, "Open the selected instance's prefab");
  const prefab = page.getByRole("tree", { name: "Assets" }).getByRole("treeitem", { name: /ball\.prefab\.ron/ });
  await page.getByRole("tab", { name: "Assets" }).click();
  await expect(prefab).toContainText("(open)");
  await page.getByRole("tab", { name: "Scene" }).click();
  await page.getByRole("listbox", { name: "Entities" }).getByRole("option").first().click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  const density = collider.getByLabel("density", { exact: true });
  await density.fill("4");
  await density.press("Enter");
  await page.getByRole("tab", { name: "Assets" }).click();
  await expect(prefab.getByLabel("unsaved changes")).toBeVisible();
  await runCommand(page, "Save scene");
  await expect(prefab.getByLabel("unsaved changes")).not.toBeAttached();

  // Back in the scene: the session replays against the heavier prefab, and the override stays.
  await openScene(page, "main.scene.ron");
  await page.getByRole("tab", { name: "Timeline" }).click();
  await expect(page.getByLabel(/^Divergence at tick \d+$/)).toBeAttached({ timeout: 60_000 });
  expect((await settledTimeline(page)).head).toBe(recorded.head);
  await page.getByRole("tab", { name: "Scene" }).click();
  await expect(balls).toHaveCount(3);
  await balls.nth(0).getByRole("button").click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  await expect(density).toHaveValue("4");
  // The override holds the whole collider as it was then, the prefab's density included.
  await page.getByRole("tab", { name: "Scene" }).click();
  await balls.nth(2).getByRole("button").click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  await expect(radius).toHaveValue("0.75");
  await expect(collider.getByLabel(/radius overridden$/)).toBeVisible();
  expect(errors).toEqual([]);
});
