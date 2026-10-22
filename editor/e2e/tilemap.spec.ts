import { copyFileSync, cpSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Locator, type Page } from "@playwright/test";
import { button, collectErrors, expectNoErrors, pixelCounts, repository, runCommand, serveProject, waitForGame, type ServedProject } from "./project";

let served: ServedProject;

// The template's project with the fixture's game (a renderer and its `world` layer) and scene
// (an empty 16 by 9 tilemap at the origin), and the swarm example's bug as the tileset.
test.beforeAll(async () => {
  served = await serveProject("tilemap_game", 4325, (root) => {
    rmSync(join(root, "assets"), { recursive: true, force: true });
    mkdirSync(join(root, "assets"));
    for (const file of ["bug.png", "bug.png.meta"]) copyFileSync(join(repository, "examples/swarm/assets", file), join(root, "assets", file));
    cpSync(join(import.meta.dirname, "fixtures/tilemap-project"), root, { recursive: true });
  });
});

test.afterAll(() => served?.stop());

/** Selects the scene's one entity, its tilemap, from the scene tree. */
async function selectTilemap(page: Page): Promise<void> {
  await page.getByRole("tab", { name: "Scene" }).click();
  const entity = page.getByRole("listbox", { name: "Entities" }).getByRole("option");
  await expect(entity).toHaveCount(1);
  await expect(entity).toContainText("Tilemap");
  await entity.click();
}

/** Frames the selected tilemap in the viewport; returns the viewport's box on the page. */
async function frameTilemap(page: Page, viewport: Locator) {
  await page.getByRole("tab", { name: "Viewport" }).click();
  await runCommand(page, "Frame the selection in the viewport");
  return (await viewport.boundingBox())!;
}

/** How many pixels of the page's `clip` the renderer drew over its background. */
async function drawnPixels(page: Page, clip: { x: number; y: number; width: number; height: number }): Promise<number> {
  return (await pixelCounts(page, await page.screenshot({ clip }))).drawn;
}

test("paint_a_rectangle_undo_and_save", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto(served.url);
  await waitForGame(page);
  await selectTilemap(page);

  // The palette shows the tileset's four frames, cut by the renderer in the browser.
  await page.getByRole("tab", { name: "Tile palette" }).click();
  const tiles = page.getByRole("group", { name: "Tiles of assets/bug.png" });
  await expect(tiles.getByRole("button")).toHaveCount(4);
  const viewBoxes = await tiles.locator("svg").evaluateAll((svgs) => svgs.map((svg) => svg.getAttribute("viewBox")));
  expect(viewBoxes).toEqual(["0 0 16 16", "16 0 16 16", "32 0 16 16", "48 0 16 16"]);
  // Each shows the PNG the editor read: 64 by 16 pixels.
  const href = await tiles.locator("image").first().getAttribute("href");
  const size = await page.evaluate(async (url) => {
    const image = new Image();
    image.src = url!;
    await image.decode();
    return [image.naturalWidth, image.naturalHeight];
  }, href);
  expect(size).toEqual([64, 16]);
  const tile = tiles.getByRole("button", { name: "Tile 2" });
  await tile.click();
  await expect(tile).toHaveAttribute("aria-pressed", "true");
  // B, with the focus on the palette, takes the paint tool.
  await page.keyboard.press("b");

  const viewport = page.getByRole("application", { name: "Viewport" });
  const box = await frameTilemap(page, viewport);
  await expect(viewport).toHaveCSS("cursor", "crosshair");
  // The view's center is the tilemap's: a drag from there 60 pixels right and 40 down covers
  // cells all around `inside`, which shows the background until painted.
  const from = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  const to = { x: from.x + 60, y: from.y + 40 };
  const inside = { x: from.x + 5, y: from.y + 5, width: 50, height: 30 };
  const drag = async () => {
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(to.x, to.y, { steps: 5 });
    await page.mouse.up();
  };
  expect(await drawnPixels(page, inside)).toBe(0);
  await drag();
  await expect.poll(() => drawnPixels(page, inside)).toBeGreaterThan(0);

  // One undo takes the whole rectangle back.
  await page.getByRole("tab", { name: "Timeline" }).click();
  await button(page, "Undo").click();
  await expect.poll(() => drawnPixels(page, inside)).toBe(0);

  await drag();
  await expect.poll(() => drawnPixels(page, inside)).toBeGreaterThan(0);
  await runCommand(page, "Save scene");
  const scene = join(served.root, "scenes/main.scene.ron");
  await expect.poll(() => readFileSync(scene, "utf8")).toContain("Some(2)");

  // Opened again, the editor draws the tiles the scene holds.
  await page.reload();
  await waitForGame(page);
  await selectTilemap(page);
  await frameTilemap(page, viewport);
  await expect.poll(() => drawnPixels(page, inside)).toBeGreaterThan(0);
  await expectNoErrors(page, errors);
});
