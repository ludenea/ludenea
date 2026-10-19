import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { button, cli, collectErrors, expectNoErrors, expectTick, openScene, play, seek, serveProject, settledTimeline, waitForGame, type ServedProject } from "./project";
import { decodeTrace } from "./trace";

interface ExportedSession {
  scene: string;
  inputs: string;
  edits: string;
  hashes: string[];
}

declare global {
  interface Window {
    /** Sends `ExportSession` to the running game; set by the session plugin below. */
    exportSession(): Promise<ExportedSession>;
  }
}

// Gives the test the session the editor recorded; nothing else in the test goes around the UI.
const SESSION_PLUGIN = `import { definePlugin } from "@ludenea/editor-shell";

export default definePlugin({
  name: "session_game",
  activate(editor) {
    window.exportSession = () => editor.control.request({ type: "export_session" });
  },
});
`;

let served: ServedProject;

test.beforeAll(async () => {
  served = await serveProject("session_game", 4312, (root) => writeFileSync(join(root, "editor/index.ts"), SESSION_PLUGIN));
});

test.afterAll(() => served?.stop());

test("an editor session replays in the CLI", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto(served.url);
  await openScene(page, "main.scene.ron");
  await waitForGame(page);
  await expectTick(page, 0, 0);

  const played = await play(page, 2000);
  expect(played).toBeGreaterThan(60);
  const parentHead = (await settledTimeline(page)).head;
  const editTick = played - 60;
  await seek(page, editTick);

  // A falling box, moved sideways in the inspector at the past tick.
  await page.getByRole("tab", { name: "Scene" }).click();
  await page.getByRole("listbox", { name: "Entities" }).getByRole("option").nth(1).click();
  await page.getByRole("tab", { name: "Inspector" }).click();
  const x = page.getByRole("region", { name: "ludenea::Pose" }).getByLabel("position.x");
  await x.fill("-2.5");
  await x.press("Enter");
  await page.getByRole("tab", { name: "Timeline" }).click();
  await expect(page.getByLabel(`Edit at tick ${editTick}`, { exact: true })).toBeAttached();
  // The edit forked a branch at that tick, with no future yet.
  await expectTick(page, editTick, editTick);

  // Record a future after the edit, then move around in it.
  const after = await play(page, 1000);
  expect(after).toBeGreaterThan(editTick + 30);
  await seek(page, editTick);
  await seek(page, editTick + 30);
  await seek(page, editTick);
  await play(page, 1000);

  // Undo removes the branch: back on the parent, whose recorded future is intact.
  await button(page, "Undo").click();
  await expectTick(page, editTick, parentHead);
  await expect(page.getByLabel(`Edit at tick ${editTick}`, { exact: true })).not.toBeAttached();
  await button(page, "Redo").click();
  await expect(page.getByLabel(`Edit at tick ${editTick}`, { exact: true })).toBeAttached();
  // Simulate past the redone edit, so the hashes cover its effect.
  await play(page, 1000);

  const session = await page.evaluate(() => window.exportSession());
  const files = mkdtempSync(join(tmpdir(), "ludenea-session-"));
  try {
    const scene = join(files, "session.scene.ron");
    const inputs = join(files, "session.inputs");
    const edits = join(files, "session.edits");
    const trace = join(files, "session.trace");
    writeFileSync(scene, session.scene);
    writeFileSync(inputs, Buffer.from(session.inputs, "base64"));
    writeFileSync(edits, Buffer.from(session.edits, "base64"));

    // `hashes[k]` is the hash after k ticks: every recorded tick is replayed.
    const ticks = session.hashes.length - 1;
    expect(ticks).toBeGreaterThan(editTick);
    // The scene's prefab instances load from the project's assets.
    cli(served.root, ["run", scene, "--project", served.root, "--inputs", inputs, "--edits", edits, "--ticks", String(ticks), "--trace", trace]);
    const replayed = decodeTrace(readFileSync(trace)).hashes.map((hash) => hash.toString(16).padStart(16, "0"));
    expect(replayed).toEqual(session.hashes);

    const verified = cli(served.root, ["verify", scene, "--project", served.root, "--inputs", inputs, "--edits", edits, "--golden", trace, "--dump-dir", join(files, "dump")]);
    expect(verified).toContain(`ok: ${ticks + 1} hashes match`);
  } finally {
    rmSync(files, { recursive: true, force: true });
  }
  await expectNoErrors(page, errors);
});
