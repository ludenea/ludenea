import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, schemas, settle, testEditor, testGame } from "./support";

type Command = { type: string; [key: string]: unknown };

const entity = { index: 4, generation: 1 };
const json = (value: unknown) => ({ type: "json", value });
const collider = { shape: { Circle: { center: { x: 0, y: 0 }, radius: 0.5 } }, density: 1 };
const field = (name: string) => ({ type: "field", name });

async function inspector(answer: (command: Command) => object | undefined = () => undefined) {
  const host = await testEditor(builtinPlugins);
  const game = await testGame(
    (command) => {
      const reply = answer(command);
      if (reply) return reply;
      if (command.type === "get_entity") {
        return { type: "entity", entity, components: { "ludenea::Collider": json(collider), "my_game::Score": json({ points: "18446744073709551615" }) } };
      }
      if (command.type === "get_resources") return { type: "resources", resources: { "ludenea::Time": json({ tick: 3 }) } };
      return { type: "done" };
    },
    {
      components: [
        { name: "ludenea::Collider", schema: schemas.collider },
        { name: "my_game::Score", schema: schemas.score },
      ],
      resources: [{ name: "ludenea::Time", schema: { kind: "struct", name: "ludenea_app::Time", fields: [{ name: "tick", schema: { kind: "u64" } }] } }],
    },
  );
  act(() => host.setControl(game.client));
  act(() => host.editor.selection.getState().select([entity]));
  renderPanel(host, "ludenea.inspector");
  await settle();
  return { host, game };
}

const component = (name: string) => within(screen.getByRole("region", { name }));

function edit(label: string, value: string, inside = component("ludenea::Collider")) {
  const input = inside.getByLabelText(label);
  fireEvent.change(input, { target: { value } });
  fireEvent.blur(input);
  return input;
}

describe("inspector", () => {
  afterEach(cleanup);

  it("editing_newtype_variant_field_sends_set_field_with_path", async () => {
    const { game } = await inspector();
    edit("shape.Circle.radius", "2");
    await settle();

    expect(game.sent).toContainEqual({
      type: "set_field",
      target: { type: "component", entity, component: "ludenea::Collider" },
      path: [field("shape"), field("Circle"), field("radius")],
      value: 2,
    });
  });

  it("large_integer_is_edited_as_text_and_sent_as_string", async () => {
    const { game } = await inspector();
    const input = component("my_game::Score").getByLabelText("points") as HTMLInputElement;
    expect(input.type).toBe("text");
    expect(input.value).toBe("18446744073709551615");

    edit("points", "9007199254740993", component("my_game::Score"));
    await settle();

    expect(game.sent).toContainEqual({
      type: "set_field",
      target: { type: "component", entity, component: "my_game::Score" },
      path: [field("points")],
      value: "9007199254740993",
    });
  });

  it("invalid_edit_message_shows_under_field", async () => {
    const message = "invalid edit of ludenea::Collider at shape.Circle.radius: the radius must be finite and positive";
    await inspector((command) => (command.type === "set_field" ? { error: { code: "invalid_edit", message } } : undefined));
    const input = edit("shape.Circle.radius", "-1");
    await settle();

    expect(input.closest(".ludenea-field")!.textContent).toContain(message);
    expect(component("ludenea::Collider").getByLabelText("density").closest(".ludenea-field")!.textContent).not.toContain(message);
  });

  it("no_such_entity_clears_selection", async () => {
    const message = "entity 4v1 is not alive; query the entities first";
    const { host } = await inspector((command) => (command.type === "set_field" ? { error: { code: "no_such_entity", message } } : undefined));
    edit("density", "3");
    await settle();

    expect(host.editor.selection.getState().entities).toEqual([]);
  });

  it("time_resource_is_read_only", async () => {
    await inspector();
    expect((component("ludenea::Time").getByLabelText("tick") as HTMLInputElement).readOnly).toBe(true);
  });

  it("add_component_with_invalid_default_opens_value_form", async () => {
    const message = "invalid edit of ludenea::Collider: the radius must be finite and positive";
    const { game } = await inspector((command) => {
      if (command.type === "get_entity") return { type: "entity", entity, components: {} };
      if (command.type === "insert_component" && (command.value as typeof collider).shape.Circle.radius <= 0) {
        return { error: { code: "invalid_edit", message } };
      }
      return undefined;
    });
    await act(() => screen.getByRole("button", { name: "Add component" }).click());
    fireEvent.change(screen.getByRole("combobox", { name: "Component" }), { target: { value: "ludenea::Collider" } });
    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    await settle();
    expect(screen.getByRole("form", { name: "New ludenea::Collider" }).textContent).toContain(message);

    edit("shape.Circle.radius", "1", within(screen.getByRole("form", { name: "New ludenea::Collider" })));
    // The message was about the previous value.
    expect(screen.getByRole("form", { name: "New ludenea::Collider" }).textContent).not.toContain(message);
    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    await settle();
    expect(game.sent).toContainEqual({
      type: "insert_component",
      entity,
      component: "ludenea::Collider",
      value: { shape: { Circle: { center: { x: 0, y: 0 }, radius: 1 } }, density: 0 },
    });
    expect(screen.queryByRole("form", { name: "New ludenea::Collider" })).toBeNull();
  });

  it("custom_inspector_replaces_generic", async () => {
    const host = await testEditor(builtinPlugins);
    const Custom = ({ value }: { value: { density: number } }) => <p>custom density {value.density}</p>;
    // The editor build knows no component types; a project build would type this registration.
    host.editor.inspectors.register("ludenea::Collider" as never, Custom as never);
    const game = await testGame(
      (command) => {
        if (command.type === "get_entity") return { type: "entity", entity, components: { "ludenea::Collider": json(collider) } };
        if (command.type === "get_resources") return { type: "resources", resources: {} };
        return { type: "done" };
      },
      { components: [{ name: "ludenea::Collider", schema: schemas.collider }] },
    );
    act(() => host.setControl(game.client));
    act(() => host.editor.selection.getState().select([entity]));
    renderPanel(host, "ludenea.inspector");
    await settle();

    expect(component("ludenea::Collider").getByText("custom density 1")).toBeTruthy();
    expect(component("ludenea::Collider").queryByLabelText("density")).toBeNull();
  });

  it("add_component_choice_follows_the_components_present", async () => {
    let components: Record<string, unknown> = {};
    const { game } = await inspector((command) => (command.type === "get_entity" ? { type: "entity", entity, components } : undefined));
    await act(() => screen.getByRole("button", { name: "Add component" }).click());
    const picker = () => screen.getByRole("combobox", { name: "Component" }) as HTMLSelectElement;
    fireEvent.change(picker(), { target: { value: "my_game::Score" } });

    // The entity gains the chosen component elsewhere (another panel, an undo).
    components = { "my_game::Score": json({ points: 1 }) };
    act(() => game.push({ type: "entity_changed", entity, components, removed: [] }));
    await settle();
    expect(picker().value).toBe("ludenea::Collider");
    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    await settle();
    expect(game.sent.filter((command) => command.type === "insert_component").map((command) => command.component)).toEqual(["ludenea::Collider"]);
  });

  it("add_component_draft_survives_hiding_the_panel", async () => {
    const { host } = await inspector((command) => {
      if (command.type === "get_entity") return { type: "entity", entity, components: {} };
      if (command.type === "insert_component") return { error: { code: "invalid_edit", message: "the radius must be finite and positive" } };
      return undefined;
    });
    await act(() => screen.getByRole("button", { name: "Add component" }).click());
    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    await settle();
    edit("shape.Circle.radius", "3", within(screen.getByRole("form", { name: "New ludenea::Collider" })));

    // The shell unmounts a panel while its tab is hidden.
    cleanup();
    renderPanel(host, "ludenea.inspector");
    await settle();
    const form = within(screen.getByRole("form", { name: "New ludenea::Collider" }));
    expect((form.getByLabelText("shape.Circle.radius") as HTMLInputElement).value).toBe("3");
  });
});
