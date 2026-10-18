import { expect, test, type Page } from "@playwright/test";
import { button, collectErrors, expectTick, openScene, play, seek, serveProject, settledTimeline, waitForGame, type ServedProject } from "./project";

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("branch_game", 4318);
});

test.afterAll(() => served?.stop());

/** The hash at `tick`, once it no longer waits for edits to be simulated. */
const hashAt = async (page: Page, tick: number) => {
  await seek(page, tick);
  await expect(page.getByLabel("Hash")).not.toContainText("pending");
  return page.getByLabel("Hash").textContent();
};

test("fork_switch_and_keep_parent_future", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto(served.url);
  await openScene(page, "main.scene.ron");
  await waitForGame(page);
  await play(page, 60_000, 300);
  const parent = await settledTimeline(page);
  const parentHash = await hashAt(page, 150);

  // A ball moved at tick 100 forks a branch from the recorded future.
  await seek(page, 100);
  await page.getByRole("tab", { name: "Scene" }).click();
  await page.getByRole("group", { name: "ball.prefab.ron" }).first().getByRole("button").click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  const x = page.getByRole("region", { name: "ludenea::Pose" }).getByLabel("position.x");
  await x.fill("3");
  await x.press("Enter");
  await page.getByRole("tab", { name: "Timeline" }).click();
  const lanes = page.getByRole("list", { name: "Branches" }).getByRole("listitem");
  await expect(lanes).toHaveCount(2);
  await expectTick(page, 100, 100);
  await play(page, 60_000, 200);
  const child = await settledTimeline(page);
  const childHash = await hashAt(page, 150);
  expect(childHash).not.toBe(parentHash);

  // Each branch keeps its own future.
  await lanes.nth(0).getByRole("button").click();
  await expect(lanes.nth(0).getByRole("button")).toHaveAttribute("aria-current", "true");
  expect((await settledTimeline(page)).head).toBe(parent.head);
  expect(await hashAt(page, 150)).toBe(parentHash);
  await lanes.nth(1).getByRole("button").click();
  await expect(lanes.nth(1).getByRole("button")).toHaveAttribute("aria-current", "true");
  expect((await settledTimeline(page)).head).toBe(child.head);
  expect(await hashAt(page, 150)).toBe(childHash);

  // Undoing the edit removes its branch; redoing it brings the branch back.
  await button(page, "Undo").click();
  await expect(lanes).toHaveCount(1);
  expect((await settledTimeline(page)).head).toBe(parent.head);
  await button(page, "Redo").click();
  await expect(lanes).toHaveCount(2);
  expect(errors).toEqual([]);
});
