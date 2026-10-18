import { describe, expect, it } from "vitest";
import { fakeProject, gameWithSession, ids, settle, testGame, testHost, timeline } from "./support";

const session = { scene: "(main)", saved_scene: null, inputs: "AA==", edits: "", undo: "", hashes: ["0000000000000001"], assets: [], document: "Scene" as const, keeps_assets: false, current: 0 };

describe("branch store", () => {
  it("branch_store_follows_timeline_events", async () => {
    const host = await testHost();
    const game = await testGame((command) => (command.type === "delete_branch" ? { type: "deleted", branches: [2, 3] } : { type: "done" }));
    host.setControl(game.control);
    const branches = [
      { id: 0, name: "main", parent: null, fork_tick: 0, head: 9, stale: false },
      { id: 2, name: "Branch 3", parent: 0, fork_tick: 4, head: 9, stale: true },
    ];

    game.push({ type: "timeline_changed", timeline: { ...timeline(4), branch: 2, branches, compare: 0 } });

    expect(host.editor.branches.getState()).toMatchObject({ list: branches, current: 2, compare: 0 });
    const store = host.editor.branches.getState();
    await store.switch(0);
    await store.rename(2, "jump higher");
    await store.setCompare(null);
    expect(await store.remove(2)).toEqual([2, 3]);
    expect(game.sent.slice(-4)).toEqual([
      { type: "switch_branch", branch: 0 },
      { type: "rename_branch", branch: 2, name: "jump higher" },
      { type: "set_compare_branch", branch: null },
      { type: "delete_branch", branch: 2 },
    ]);

    host.setControl(null);
    expect(host.editor.branches.getState()).toMatchObject({ list: [], current: 0, compare: null });
  });

  it("branches_survive_editor_restart", async () => {
    const project = fakeProject();
    const before = await testHost(undefined, project.answer);
    const first = await gameWithSession({ ...session, branches: "AQID" });
    before.setControl(first.control);
    await before.editor.document.getState().open(ids.main);
    // Closing the editor stores the session.
    await before.sessions.store();

    const after = await testHost(undefined, project.answer);
    const second = await gameWithSession({ ...session, branches: "" });
    after.setControl(second.control);
    await after.editor.document.getState().open(ids.main);
    await settle();

    expect(second.sent.find((command) => command.type === "load_session")).toMatchObject({ branches: "AQID" });
  });
});
