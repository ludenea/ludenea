import { describe, expect, expectTypeOf, it, vi } from "vitest";
import { testGame, testHost } from "./support";

type Pose = { position: { x: number; y: number }; rotation: { cos: number; sin: number } };

declare module "../src/entities" {
  interface ComponentTypes {
    "ludenea::Pose": Pose;
  }
}

const entity = { index: 4, generation: 2 };
const pose: Pose = { position: { x: 1, y: 2 }, rotation: { cos: 1, sin: 0 } };

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
    expect(sent).toEqual([{ type: "get_entity", entity }]);
  });

  it("entities_set_sends_set_field_with_path", async () => {
    const { editor, setControl } = await testHost();
    const { control, sent } = await testGame(() => ({ type: "done" }));
    setControl(control);

    await editor.entities.set(entity, "ludenea::Pose", [{ type: "field", name: "position" }, { type: "field", name: "x" }], 5);

    expect(sent).toEqual([
      {
        type: "set_field",
        target: { type: "component", entity, component: "ludenea::Pose" },
        path: [{ type: "field", name: "position" }, { type: "field", name: "x" }],
        value: 5,
      },
    ]);
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

    expect(first.sent).toEqual([{ type: "subscribe", topic: { type: "entity", entity } }]);
    expect(second.sent).toEqual([
      { type: "subscribe", topic: { type: "entity", entity } },
      { type: "unsubscribe", topic: { type: "entity", entity } },
    ]);
  });

  it("entities_need_a_game", async () => {
    const { editor } = await testHost();
    await expect(editor.entities.get(entity, "ludenea::Pose")).rejects.toThrow("no game");
  });
});
