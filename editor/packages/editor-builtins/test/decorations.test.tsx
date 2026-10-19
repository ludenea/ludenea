// Decorations and columns are registered on the shell's `Editor` and rendered by the built-in
// inspector and explorer, so their rendering is tested here, next to those panels.
import type { ExplorerColumn } from "@ludenea/editor-shell";
import { act, cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
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
  afterEach(cleanup);

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
    expect(section.getAllByRole("img", { name: failure })).toHaveLength(8);
    expect((section.getByLabelText("density") as HTMLInputElement).value).toBe("1");
    expect(reported(host)).toEqual([failure]);
  });

  it("an_explorer_column_that_throws_marks_its_cells_until_it_answers", async () => {
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
  });
});
