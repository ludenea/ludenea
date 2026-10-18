// Decorations and columns are registered on the shell's `Editor` and rendered by the built-in
// inspector and explorer, so their rendering is tested here, next to those panels.
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
});
