import { describe, expect, it } from "vitest";
import { testGame, testHost } from "./support";

const timeline = { head: 5, simulated: 5, current: 5, checkpoints: [0], edit_ticks: [], can_undo: false, can_redo: false, scene_dirty: false, resimulating: false };
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("timeline store", () => {
  it("store_resets_on_game_change_and_reads_pending_from_hash_replies", async () => {
    const host = await testHost();
    const answer = (command: { type: string }) => {
      if (command.type === "get_timeline") return { type: "timeline", ...timeline };
      if (command.type === "get_hash") return { type: "hash", tick: 5, hash: "00000000000000aa", pending: true };
      return { type: "done" };
    };
    const first = await testGame(answer);
    host.setControl(first.control);
    await settle();
    const store = host.editor.timeline;
    expect(store.getState()).toMatchObject({ timeline, hash: "00000000000000aa", pending: true });

    first.push({ type: "ticked", tick: 6, hash: "00000000000000bb", pending: false, playing: true, effective_rate: 60 });
    first.push({ type: "diverged", tick: 3, expected: "0000000000000001", got: "0000000000000002" });
    expect(store.getState()).toMatchObject({ hash: "00000000000000bb", pending: false, playing: true, divergence: { tick: 3 } });
    expect(store.getState().timeline?.current).toBe(6);

    // An edit's `Ticked` says it is pending, whenever it arrives.
    first.push({ type: "ticked", tick: 6, hash: "00000000000000cc", pending: true, playing: false, effective_rate: 0 });
    expect(store.getState()).toMatchObject({ hash: "00000000000000cc", pending: true });

    first.push({ type: "paused", reason: { type: "user" } });
    await settle();
    expect(store.getState()).toMatchObject({ playing: false, hash: "00000000000000aa", pending: true });

    const second = await testGame(() => undefined);
    host.setControl(second.control);
    expect(store.getState()).toMatchObject({ timeline: null, hash: null, pending: false, playing: false, divergence: null });
    expect(second.sent.map((command) => command.type)).toEqual(["get_timeline", "get_hash"]);
  });
});
