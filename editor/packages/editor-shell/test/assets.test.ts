import { describe, expect, it, vi } from "vitest";
import { ids, testGame, testHost, timeline } from "./support";

const path = [{ type: "field" as const, name: "gravity_scale" }];
/** What was sent besides the timeline refresh every new game and every edit gets. */
const own = (sent: { type: string }[]) => sent.filter((command) => command.type !== "get_timeline" && command.type !== "get_hash");

describe("assets", () => {
  it("assets_set_field_is_an_edit_its_listeners_hear", async () => {
    const { editor, setControl } = await testHost();
    const { control, sent } = await testGame((command) => (command.type === "get_timeline" ? timeline() : { type: "done" }));
    setControl(control);
    const edited = vi.fn();
    editor.onEdited(edited);

    await editor.assets.setField(ids.settings, path, 2);

    const command = { type: "set_asset_field", id: ids.settings, path, value: 2 };
    expect(own(sent)).toEqual([command]);
    expect(edited).toHaveBeenCalledExactlyOnceWith(command);
  });

  it("assets_set_field_is_refused_while_the_game_resimulates", async () => {
    const { editor, setControl } = await testHost();
    const { control, sent } = await testGame((command) => (command.type === "get_timeline" ? timeline() : { type: "done" }));
    setControl(control);
    const { type: _, ...shown } = timeline();
    editor.timeline.setState({ timeline: { ...shown, resimulating: true } });

    await expect(editor.assets.setField(ids.settings, path, 2)).rejects.toThrow("the game is resimulating; edit once it finishes");
    expect(own(sent)).toEqual([]);
  });

  it("assets_set_field_needs_a_game", async () => {
    const { editor } = await testHost();
    await expect(editor.assets.setField(ids.settings, path, 2)).rejects.toThrow("no game is running");
  });
});
