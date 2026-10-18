import type { PathSegment, TypeSchema } from "@ludenea/control-client";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { builtinPlugins } from "../src";
import { Field } from "../src/inspector/fields/Field";
import { renderPanel, settle, testEditor, testGame } from "./support";

// The project template's shape: `scenes/main.scene.ron` holds instances 4 and 5 of
// `assets/ball.prefab.ron`, and instance 5 overrides its entity 0's `position.x`.
const SCENE = "0000000000000000000000000000000a";
const OTHER_SCENE = "0000000000000000000000000000000b";
const BALL = "6c90cde358504084bfb84f6d1d83c6d9";
const STATS = "bef617c9e94d41c4b0e07882849cef18";
const CREATED = "0000000000000000000000000000000c";
const files = [
  { path: "scenes/main.scene.ron", id: SCENE, kind: "Scene" },
  { path: "scenes/other.scene.ron", id: OTHER_SCENE, kind: "Scene" },
  { path: "assets/ball.prefab.ron", id: BALL, kind: "Prefab" },
  { path: "assets/new.prefab.ron", id: "0000000000000000000000000000000d", kind: "Prefab" },
  { path: "assets/settings.ron", id: STATS, kind: { Data: { type: "my_game::Settings" } } },
  { path: "assets/logo.png", id: "0000000000000000000000000000000e", kind: "Other" },
];

const entity = (index: number) => ({ index, generation: 0 });
const field = (name: string): PathSegment => ({ type: "field", name });
const f32 = { kind: "f32" } as const;
const vec2: TypeSchema = { kind: "struct", name: "ludenea_math::Vec2", fields: [{ name: "x", schema: f32 }, { name: "y", schema: f32 }] };
const rotation: TypeSchema = { kind: "struct", name: "ludenea_math::Rot", fields: [{ name: "cos", schema: f32 }, { name: "sin", schema: f32 }] };
const pose: TypeSchema = { kind: "struct", name: "ludenea::Pose", fields: [{ name: "position", schema: vec2 }, { name: "rotation", schema: rotation }] };
const POSE = "ludenea::Pose";

type Override = { entity: number[]; component: string; path: PathSegment[]; stale: boolean };
const override = (...path: string[]): Override => ({ entity: [0], component: POSE, path: path.map(field), stale: false });
const loadAddresses = [0, 1, 2, 3, 4, 5].map((index) => [entity(index), index < 4 ? [index] : [index, 0]]);
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 400 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 300 });
});

/** The editor with every built-in plugin and a game that opened `main.scene.ron` with `warnings`. */
async function editor(options: { warnings?: string[]; refuse?: (scene: string) => string | null; overrides?: Override[] } = {}) {
  // What the host reports of instance 5's overrides; a field edit on its entity adds one.
  const held = { overrides: options.overrides ?? [override("position", "x")] };
  const instances = () => [
    { address: [4], prefab: BALL, entities: [entity(4)], overrides: [] },
    { address: [5], prefab: BALL, entities: [entity(5)], overrides: held.overrides },
  ];
  const project = await testEditor(builtinPlugins, (request) => {
    if (request.type === "list_files") return { type: "files", entries: files };
    if (request.type === "read_text") return { type: "text", text: `(${request.path as string})` };
    if (request.type === "read_asset_texts") return { type: "asset_texts", assets: [], paths: [] };
    if (request.type === "load_stored_session") return { type: "stored_session", bytes: null };
    if (request.type === "create_asset") return { type: "file", path: request.path, id: CREATED, kind: request.kind };
    if (request.type === "info") return undefined;
    return { type: "done" };
  });
  const loaded = () => ({ type: "loaded", warnings: options.warnings ?? [], instances: instances(), load_addresses: loadAddresses });
  const game = await testGame(
    (command) => {
      if (command.type === "load_scene") {
        const message = options.refuse?.(command.scene as string);
        return message ? { error: { code: "invalid_scene", message } } : loaded();
      }
      if (command.type === "get_instances") return { type: "instances", instances: instances(), load_addresses: loadAddresses };
      if (command.type === "set_field") {
        const path = (command.path as PathSegment[]).map((segment) => (segment as { name: string }).name);
        held.overrides = [...held.overrides, override(...path)];
      }
      if (command.type === "get_timeline") return { type: "timeline", head: 0, simulated: 0, current: 0, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, undo_label: null, redo_label: null, scene_dirty: false, resimulating: false, branch: 0, branches: [], compare: null };
      if (command.type === "query_entities") return { type: "entities", total: 6, rows: [0, 1, 2, 3, 4, 5].map((index) => ({ entity: entity(index), hash: "0000000000000000" })) };
      if (command.type === "get_entity") return { type: "entity", entity: command.entity, components: { [POSE]: { type: "json", value: { position: { x: -1.5, y: 3 }, rotation: { cos: 1, sin: 0 } } } } };
      if (command.type === "get_resources") return { type: "resources", resources: {} };
      if (command.type === "create_prefab") return { type: "text", text: "(new prefab)" };
      if (command.type === "apply_to_prefab") return { type: "text", text: "(applied)" };
      if (command.type === "place_instance" || command.type === "replace_with_instance") return { type: "placed", instance: 6 };
      return { type: "done" };
    },
    { components: [{ name: POSE, schema: pose }] },
  );
  act(() => project.setControl(game.client));
  await act(() => project.editor.document.getState().open(SCENE));
  await settle();
  return { host: project, game, held };
}

const sent = (game: { sent: { type: string }[] }, type: string) => game.sent.filter((command) => command.type === type);

async function inspectInstanceEntity(host: Awaited<ReturnType<typeof editor>>["host"]) {
  act(() => host.editor.selection.getState().select([entity(5)]));
  renderPanel(host, "ludenea.inspector");
  await settle();
  return within(screen.getByRole("region", { name: POSE }));
}

describe("prefabs", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("instances_render_as_groups", async () => {
    const { host } = await editor();
    renderPanel(host, "ludenea.scene-tree");
    await settle();

    const groups = screen.getAllByRole("group", { name: "ball.prefab.ron" });
    expect(groups.map((group) => group.textContent)).toEqual(["ball.prefab.ron4v0", "ball.prefab.ron5v0"]);
    fireEvent.click(within(groups[1]!).getByRole("button", { name: "5v0" }));
    expect(host.editor.selection.getState().entities).toEqual([entity(5)]);
  });

  it("override_mark_shows_on_overridden_field_only", async () => {
    const { host } = await editor();
    const section = await inspectInstanceEntity(host);

    expect(section.getByLabelText("position.x overridden")).toBeTruthy();
    expect(section.queryByLabelText("position.y overridden")).toBeNull();
    // The component holds an override: its header gets the per component actions.
    expect(section.getByRole("button", { name: "Revert Pose to prefab" })).toBeTruthy();
    expect(section.queryByRole("button", { name: "Revert position.y to prefab" })).toBeNull();
  });

  it("fields_under_an_overridden_parent_or_component_are_marked", async () => {
    const { host } = await editor({ overrides: [override("position")] });
    let section = await inspectInstanceEntity(host);
    // Marked under the override, the actions only where it targets.
    expect(section.getByLabelText("position.y overridden")).toBeTruthy();
    expect(section.queryByRole("button", { name: "Revert position.y to prefab" })).toBeNull();
    expect(section.getByRole("button", { name: "Revert position to prefab" })).toBeTruthy();
    expect(section.queryByLabelText("rotation.cos overridden")).toBeNull();
    cleanup();

    const whole = await editor({ overrides: [override()] });
    section = await inspectInstanceEntity(whole.host);
    expect(section.getByLabelText("rotation.cos overridden")).toBeTruthy();
    expect(section.getByLabelText("position.x overridden")).toBeTruthy();
  });

  it("consecutive_field_edits_on_an_instance_are_all_marked", async () => {
    const { host } = await editor();
    const section = await inspectInstanceEntity(host);
    const change = (name: string, value: string) => {
      const input = section.getByRole("spinbutton", { name });
      fireEvent.change(input, { target: { value } });
      fireEvent.blur(input);
    };

    change("position.y", "4");
    await settle();
    change("rotation.sin", "0.5");
    await settle();

    for (const name of ["position.x", "position.y", "rotation.sin"]) expect(section.getByLabelText(`${name} overridden`)).toBeTruthy();
  });

  it("revert_sends_revert_override", async () => {
    const { host, game } = await editor();
    const section = await inspectInstanceEntity(host);

    const reads = sent(game, "get_instances").length;
    fireEvent.click(section.getByRole("button", { name: "Revert position.x to prefab" }));
    fireEvent.click(section.getByRole("button", { name: "Revert Pose to prefab" }));
    await settle();

    // One read of the instances per revert.
    expect(sent(game, "get_instances").length - reads).toBe(2);
    const target = { instance: [5], entity: [0], component: POSE };
    expect(sent(game, "revert_override")).toEqual([
      { type: "revert_override", ...target, path: [field("position"), field("x")] },
      { type: "revert_override", ...target, path: [] },
    ]);
  });

  it("apply_writes_returned_prefab_text", async () => {
    const { host, game } = await editor();
    const section = await inspectInstanceEntity(host);

    fireEvent.click(section.getByRole("button", { name: "Apply position.x to prefab" }));
    await settle();

    expect(sent(game, "apply_to_prefab")).toEqual([{ type: "apply_to_prefab", instance: [5], entity: [0], component: POSE, path: [field("position"), field("x")] }]);
    expect(host.project.sent).toContainEqual({ type: "write_text", path: "assets/ball.prefab.ron", text: "(applied)" });
  });

  it("place_prefab_sends_place_instance", async () => {
    const { host, game } = await editor();
    renderPanel(host, "ludenea.scene-tree");
    await act(() => host.editor.commands.run("prefabs.place"));
    await settle();

    const picker = screen.getByRole("combobox", { name: "Prefab" }) as HTMLSelectElement;
    expect([...picker.options].map((option) => option.textContent)).toEqual(["assets/ball.prefab.ron", "assets/new.prefab.ron"]);
    fireEvent.change(picker, { target: { value: BALL } });
    fireEvent.click(screen.getByRole("button", { name: "Place" }));
    await settle();

    expect(sent(game, "place_instance")).toEqual([{ type: "place_instance", prefab: BALL }]);
    expect(screen.queryByRole("combobox", { name: "Prefab" })).toBeNull();
  });

  it("create_from_selection_creates_prefab_asset", async () => {
    const { host, game } = await editor();
    act(() => host.editor.selection.getState().select([entity(0)]));

    await act(() => host.editor.commands.run("prefabs.create-from-selection"));

    expect(sent(game, "create_prefab")).toEqual([{ type: "create_prefab", entities: [entity(0)] }]);
    // `assets/new.prefab.ron` exists: the next free name is taken.
    expect(host.project.sent).toContainEqual({ type: "create_asset", path: "assets/new-2.prefab.ron", kind: "Prefab", text: "(new prefab)" });
    expect(sent(game, "replace_with_instance")).toEqual([{ type: "replace_with_instance", entities: [entity(0)], prefab: CREATED, prefab_text: "(new prefab)" }]);
  });

  it("missing_prefab_error_shows_on_tree", async () => {
    const message = `instance 5 references asset ${BALL}, which is not available; add it or fix the reference`;
    const { host, game } = await editor({ refuse: (text) => (text.includes("other") ? message : null) });
    renderPanel(host, "ludenea.scene-tree");
    renderPanel(host, "ludenea.console");

    await act(() => host.editor.document.getState().open(OTHER_SCENE).catch(() => {}));
    await settle();

    const tree = screen.getByRole("listbox", { name: "Entities" }).closest(".ludenea-scene-tree") as HTMLElement;
    expect(within(tree).getByRole("alert").textContent).toContain(message);
    expect(screen.getAllByText(new RegExp(`Cannot open scenes/other.scene.ron: ${message}`))).toHaveLength(2);
    // The previous document and its session stay.
    expect(host.editor.document.getState().current?.id).toBe(SCENE);
    expect(sent(game, "load_session")).toEqual([]);
  });

  it("stale_override_warnings_reach_console", async () => {
    const warnings = [
      `instance 5 of prefab assets/ball.prefab.ron: the override of \`ludenea::Pose\` at \`position.x\` on entity 3 targets nothing the prefab has; it is kept in the file and ignored`,
      `instance 5 of prefab assets/ball.prefab.ron: the removal of \`ludenea::Body\` from entity 4 targets nothing the prefab has; it is kept in the file and ignored`,
    ];
    const { host } = await editor({ warnings });
    renderPanel(host, "ludenea.console");
    await settle();

    for (const warning of warnings) expect(screen.getAllByText(warning)).toHaveLength(1);
  });

  it("handle_field_lists_assets_of_its_type", async () => {
    const { host } = await editor();
    const edit = vi.fn(async () => {});
    const show = (asset: string, name: string) =>
      render(<Field schema={{ kind: "handle", asset }} value={BALL} path={[field(name)]} edit={edit} readOnly={false} editor={host.editor} />);
    show("ludenea::Prefab", "bullet");
    show("my_game::Settings", "settings");
    await settle();

    const options = (name: string) => [...(screen.getByRole("combobox", { name }) as HTMLSelectElement).options].map((option) => option.textContent);
    expect(options("bullet")).toEqual(["assets/ball.prefab.ron", "assets/new.prefab.ron"]);
    // The current value stays shown even when it is not of the type.
    expect(options("settings")).toEqual([BALL, "assets/settings.ron"]);
  });
});
