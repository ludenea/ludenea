import { copyFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { button, cli, collectErrors, DIVERGENCE_FIXTURES, openScene, play, repository, seek, serveBoxes, serveProject, waitForGame, type ServedProject } from "./project";

test.describe("a saved branch", () => {
  let served: ServedProject;

  test.beforeAll(async () => {
    served = await serveProject("replay_game", 4319);
  });

  test.afterAll(() => served?.stop());

  test("saved_replay_verifies_in_cli", async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(served.url);
    await openScene(page, "main.scene.ron");
    await waitForGame(page);
    await play(page, 60_000, 200);
    await seek(page, 100);

    // A branch with a past edit and a future of its own.
    await page.getByRole("tab", { name: "Scene" }).click();
    await page.getByRole("group", { name: "ball.prefab.ron" }).first().getByRole("button").click();
    await page.getByRole("tab", { name: "Inspector" }).click();
    const x = page.getByRole("region", { name: "ludenea::Pose" }).getByLabel("position.x");
    await x.fill("3");
    await x.press("Enter");
    await page.getByRole("tab", { name: "Timeline" }).click();
    await expect(page.getByRole("list", { name: "Branches" }).getByRole("listitem")).toHaveCount(2);
    await play(page, 60_000, 160);

    await button(page, "Save as replay").click();
    await page.getByRole("tab", { name: "Assets" }).click();
    await expect(page.getByRole("tree", { name: "Assets" }).getByRole("treeitem", { name: /\.replay/ })).toBeVisible();
    const replays = readdirSync(join(served.root, "assets/replays")).filter((file) => file.endsWith(".replay"));
    expect(replays).toHaveLength(1);
    // Exits with an error, which throws, unless every recorded hash matches.
    expect(cli(served.root, ["verify", join(served.root, "assets/replays", replays[0]!)])).toContain("hashes match");
    expect(errors).toEqual([]);
  });
});

test.describe("a replay recorded with other types", () => {
  let served: ServedProject;

  test.beforeAll(async () => {
    served = await serveBoxes(4320, (root) => {
      copyFileSync(join(repository, "examples/boxes/assets/boxes.scene.ron"), join(root, "scenes/boxes.scene.ron"));
      copyFileSync(join(DIVERGENCE_FIXTURES, "broken-input.replay"), join(root, "assets/broken-input.replay"));
    });
  });

  test.afterAll(() => served?.stop());

  test("replay_from_changed_types_is_explained", async ({ page }) => {
    await page.goto(served.url);
    await openScene(page, "boxes.scene.ron");
    await waitForGame(page);

    await openScene(page, "broken-input.replay");
    await page.getByRole("tab", { name: "Scene" }).click();
    await expect(page.getByRole("alert").filter({ hasText: "Cannot open" })).toContainText(
      "Cannot open assets/broken-input.replay: cannot decode the input of tick 5 of the replay",
    );
    // The scene stays open.
    await page.getByRole("tab", { name: "Assets" }).click();
    await expect(page.getByRole("tree", { name: "Assets" }).getByRole("treeitem", { name: /boxes\.scene\.ron/ })).toContainText("(open)");
  });
});
