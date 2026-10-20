import type { PluginDefinition } from "@ludenea/editor-shell";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { builtinPlugins } from "../src";
import { renderFrame, renderPanel, setSceneDirty, settle, testEditor, testGame } from "./support";

// The physics crate's engine plugin lives outside this package: the project service bundles it
// at runtime. A computed specifier keeps it out of this package's type checking: the engine
// plugins have their own (editor/engine-plugins).
const PHYSICS_PLUGIN: string = new URL("../../../../crates/ludenea-physics/editor/index.tsx", import.meta.url).pathname;

const entity = (index: number) => ({ index, generation: 0 });
const kinds = ["Dynamic", "Static", "Dynamic", "Kinematic", "Dynamic"];

const physicsPlugin = async () => ((await import(/* @vite-ignore */ PHYSICS_PLUGIN)) as { default: PluginDefinition }).default;

describe("physics plugin", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("opening_a_scene_refreshes_the_viewport_and_the_bodies_without_a_tick", async () => {
    // jsdom has no 2D context: the viewport then draws through the renderer only.
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    const scene = "scenes/main.scene.ron";
    const host = await testEditor([...builtinPlugins, await physicsPlugin()], (request) => {
      if (request.type === "list_files") return { type: "files", entries: [{ path: scene, id: "0000000000000000000000000000000a", kind: "Scene" }] };
      if (request.type === "read_text") return { type: "text", text: "(scene)" };
      if (request.type === "info") return undefined;
      return { type: "done" };
    });
    let loaded = false;
    const game = await testGame((command) => {
      if (command.type === "query_entities") {
        const rows = loaded ? kinds.map((_, index) => ({ entity: entity(index), hash: "0000000000000000" })) : [];
        return { type: "entities", total: rows.length, rows };
      }
      if (command.type === "get_entity") {
        const { index } = command.entity as { index: number };
        return { type: "entity", entity: command.entity, components: { "ludenea::Body": { type: "json", value: { kind: kinds[index] } } } };
      }
      if (command.type === "get_render_frame") return renderFrame(0);
      if (command.type === "get_tick_events") return { type: "tick_events", events: [] };
      if (command.type === "load_scene") {
        loaded = true;
        // The host announces the new state at the same tick on its next advance.
        setTimeout(() => game.push({ type: "ticked", tick: 0, hash: "00000000000000aa", pending: false, playing: false, effective_rate: 0 }));
      }
      return { type: "done" };
    });
    act(() => host.setControl(game.client));
    setSceneDirty(host, false);
    renderPanel(host, "ludenea.assets");
    renderPanel(host, "ludenea.viewport");
    renderPanel(host, "ludenea_physics.panel");
    await waitFor(() => expect(screen.getByRole("table", { name: "Bodies" }).textContent).toContain("Dynamic0"));
    await waitFor(() => expect(game.sent.filter((command) => command.type === "get_render_frame").length).toBeGreaterThan(0));
    const drawsBefore = game.sent.filter((command) => command.type === "get_render_frame").length;

    fireEvent.doubleClick(screen.getByText("main.scene.ron"));
    await waitFor(() => expect(screen.getByRole("table", { name: "Bodies" }).textContent).toContain("Dynamic3"));
    await waitFor(() => expect(game.sent.filter((command) => command.type === "get_render_frame").length).toBeGreaterThan(drawsBefore));
    const counts = screen.getByRole("table", { name: "Bodies" }).textContent;
    expect(counts).toContain("Static1");
    expect(counts).toContain("Kinematic1");
    expect(game.sent.filter((command) => command.type === "load_scene")).toHaveLength(1);
    expect(game.sent.filter((command) => command.type === "step")).toEqual([]);
  });

  it("closing_the_physics_panel_keeps_the_explorer_recording", async () => {
    const host = await testEditor([...builtinPlugins, await physicsPlugin()]);
    const game = await testGame((command) => {
      if (command.type === "query_entities") return { type: "entities", total: 0, rows: [] };
      if (command.type === "get_schedule") return { type: "schedule", systems: [] };
      if (command.type === "get_tick_events") return { type: "tick_events", events: [] };
      return { type: "done" };
    });
    act(() => host.setControl(game.client));
    setSceneDirty(host, false);
    const introspection = () => game.sent.filter((command) => (command.topic as { type?: string } | undefined)?.type === "introspection").map((command) => command.type);
    renderPanel(host, "ludenea.explorer");
    const Physics = host.registry.panels.get("ludenea_physics.panel")!.component;
    const physics = render(<Physics editor={host.editor} />);
    fireEvent.click(screen.getByRole("tab", { name: "Schedule" }));
    await settle();
    expect(introspection()).toEqual(["subscribe"]);

    physics.unmount();
    await settle();
    expect(introspection()).toEqual(["subscribe"]);

    fireEvent.click(screen.getByRole("tab", { name: "Entities" }));
    await settle();
    expect(introspection()).toEqual(["subscribe", "unsubscribe"]);
  });

  it("physics_panel_counts_bodies_by_kind", async () => {
    const plugin = await physicsPlugin();
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
        const collision = (kind: string, a: number, b: number) => ({ event_type: "ludenea::Collision", value: { kind, a: entity(a), b: entity(b) } });
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
