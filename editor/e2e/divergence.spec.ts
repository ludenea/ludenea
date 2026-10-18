import { copyFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { button, collectErrors, DIVERGENCE_FIXTURES, openScene, play, repository, runCommand, seek, serveBoxes, serveProject, waitForGame, type ServedProject } from "./project";

/** Opens the project file `name` from the asset browser. */
async function openAsset(page: Page, name: string): Promise<void> {
  await page.getByRole("tab", { name: "Assets" }).click();
  await page.getByRole("tree", { name: "Assets" }).getByRole("treeitem", { name: new RegExp(name.replaceAll(".", "\\.")) }).dblclick();
}

test.describe("the trace and snapshot viewer", () => {
  let served: ServedProject;

  // The fixtures come from the `boxes` app: a snapshot only compares in a game of the same types.
  test.beforeAll(async () => {
    served = await serveBoxes(4316, (root) => {
      copyFileSync(join(repository, "examples/boxes/assets/boxes.scene.ron"), join(root, "scenes/boxes.scene.ron"));
      mkdirSync(join(root, "assets/divergence"));
      for (const file of ["a.trace", "b.trace", "a.snap", "b.snap"]) copyFileSync(join(DIVERGENCE_FIXTURES, file), join(root, "assets/divergence", file));
    });
  });

  test.afterAll(() => served?.stop());

  test("trace_and_snapshot_viewer", async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(served.url);
    await openScene(page, "boxes.scene.ron");
    await waitForGame(page);

    await openAsset(page, "a.trace");
    const traces = page.getByRole("region", { name: "Trace comparison" });
    await traces.getByLabel("Compare the trace with").selectOption("assets/divergence/b.trace");
    // `b` moved a crate at tick 60 of 120.
    await expect(traces).toContainText("First divergent tick 60 of 121 common ticks.");
    await expect(traces.getByRole("button", { name: "First divergent tick 60" })).toBeVisible();

    await openAsset(page, "a.snap");
    const snapshots = page.getByRole("region", { name: "Snapshot comparison" });
    await snapshots.getByLabel("Compare the snapshot with").selectOption("assets/divergence/b.snap");
    const differences = snapshots.getByRole("tree", { name: "Snapshot differences" });
    // The moved crate, the first after the container's four walls: its pose's x only.
    const entity = differences.getByRole("treeitem", { name: "entity 4v0" });
    await expect(entity).toBeVisible();
    const pose = entity.getByRole("treeitem", { name: "ludenea::Pose" });
    await expect(pose.getByRole("treeitem")).toHaveCount(1);
    await expect(pose.getByRole("treeitem")).toContainText("position.x");
    await expect(differences.locator(":scope > li")).toHaveCount(1);
    expect(errors).toEqual([]);
  });
});

test.describe("the ghost and the causal report", () => {
  let served: ServedProject;

  test.beforeAll(async () => {
    served = await serveProject("ghost_game", 4315);
  });

  test.afterAll(() => served?.stop());

  test("ghost_and_causal_report", async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(served.url);
    await openScene(page, "main.scene.ron");
    await waitForGame(page);
    await play(page, 60_000, 150);
    const fork = 30;
    await seek(page, fork);

    // The template's two balls fall side by side, 3 m apart: thrown at the other one, the
    // first ball starts a collision the parent branch never had.
    await page.getByRole("tab", { name: "Scene" }).click();
    const balls = page.getByRole("group", { name: "ball.prefab.ron" });
    await expect(balls).toHaveCount(2);
    const thrown = (await balls.nth(0).getByRole("button").textContent())!;
    const hit = (await balls.nth(1).getByRole("button").textContent())!;
    await balls.nth(0).getByRole("button").click();
    await page.getByRole("tab", { name: "Inspector" }).click();
    const velocity = page.getByRole("region", { name: "ludenea::Body" }).getByLabel("linear_velocity.x");
    await velocity.fill("-20");
    await velocity.press("Enter");
    await page.getByRole("tab", { name: "Timeline" }).click();
    const lanes = page.getByRole("list", { name: "Branches" }).getByRole("listitem");
    await expect(lanes).toHaveCount(2);
    await play(page, 60_000, fork + 120);

    // The parent branch as the ghost.
    await button(page, "Compare with").click();
    const compare = page.getByRole("group", { name: "Compare branch" });
    await compare.getByLabel("Branch").selectOption({ label: "main" });
    await compare.getByRole("button", { name: "Compare" }).click();
    await expect(lanes.nth(0)).toContainText("compare");

    // Shortly after the fork, the thrown ball and its ghost are a few meters apart.
    await seek(page, fork + 5);
    const viewport = page.getByRole("application", { name: "Viewport" });
    await page.getByRole("tab", { name: "Viewport" }).click();
    await runCommand(page, "Frame the selection in the viewport");
    for (let step = 0; step < 4; step++) await runCommand(page, "Zoom the viewport out");
    const withGhost = await viewport.screenshot();
    await runCommand(page, "Toggle the ghost of the compare branch");
    await expect.poll(async () => (await viewport.screenshot()).equals(withGhost)).toBe(false);
    await runCommand(page, "Toggle the ghost of the compare branch");
    await expect.poll(async () => (await viewport.screenshot()).equals(withGhost)).toBe(true);

    await page.getByRole("tab", { name: "Divergence report" }).click();
    const report = page.getByRole("tree", { name: "Divergence report" });
    await expect(report.getByRole("treeitem", { name: `${thrown} at tick ${fork}`, exact: true })).toBeVisible({ timeout: 60_000 });
    const collided = report.getByRole("treeitem", { name: new RegExp(`^${hit} at tick \\d+$`) });
    await expect(collided).toContainText(`ludenea::Collision`);
    await expect(collided).toContainText(`with ${thrown}`);
    expect(errors).toEqual([]);
  });
});
