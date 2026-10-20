// Decorations and columns are registered on the shell's `Editor` and rendered by the built-in
// inspector and explorer, so their rendering is tested here, next to those panels.
import { definePlugin, type ExplorerColumn } from "@ludenea/editor-shell";
import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, schemas, settle, testEditor, testGame } from "./support";

const entity = { index: 4, generation: 1 };
const collider = { shape: { Circle: { center: { x: 0, y: 0 }, radius: 0.5 } }, density: 1 };

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 400 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 300 });
});

/** The failures the editor console holds. */
const reported = (host: Awaited<ReturnType<typeof testEditor>>) => host.editor.console.list().map(({ text }) => text);

describe("decorations and columns", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("decorations_render_beside_fields", async () => {
    const host = await testEditor(builtinPlugins);
    const game = await testGame(
      (command) =>
        command.type === "get_entity"
          ? { type: "entity", entity, components: { "ludenea::Collider": { type: "json", value: collider } } }
          : command.type === "get_resources"
            ? { type: "resources", resources: {} }
            : { type: "done" },
      { components: [{ name: "ludenea::Collider", schema: schemas.collider }] },
    );
    act(() => host.setControl(game.client));
    act(() => host.editor.selection.getState().select([entity]));
    renderPanel(host, "ludenea.inspector");
    await settle();

    act(() => {
      host.editor.inspectors.decorate({
        id: "test.overridden",
        render: ({ entity, component, path }) =>
          path.at(-1)?.type === "field" && (path.at(-1) as { name: string }).name === "density" ? (
            <span aria-label="overridden">
              {component} {entity.index} {path.length}
            </span>
          ) : null,
      });
    });

    const section = within(screen.getByRole("region", { name: "ludenea::Collider" }));
    expect(section.getByLabelText("overridden").textContent).toBe("ludenea::Collider 4 1");
    // Beside the field it marks: in the same row as its input.
    expect(section.getByLabelText("density").closest(".ludenea-decorated")).toBe(section.getByLabelText("overridden").closest(".ludenea-decorated"));
  });

  it("explorer_columns_render", async () => {
    const host = await testEditor(builtinPlugins);
    const game = await testGame((command) =>
      command.type === "query_entities" ? { type: "entities", total: 2, rows: [0, 1].map((index) => ({ entity: { index, generation: 0 }, hash: "0000000000000000" })) } : { type: "done" },
    );
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.explorer");
    await settle();

    act(() => {
      host.editor.explorer.columns.register({ id: "test.prefab", title: "Prefab", width: 80, value: (entity) => (entity.index === 1 ? "ball" : null) });
    });
    await settle();

    expect(screen.getByRole("columnheader", { name: "Prefab" })).toBeTruthy();
    const rows = screen.getAllByRole("option");
    expect(within(rows[1]!).getByText("ball")).toBeTruthy();
    expect(within(rows[0]!).queryByText("ball")).toBeNull();
  });

  it("a_decoration_that_throws_marks_the_fields_and_is_reported_once", async () => {
    const devtools = vi.spyOn(console, "error");
    const host = await testEditor(builtinPlugins);
    const game = await testGame(
      (command) => (command.type === "get_entity" ? { type: "entity", entity, components: { "ludenea::Collider": { type: "json", value: collider } } } : { type: "done" }),
      { components: [{ name: "ludenea::Collider", schema: schemas.collider }] },
    );
    act(() => host.setControl(game.client));
    act(() => host.editor.selection.getState().select([entity]));
    renderPanel(host, "ludenea.inspector");
    await settle();

    act(() => {
      host.editor.inspectors.decorate({
        id: "test.broken",
        render: () => {
          throw new Error("the prefab state is not loaded");
        },
      });
    });

    const failure = 'The "test.broken" decoration of plugin "editor" failed: the prefab state is not loaded';
    const section = within(screen.getByRole("region", { name: "ludenea::Collider" }));
    // The component itself, its shape, the circle, its center, x, y, radius, and density.
    expect(section.getAllByRole("button", { name: failure })).toHaveLength(8);
    expect((section.getByLabelText("density") as HTMLInputElement).value).toBe("1");
    expect(reported(host)).toEqual([failure]);
    expect(devtools).not.toHaveBeenCalled();
  });

  it("a_decoration_that_threw_renders_again_for_another_entity_or_a_changed_component", async () => {
    const devtools = vi.spyOn(console, "error");
    let density = 1;
    const value = () => ({ "ludenea::Collider": { type: "json", value: { ...collider, density } } });
    const host = await testEditor(builtinPlugins);
    const game = await testGame((command) => (command.type === "get_entity" ? { type: "entity", entity: command.entity, components: value() } : { type: "done" }), {
      components: [{ name: "ludenea::Collider", schema: schemas.collider }],
    });
    act(() => host.setControl(game.client));
    act(() => host.editor.selection.getState().select([entity]));
    renderPanel(host, "ludenea.inspector");
    await settle();

    let loaded = false;
    let mounts = 0;
    function Counted() {
      useEffect(() => void (mounts += 1), []);
      return null;
    }
    act(() => {
      host.editor.inspectors.decorate({ id: "test.counted", render: () => <Counted /> });
      host.editor.inspectors.decorate({
        id: "test.prefab",
        render: ({ entity, path }) => {
          if (entity.index === 4 && !loaded) throw new Error("the prefab state is not loaded");
          return path.length === 0 ? <span aria-label="prefab state">{entity.index}</span> : null;
        },
      });
    });
    const failure = 'The "test.prefab" decoration of plugin "editor" failed: the prefab state is not loaded';
    const section = () => within(screen.getByRole("region", { name: "ludenea::Collider" }));
    expect(section().getAllByRole("button", { name: failure })).toHaveLength(8);

    // Another entity: the decoration renders again.
    act(() => host.editor.selection.getState().select([{ index: 5, generation: 1 }]));
    await settle();
    expect(section().queryByRole("button", { name: failure })).toBeNull();
    expect(section().getByLabelText("prefab state").textContent).toBe("5");

    // Back on the first, which still fails: marked again, not reported twice.
    act(() => host.editor.selection.getState().select([entity]));
    await settle();
    expect(section().getAllByRole("button", { name: failure })).toHaveLength(8);

    // The state loads, then the component changes: every field's decoration renders again, and
    // the decorations that did not fail keep their state.
    const counted = mounts;
    loaded = true;
    density = 2;
    act(() => game.push({ type: "entity_changed", entity, components: value(), removed: [] }));
    await settle();
    expect((section().getByLabelText("density") as HTMLInputElement).value).toBe("2");
    expect(section().queryByRole("button", { name: failure })).toBeNull();
    expect(section().getByLabelText("prefab state").textContent).toBe("4");
    expect(mounts).toBe(counted);
    expect(reported(host)).toEqual([failure]);
    expect(devtools).not.toHaveBeenCalled();
  });

  it("clicking_a_failed_decoration_s_mark_renders_it_again", async () => {
    const host = await testEditor(builtinPlugins);
    const game = await testGame(
      (command) => (command.type === "get_entity" ? { type: "entity", entity, components: { "ludenea::Collider": { type: "json", value: collider } } } : { type: "done" }),
      { components: [{ name: "ludenea::Collider", schema: schemas.collider }] },
    );
    act(() => host.setControl(game.client));
    act(() => host.editor.selection.getState().select([entity]));
    renderPanel(host, "ludenea.inspector");
    await settle();
    let loaded = false;
    act(() => {
      host.editor.inspectors.decorate({
        id: "test.prefab",
        render: ({ path }) => {
          if (!loaded) throw new Error("the prefab state is not loaded");
          return path.length === 0 ? <span aria-label="prefab state">loaded</span> : null;
        },
      });
    });
    const failure = 'The "test.prefab" decoration of plugin "editor" failed: the prefab state is not loaded';
    const section = () => within(screen.getByRole("region", { name: "ludenea::Collider" }));

    // The state loads while neither the entity nor the component changes (a paused game).
    loaded = true;
    for (const mark of section().getAllByRole("button", { name: failure })) fireEvent.click(mark);

    expect(section().getByLabelText("prefab state").textContent).toBe("loaded");
    expect(section().queryByRole("button", { name: failure })).toBeNull();
    expect(reported(host)).toEqual([failure]);
  });

  it("an_explorer_column_that_throws_marks_its_cells_until_it_answers", async () => {
    const devtools = vi.spyOn(console, "error");
    const host = await testEditor(builtinPlugins);
    const game = await testGame((command) =>
      command.type === "query_entities" ? { type: "entities", total: 2, rows: [0, 1].map((index) => ({ entity: { index, generation: 0 }, hash: "0000000000000000" })) } : { type: "done" },
    );
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.explorer");
    await settle();

    let loaded = false;
    const listeners = new Set<() => void>();
    const prefab: ExplorerColumn = {
      id: "test.prefab",
      title: "Prefab",
      width: 80,
      value(entity) {
        if (entity.index === 1 && !loaded) throw new Error("the prefab list is not loaded");
        return "ball";
      },
      subscribe(listener) {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    };
    act(() => void host.editor.explorer.columns.register(prefab));
    await settle();

    const failure = 'The "Prefab" explorer column of plugin "editor" failed: the prefab list is not loaded';
    const rows = () => screen.getAllByRole("option");
    expect(within(rows()[0]!).getByText("ball")).toBeTruthy();
    expect(within(rows()[1]!).getByRole("img", { name: failure })).toBeTruthy();
    expect(reported(host)).toEqual([failure]);

    // The column says its values changed: the cell reads it again.
    loaded = true;
    act(() => listeners.forEach((listener) => listener()));
    expect(within(rows()[1]!).getByText("ball")).toBeTruthy();
    expect(within(rows()[1]!).queryByRole("img")).toBeNull();
    expect(reported(host)).toHaveLength(1);
    expect(devtools).not.toHaveBeenCalled();
  });

  it("a_decoration_registered_again_renders_without_its_mark", async () => {
    const host = await testEditor(builtinPlugins);
    const game = await testGame(
      (command) => (command.type === "get_entity" ? { type: "entity", entity, components: { "ludenea::Collider": { type: "json", value: collider } } } : { type: "done" }),
      { components: [{ name: "ludenea::Collider", schema: schemas.collider }] },
    );
    act(() => host.setControl(game.client));
    act(() => host.editor.selection.getState().select([entity]));
    renderPanel(host, "ludenea.inspector");
    await settle();
    let loaded = false;
    const decoration = {
      id: "test.prefab",
      render: ({ path }: { path: unknown[] }) => {
        if (!loaded) throw new Error("the prefab state is not loaded");
        return path.length === 0 ? <span aria-label="prefab state">loaded</span> : null;
      },
    };
    let registration!: { dispose(): void };
    act(() => void (registration = host.editor.inspectors.decorate(decoration)));
    const failure = 'The "test.prefab" decoration of plugin "editor" failed: the prefab state is not loaded';
    const section = () => within(screen.getByRole("region", { name: "ludenea::Collider" }));
    expect(section().getAllByRole("button", { name: failure })).toHaveLength(8);

    // Its plugin reloads with the state it lacked: the new registration starts unmarked.
    loaded = true;
    act(() => {
      registration.dispose();
      host.editor.inspectors.decorate({ ...decoration });
    });

    expect(section().queryByRole("button", { name: failure })).toBeNull();
    expect(section().getByLabelText("prefab state").textContent).toBe("loaded");
  });

  it("an_explorer_column_registered_again_reads_its_cells_without_their_mark", async () => {
    const host = await testEditor(builtinPlugins);
    const game = await testGame((command) =>
      command.type === "query_entities" ? { type: "entities", total: 2, rows: [0, 1].map((index) => ({ entity: { index, generation: 0 }, hash: "0000000000000000" })) } : { type: "done" },
    );
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.explorer");
    await settle();
    let loaded = false;
    const prefab: ExplorerColumn = {
      id: "test.prefab",
      title: "Prefab",
      width: 80,
      value(entity) {
        if (entity.index === 1 && !loaded) throw new Error("the prefab list is not loaded");
        return "ball";
      },
    };
    let registration!: { dispose(): void };
    act(() => void (registration = host.editor.explorer.columns.register(prefab)));
    await settle();
    const failure = 'The "Prefab" explorer column of plugin "editor" failed: the prefab list is not loaded';
    const rows = () => screen.getAllByRole("option");
    expect(within(rows()[1]!).getByRole("img", { name: failure })).toBeTruthy();

    // Without a subscription to say it changed, a new registration is what reads it again.
    loaded = true;
    act(() => {
      registration.dispose();
      host.editor.explorer.columns.register({ ...prefab });
    });
    await settle();

    expect(within(rows()[1]!).getByText("ball")).toBeTruthy();
    expect(within(rows()[1]!).queryByRole("img")).toBeNull();
  });

  it("an_explorer_column_whose_subscription_throws_costs_only_that_subscription", async () => {
    const devtools = vi.spyOn(console, "error");
    const listeners = new Set<() => void>();
    const column = (title: string, subscribe: ExplorerColumn["subscribe"]): ExplorerColumn => ({ id: `my_game.${title}`, title, width: 80, value: () => title.toLowerCase(), subscribe });
    const plugin = definePlugin({
      name: "my_game",
      activate(editor) {
        editor.explorer.columns.register(
          column("Unloaded", () => {
            throw new Error("the prefab list is not loaded");
          }),
        );
        editor.explorer.columns.register(
          column("Healthy", (listener) => {
            listeners.add(listener);
            return () => listeners.delete(listener);
          }),
        );
        editor.explorer.columns.register(
          column("Sticky", () => () => {
            throw new Error("the listener is already gone");
          }),
        );
      },
    });
    const host = await testEditor([...builtinPlugins, plugin]);
    const game = await testGame((command) =>
      command.type === "query_entities" ? { type: "entities", total: 2, rows: [0, 1].map((index) => ({ entity: { index, generation: 0 }, hash: "0000000000000000" })) } : { type: "done" },
    );
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.explorer");
    await settle();

    const subscribing = 'Subscribing to the "Unloaded" explorer column of plugin "my_game" failed: the prefab list is not loaded';
    for (const title of ["Unloaded", "Healthy", "Sticky"]) expect(screen.getAllByText(title.toLowerCase())).toHaveLength(2);
    expect(listeners.size).toBe(1);
    expect(reported(host)).toEqual([subscribing]);

    // Another column: the explorer subscribes again, and each failure is reported once.
    act(() => void host.editor.explorer.columns.register({ id: "test.other", title: "Other", width: 80, value: () => null }));
    await settle();
    const unsubscribing = 'Unsubscribing from the "Sticky" explorer column of plugin "my_game" failed: the listener is already gone';
    expect(listeners.size).toBe(1);
    expect(reported(host)).toEqual([subscribing, unsubscribing]);

    cleanup();
    expect(listeners.size).toBe(0);
    expect(reported(host)).toHaveLength(2);
    expect(devtools).not.toHaveBeenCalled();
  });
});
