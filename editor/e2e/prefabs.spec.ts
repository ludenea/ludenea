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

  // Back in the scene: the session replays against the heavier prefab from tick 0, and the
  // override stays.
  await openScene(page, "main.scene.ron");
  await page.getByRole("tab", { name: "Timeline" }).click();
  await expect(page.getByLabel("Divergence at tick 0", { exact: true })).toBeAttached({ timeout: 60_000 });
  expect((await settledTimeline(page)).head).toBe(recorded.head);
  await page.getByRole("tab", { name: "Scene" }).click();
  await expect(balls).toHaveCount(3);
  await balls.nth(2).getByRole("button").click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  // The override holds the radius only: the prefab's new density reaches the placed ball.
  await expect(density).toHaveValue("4");
  await expect(radius).toHaveValue("0.75");
  // The component's mark, and the radius's: no other field is overridden.
  const marks = collider.getByLabel(/ overridden$/);
  await expect(marks).toHaveCount(2);
  const labels = await marks.evaluateAll((elements) => elements.map((element) => element.getAttribute("aria-label")));
  expect(labels.sort()).toEqual(["Collider overridden", "shape.Circle.radius overridden"]);
  expect(errors).toEqual([]);
});
