import type { PluginDefinition } from "@ludenea/editor-shell";
import { act, cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { renderPanel, setSceneDirty, testEditor, testGame } from "./support";

// The physics crate's engine plugin lives outside this package: the project service bundles it
// at runtime. A computed specifier keeps it out of this package's type checking.
const PHYSICS_PLUGIN: string = new URL("../../../../crates/ludenea-physics/editor/index.tsx", import.meta.url).pathname;

const entity = (index: number) => ({ index, generation: 0 });
const kinds = ["Dynamic", "Static", "Dynamic", "Kinematic", "Dynamic"];

describe("physics plugin", () => {
  afterEach(cleanup);

  it("physics_panel_counts_bodies_by_kind", async () => {
    const plugin = ((await import(/* @vite-ignore */ PHYSICS_PLUGIN)) as { default: PluginDefinition }).default;
    expect(plugin.name).toBe("ludenea_physics");
    const host = await testEditor([plugin]);
    const game = await testGame((command) => {
      if (command.type === "query_entities") {
        return { type: "entities", total: kinds.length, rows: kinds.map((_, index) => ({ entity: entity(index), hash: "0000000000000000" })) };
      }
      if (command.type === "get_entity") {
        const { index } = command.entity as { index: number };
        return { type: "entity", entity: command.entity, components: { "ludenea::Body": { type: "json", value: { kind: kinds[index] } } } };
      }
      if (command.type === "get_tick_events") {
        const collision = (kind: string, a: number, b: number) => ({ event_type: "ludenea_physics::events::Collision", value: { kind, a: entity(a), b: entity(b) } });
        return { type: "tick_events", events: [collision("Begin", 0, 1), { event_type: "my_game::Scored", value: 1 }, collision("End", 2, 4)] };
      }
      return { type: "done" };
    });
    act(() => host.setControl(game.client));
    setSceneDirty(host, false);
    renderPanel(host, "ludenea_physics.panel");
    // Each body is one more request: wait until all replies are in.
    await waitFor(() => expect(screen.getByRole("list", { name: "Contacts" }).children).toHaveLength(2));

    expect(game.sent).toContainEqual({ type: "query_entities", filter: "With<ludenea::Body>", offset: 0, limit: 1000 });
    expect(game.sent).toContainEqual({ type: "subscribe", topic: { type: "introspection" } });
    const counts = screen.getByRole("table", { name: "Bodies" }).textContent;
    expect(counts).toContain("Static1");
    expect(counts).toContain("Kinematic1");
    expect(counts).toContain("Dynamic3");
    const contacts = screen.getByRole("list", { name: "Contacts" });
    expect(contacts.textContent).toContain("Begin 0v0 and 1v0");
    expect(contacts.textContent).toContain("End 2v0 and 4v0");
  });
});
