import { definePlugin } from "@ludenea/editor-shell";
import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import type { TypeSchema } from "@ludenea/control-client";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
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
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  /** The resource topics subscribed and unsubscribed, in order. */
  const resourceTopics = (game: { sent: Command[] }) =>
    game.sent.filter((command) => (command.type === "subscribe" || command.type === "unsubscribe") && (command.topic as { type: string }).type === "resource").map((command) => `${command.type} ${(command.topic as { name: string }).name}`);
  const time = (tick: number) => ({ type: "resources_changed", resources: { "ludenea::Time": json({ tick }) } });
  const expand = (name: string) => act(() => screen.getByRole("button", { name, expanded: false }).click());

  it("a_resource_follows_the_game_only_while_expanded_even_while_playing", async () => {
    const { host, game } = await inspector();
    // Collapsed: nothing is subscribed, nothing shown.
    expect(resourceTopics(game)).toEqual([]);
    expect(component("ludenea::Time").queryByLabelText("tick")).toBeNull();

    await expand("Time");
    await settle();
    expect(resourceTopics(game)).toEqual(["subscribe ludenea::Time"]);
    act(() => game.push(time(3)));
    expect((component("ludenea::Time").getByLabelText("tick") as HTMLInputElement).value).toBe("3");

    act(() => host.editor.timeline.setState({ playing: true }));
    act(() => game.push(time(9)));
    expect((component("ludenea::Time").getByLabelText("tick") as HTMLInputElement).value).toBe("9");
    expect(resourceTopics(game)).toEqual(["subscribe ludenea::Time"]);

    await act(() => screen.getByRole("button", { name: "Time", expanded: true }).click());
    await settle();
    expect(resourceTopics(game)).toEqual(["subscribe ludenea::Time", "unsubscribe ludenea::Time"]);
    expect(component("ludenea::Time").queryByLabelText("tick")).toBeNull();
  });

  it("an_expanded_resource_stays_expanded_when_the_panel_shows_again", async () => {
    const { host, game } = await inspector();
    await expand("Time");
    await settle();

    // The shell unmounts a panel while its tab is hidden.
    cleanup();
    renderPanel(host, "ludenea.inspector");
    await settle();

    expect(screen.getByRole("button", { name: "Time", expanded: true })).toBeTruthy();
    expect(resourceTopics(game)).toEqual(["subscribe ludenea::Time", "unsubscribe ludenea::Time", "subscribe ludenea::Time"]);
  });

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

  it("a_large_ron_value_shows_its_start_until_asked_for_all", async () => {
    const { game } = await inspector();
    await expand("Time");
    const text = `(pairs: {${"(1, 2): (touching: true), ".repeat(200)}})`;
    act(() => game.push({ type: "resources_changed", resources: { "ludenea::Time": { type: "ron", text } } }));

    const shown = component("ludenea::Time").getByLabelText("ludenea::Time as RON").textContent!;
    expect(shown.length).toBeLessThan(text.length);
    expect(text.startsWith(shown.slice(0, -1))).toBe(true);
    expect(component("ludenea::Time").getByText(/more characters/)).toBeTruthy();

    fireEvent.click(component("ludenea::Time").getByRole("button", { name: "Show all" }));
    expect(component("ludenea::Time").getByLabelText("ludenea::Time as RON").textContent).toBe(text);
  });

  it("time_resource_is_read_only", async () => {
    const { game } = await inspector();
    await expand("Time");
    act(() => game.push(time(3)));
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

  it("a_custom_inspector_that_throws_falls_back_to_the_generic_fields", async () => {
    const devtools = vi.spyOn(console, "error");
    let broken = true;
    const Custom = ({ value }: { value: { density: number } }) => {
      if (broken) throw new Error("value.shape.Box is undefined");
      return <p>custom density {value.density}</p>;
    };
    // The editor build knows no component types; a project build would type this registration.
    const plugin = definePlugin({ name: "my_game", activate: (editor) => void editor.inspectors.register("ludenea::Collider" as never, Custom as never) });
    const host = await testEditor([...builtinPlugins, plugin]);
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

    const failure = 'The "ludenea::Collider" inspector of plugin "my_game" failed: value.shape.Box is undefined';
    expect(component("ludenea::Collider").getByRole("alert").textContent).toContain(failure);
    expect((component("ludenea::Collider").getByLabelText("density") as HTMLInputElement).value).toBe("1");
    expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([["error", failure]]);

    // Rendered again for another entity: still the fields, and no second report.
    act(() => host.editor.selection.getState().select([{ index: 5, generation: 1 }]));
    await settle();
    expect(component("ludenea::Collider").getByLabelText("density")).toBeTruthy();
    expect(host.editor.console.list()).toHaveLength(1);

    broken = false;
    act(() => component("ludenea::Collider").getByRole("button", { name: "Reload inspector" }).click());
    expect(component("ludenea::Collider").getByText("custom density 1")).toBeTruthy();
    expect(component("ludenea::Collider").queryByRole("alert")).toBeNull();
    expect(devtools).not.toHaveBeenCalled();
  });

  describe("asset inspectors", () => {
    const SETTINGS = "0000000000000000000000000000000c";
    const settings: TypeSchema = { kind: "struct", name: "demo::Settings", fields: [{ name: "gravity_scale", schema: { kind: "f32" } }] };

    async function dataAsset(inspector: (props: { id: string; value: unknown; onChange(path: unknown[], value: unknown): void }) => ReactNode) {
      const plugin = definePlugin({ name: "my_game", activate: (editor) => void editor.inspectors.registerAsset("demo::Settings", inspector) });
      const host = await testEditor([...builtinPlugins, plugin]);
      const game = await testGame(
        (command) => {
          if (command.type === "get_asset") return { type: "asset", asset: "demo::Settings", value: json({ gravity_scale: 0.5 }) };
          if (command.type === "get_resources") return { type: "resources", resources: {} };
          return { type: "done" };
        },
        { assets: [{ name: "demo::Settings", schema: settings }] },
      );
      act(() => host.setControl(game.client));
      act(() => host.editor.selection.getState().selectAsset(SETTINGS, "Data"));
      renderPanel(host, "ludenea.inspector");
      await settle();
      return { host, game };
    }

    it("an_asset_inspector_renders_below_the_generic_fields", async () => {
      const seen: { id: string; value: unknown }[] = [];
      const { game } = await dataAsset(({ id, value, onChange }) => {
        seen.push({ id, value });
        return <button onClick={() => onChange([field("gravity_scale")], 2)}>Double gravity</button>;
      });

      const section = component("demo::Settings");
      const generic = section.getByLabelText("gravity_scale");
      const custom = section.getByRole("button", { name: "Double gravity" });
      expect(generic.compareDocumentPosition(custom) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(seen.at(-1)).toEqual({ id: SETTINGS, value: { gravity_scale: 0.5 } });

      fireEvent.click(custom);
      await settle();
      expect(game.sent).toContainEqual({ type: "set_asset_field", id: SETTINGS, path: [field("gravity_scale")], value: 2 });
    });

    it("an_asset_inspector_that_throws_leaves_the_generic_fields", async () => {
      vi.spyOn(console, "error").mockImplementation(() => {});
      const { host } = await dataAsset(() => {
        throw new Error("value.clips is undefined");
      });

      const failure = 'The "demo::Settings" asset inspector of plugin "my_game" failed: value.clips is undefined';
      expect(component("demo::Settings").getByRole("alert").textContent).toContain(failure);
      expect(component("demo::Settings").getByLabelText("gravity_scale")).toBeTruthy();
      expect(host.editor.console.list().map(({ text }) => text)).toEqual([failure]);
    });
  });

  describe("texture inspectors", () => {
    const HERO = "0000000000000000000000000000000d";
    const withoutTextures = builtinPlugins.filter((plugin) => plugin.name !== "ludenea.textures");

    async function texture(plugins = withoutTextures) {
      const host = await testEditor(plugins);
      act(() => host.editor.selection.getState().selectAsset(HERO, "Texture"));
      renderPanel(host, "ludenea.inspector");
      await settle();
      return host;
    }

    it("a_texture_inspector_changing_a_value_is_reported_naming_its_plugin", async () => {
      const Changing = ({ onChange }: { onChange(path: unknown[], value: unknown): void }) => <button onClick={() => onChange([], 1)}>Change</button>;
      const host = await texture([...withoutTextures, definePlugin({ name: "my_game", activate: (editor) => void editor.inspectors.registerAsset("Texture", Changing) })]);

      fireEvent.click(screen.getByRole("button", { name: "Change" }));
      expect(host.editor.console.list().map(({ level, text }) => [level, text])).toEqual([
        ["error", 'The "Texture" asset inspector of plugin "my_game" failed: a texture has no value to change: write its PNG or its meta through `editor.project`'],
      ]);
    });
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

  /** Selects `entities`, each holding the components `held` gives for its index. */
  async function selected(entities: { index: number; generation: number }[], held: Record<number, Record<string, unknown>>) {
    const found = await inspector((command) => {
      if (command.type !== "get_entity") return undefined;
      const { index } = command.entity as { index: number };
      return { type: "entity", entity: command.entity, components: held[index] };
    });
    act(() => found.host.editor.selection.getState().select(entities));
    await settle();
    return found;
  }

  const second = { index: 5, generation: 1 };
  const third = { index: 6, generation: 1 };

  it("a_multi_selection_shows_the_shared_components_and_edits_them_all_in_one_batch", async () => {
    const { game } = await selected([entity, second, third], {
      4: { "ludenea::Collider": json(collider), "my_game::Score": json({ points: "1" }) },
      5: { "ludenea::Collider": json({ ...collider, density: 2 }), "my_game::Score": json({ points: "1" }) },
      6: { "ludenea::Collider": json(collider) },
    });

    expect(screen.getByRole("heading", { name: /^3 entities selected/ })).toBeTruthy();
    // Entity 6 has no Score: only the Collider is shared.
    expect(screen.queryByRole("region", { name: "my_game::Score" })).toBeNull();
    const row = (label: string) => component("ludenea::Collider").getByLabelText(label).closest(".ludenea-field")!;
    expect(row("density").textContent).toContain("mixed");
    expect(row("shape.Circle.radius").textContent).not.toContain("mixed");
    // Applying to the scene is a command of one entity.
    expect(component("ludenea::Collider").queryByRole("button", { name: "Apply to scene" })).toBeNull();

    edit("density", "3");
    await settle();
    const target = (entity: object) => ({ type: "component", entity, component: "ludenea::Collider" });
    expect(game.sent.filter((command) => command.type === "batch" || command.type === "set_field")).toEqual([
      { type: "batch", commands: [entity, second, third].map((each) => ({ type: "set_field", target: target(each), path: [field("density")], value: 3 })) },
    ]);

    fireEvent.click(component("ludenea::Collider").getByRole("button", { name: "Remove" }));
    await settle();
    expect(game.sent.filter((command) => command.type === "batch").at(-1)).toEqual({
      type: "batch",
      commands: [entity, second, third].map((each) => ({ type: "remove_component", entity: each, component: "ludenea::Collider" })),
    });
  });

  it("a_component_shown_whole_says_when_the_selected_values_differ", async () => {
    const banner = "The selected entities have different values; the first one's is shown.";
    const ron = (text: string) => ({ type: "ron", text });
    // Without a JSON form, and without a schema: each shows as a whole.
    const held = {
      4: { "ludenea::Collider": ron("(density: 1.0)"), "my_game::Unknown": json({ a: 1 }) },
      5: { "ludenea::Collider": ron("(density: 2.0)"), "my_game::Unknown": json({ a: 2 }) },
    };
    await selected([entity, second], held);
    expect(component("ludenea::Collider").getByText(banner)).toBeTruthy();
    expect(component("my_game::Unknown").getByText(banner)).toBeTruthy();
    cleanup();

    await selected([entity, second], { 4: held[4], 5: held[4] });
    expect(component("ludenea::Collider").queryByText(banner)).toBeNull();
    expect(component("my_game::Unknown").queryByText(banner)).toBeNull();
  });

  it("a_change_to_one_selected_entity_reloads_that_entity_only", async () => {
    const held = {
      4: { "ludenea::Collider": json(collider) },
      5: { "ludenea::Collider": json({ ...collider, density: 2 }) },
      6: { "ludenea::Collider": json(collider) },
    };
    const { game } = await selected([entity, second, third], held);
    const row = () => component("ludenea::Collider").getByLabelText("density").closest(".ludenea-field")!;
    expect(row().textContent).toContain("mixed");
    const loadedBefore = game.sent.filter((command) => command.type === "get_entity").length;

    held[5] = { "ludenea::Collider": json(collider) };
    act(() => game.push({ type: "entity_changed", entity: second, components: held[5], removed: [] }));
    await settle();

    expect(game.sent.filter((command) => command.type === "get_entity").slice(loadedBefore)).toEqual([{ type: "get_entity", entity: second }]);
    expect(row().textContent).not.toContain("mixed");
  });

  it("adding_a_component_to_a_multi_selection_inserts_it_where_it_lacks_in_one_batch", async () => {
    const { game } = await selected([entity, second, third], {
      4: { "my_game::Score": json({ points: "1" }) },
      5: {},
      6: {},
    });
    await act(() => screen.getByRole("button", { name: "Add component" }).click());
    fireEvent.change(screen.getByRole("combobox", { name: "Component" }), { target: { value: "my_game::Score" } });
    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    await settle();

    expect(game.sent.filter((command) => command.type === "batch" || command.type === "insert_component")).toEqual([
      { type: "batch", commands: [second, third].map((each) => ({ type: "insert_component", entity: each, component: "my_game::Score", value: { points: 0 } })) },
    ]);
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
