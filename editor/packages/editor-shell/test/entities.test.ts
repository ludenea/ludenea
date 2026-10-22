import { describe, expect, expectTypeOf, it, vi } from "vitest";
import { logged, settle, testGame, testHost, timeline } from "./support";

type Pose = { position: { x: number; y: number }; rotation: { cos: number; sin: number } };

declare module "../src/entities" {
  interface ComponentTypes {
    "ludenea::Pose": Pose;
  }
}

const entity = { index: 4, generation: 2 };
const pose: Pose = { position: { x: 1, y: 2 }, rotation: { cos: 1, sin: 0 } };
/** What was sent besides the timeline refresh every new game gets. */
const own = (sent: { type: string }[]) => sent.filter((command) => command.type !== "get_timeline" && command.type !== "get_hash");

describe("entities", () => {
  it("entities_get_returns_typed_value_from_get_entity", async () => {
    const { editor, setControl } = await testHost();
    const { control, sent } = await testGame((command) =>
      command.type === "get_entity"
        ? { type: "entity", entity, components: { "ludenea::Pose": { type: "json", value: pose } } }
        : undefined,
    );
    setControl(control);

    const value = await editor.entities.get(entity, "ludenea::Pose");

    expectTypeOf(value).toEqualTypeOf<Pose | undefined>();
    expect(value).toEqual(pose);
    expect(own(sent)).toEqual([{ type: "get_entity", entity }]);
  });

  it("entities_set_sends_set_field_with_path", async () => {
    const { editor, setControl } = await testHost();
    const { control, sent } = await testGame(() => ({ type: "done" }));
    setControl(control);

    await editor.entities.set(entity, "ludenea::Pose", [{ type: "field", name: "position" }, { type: "field", name: "x" }], 5);

    expect(own(sent)).toEqual([
      {
        type: "set_field",
        target: { type: "component", entity, component: "ludenea::Pose" },
        path: [{ type: "field", name: "position" }, { type: "field", name: "x" }],
        value: 5,
      },
    ]);
  });

  it("entities_set_is_an_edit_its_listeners_hear", async () => {
    const { editor, setControl } = await testHost();
    const { control } = await testGame((command) => (command.type === "get_timeline" ? timeline() : { type: "done" }));
    setControl(control);
    const edited = vi.fn();
    editor.onEdited(edited);
    const path = [{ type: "field" as const, name: "position" }, { type: "field" as const, name: "x" }];

    await editor.entities.set(entity, "ludenea::Pose", path, 5);

    expect(edited).toHaveBeenCalledExactlyOnceWith({ type: "set_field", target: { type: "component", entity, component: "ludenea::Pose" }, path, value: 5 });
  });

  it("entities_set_is_refused_while_the_game_resimulates", async () => {
    const { editor, setControl } = await testHost();
    const { control, sent } = await testGame((command) => (command.type === "get_timeline" ? timeline() : { type: "done" }));
    setControl(control);
    const { type: _, ...shown } = timeline();
    editor.timeline.setState({ timeline: { ...shown, resimulating: true } });

    await expect(editor.entities.set(entity, "ludenea::Pose", [], pose)).rejects.toThrow("the game is resimulating; edit once it finishes");
    expect(own(sent)).toEqual([]);
  });

  it("entities_watch_follows_one_entity_across_games", async () => {
    const { editor, setControl } = await testHost();
    const first = await testGame(() => ({ type: "done" }));
    setControl(first.control);
    const listener = vi.fn();
    const watch = editor.entities.watch(entity, listener);
    first.push({ type: "entity_changed", entity: { index: 9, generation: 0 }, components: {}, removed: [] });
    first.push({
      type: "entity_changed",
      entity,
      components: { "ludenea::Pose": { type: "json", value: pose }, "my_game::Cache": { type: "ron", text: "{}" } },
      removed: ["ludenea::Body"],
    });
    expect(listener).toHaveBeenCalledExactlyOnceWith({ "ludenea::Pose": pose }, ["ludenea::Body"]);

    const second = await testGame(() => ({ type: "done" }));
    setControl(second.control);
    watch.dispose();

    expect(own(first.sent)).toEqual([{ type: "subscribe", topic: { type: "entity", entity } }]);
    expect(own(second.sent)).toEqual([
      { type: "subscribe", topic: { type: "entity", entity } },
      { type: "unsubscribe", topic: { type: "entity", entity } },
    ]);
  });

  it("entities_watch_tells_its_watchers_when_the_entity_is_despawned", async () => {
    const { editor, setControl } = await testHost();
    const game = await testGame(() => ({ type: "done" }));
    setControl(game.control);
    const changed = vi.fn();
    const despawned = vi.fn();
    const silent = vi.fn();
    editor.entities.watch(entity, changed, despawned);
    // A watcher without a despawn listener hears nothing of it.
    editor.entities.watch(entity, silent);
    game.push({ type: "entity_despawned", entity: { index: 9, generation: 0 } });
    expect(despawned).not.toHaveBeenCalled();

    game.push({ type: "entity_despawned", entity });
    expect(despawned).toHaveBeenCalledExactlyOnceWith();
    expect(changed).not.toHaveBeenCalled();
    expect(silent).not.toHaveBeenCalled();

    // Alive again (a redo of its spawn), it reports its components as changed.
    game.push({ type: "entity_changed", entity, components: { "ludenea::Pose": { type: "json", value: pose } }, removed: [] });
    expect(changed).toHaveBeenCalledExactlyOnceWith({ "ludenea::Pose": pose }, []);
  });

  it("setting_the_same_game_again_changes_nothing", async () => {
    const { editor, setControl } = await testHost();
    const game = await testGame(() => ({ type: "done" }));
    setControl(game.control);
    const changed = vi.fn();
    editor.onGameChanged(changed);
    editor.entities.watch(entity, () => {});

    setControl(game.control);

    expect(changed).not.toHaveBeenCalled();
    expect(own(game.sent)).toEqual([{ type: "subscribe", topic: { type: "entity", entity } }]);
  });

  it("a_watch_the_game_refuses_is_reported_in_the_editor_console", async () => {
    const host = await testHost();
    const game = await testGame((command) => (command.type === "subscribe" ? { error: { code: "invalid_command", message: "no such entity" } } : { type: "done" }));
    host.setControl(game.control);

    host.editor.entities.watch(entity, () => {});
    await settle();

    expect(logged(host)).toEqual([["error", "Watching entity 4v2 failed: no such entity", undefined]]);
  });

  it("an_unwatch_the_game_refuses_is_reported_in_the_editor_console", async () => {
    const host = await testHost();
    const game = await testGame((command) => (command.type === "unsubscribe" ? { error: { code: "invalid_command", message: "not subscribed" } } : { type: "done" }));
    host.setControl(game.control);

    host.editor.entities.watch(entity, () => {}).dispose();
    await settle();

    expect(logged(host)).toEqual([["error", "Unwatching entity 4v2 failed: not subscribed", undefined]]);
  });

  it("entities_need_a_game", async () => {
    const { editor } = await testHost();
    await expect(editor.entities.get(entity, "ludenea::Pose")).rejects.toThrow("no game");
  });
});
