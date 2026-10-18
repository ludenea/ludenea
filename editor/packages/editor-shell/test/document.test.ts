import { ProtocolFailure, type TypeSchema } from "@ludenea/control-client";
import { describe, expect, it } from "vitest";
import { AssetFailure } from "../src";
import { fakeProject, gameWithSession, ids, settle, testGame, testHost, type Session } from "./support";

const session = (scene: string, extra: Partial<Session> = {}): Session => ({
  scene,
  saved_scene: null,
  inputs: "AA==",
  edits: "",
  undo: "",
  hashes: ["0000000000000001", "0000000000000002", "0000000000000003"],
  assets: [],
  branches: "",
  document: "Scene",
  keeps_assets: false,
  current: 2,
  ...extra,
});

const loads = (sent: { type: string }[]) => sent.filter((command) => command.type === "load_scene" || command.type === "load_session");

describe("document store", () => {
  it("opening_a_document_stores_the_previous_session", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await gameWithSession(session("(main)", { branches: "AQID" }));
    host.setControl(game.control);

    await host.editor.document.getState().open(ids.main);
    expect(host.editor.document.getState().current).toEqual({ id: ids.main, kind: "scene", path: "scenes/main.scene.ron" });
    await host.editor.document.getState().open(ids.ball);

    expect(project.stored(ids.main)?.load).toMatchObject({ type: "load_session", scene: "(main)", branches: "AQID" });
    expect(project.stored(ids.main)?.tick).toBe(2);
    expect(host.editor.document.getState().current).toEqual({ id: ids.ball, kind: "prefab", path: "assets/ball.prefab.ron" });
    expect(game.sent.filter((command) => command.type === "load_scene").at(-1)).toEqual({
      type: "load_scene",
      scene: project.disk["assets/ball.prefab.ron"],
      assets: project.assets(),
      document: "Prefab",
    });
  });

  it("reopening_restores_branches_and_tick", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await gameWithSession(session("(main)", { branches: "AQID", current: 2 }));
    host.setControl(game.control);
    const { open } = host.editor.document.getState();

    await open(ids.main);
    await open(ids.ball);
    game.sent.length = 0;
    await open(ids.main);

    expect(loads(game.sent).map((command) => command.type)).toEqual(["load_scene", "load_session"]);
    expect(game.sent.find((command) => command.type === "load_session")).toMatchObject({ branches: "AQID", scene: "(main)" });
    expect(game.sent).toContainEqual({ type: "seek", tick: 2 });
  });

  it("stored_session_replays_when_prefab_changed_meanwhile", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await gameWithSession(session("(main)", { assets: [[ids.ball, "(old ball)"]] }));
    host.setControl(game.control);
    const { open } = host.editor.document.getState();
    await open(ids.main);
    await open(ids.ball);

    project.disk["assets/ball.prefab.ron"] = "(new ball)";
    await open(ids.main);
    game.push({ type: "diverged", tick: 1, expected: "0000000000000002", got: "00000000000000ff" });

    const restored = game.sent.find((command) => command.type === "load_session")!;
    expect(restored.assets).toEqual(project.assets());
    expect(restored.assets).toContainEqual([ids.ball, "(new ball)"]);
    expect(restored.expected).toEqual(session("").hashes);
    expect(host.editor.timeline.getState().divergence).toEqual({ tick: 1 });
  });

  it("root_branch_takes_the_document_name", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await gameWithSession(session("(main)"));
    host.setControl(game.control);

    await host.editor.document.getState().open(ids.main);

    expect(game.sent).toContainEqual({ type: "rename_branch", branch: 0, name: "main" });
  });

  it("a_document_opened_without_a_game_loads_into_the_next_one", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    project.store(ids.main, { load: { ...session("(main)", { branches: "AQID" }), type: "load_session", expected: [] }, tick: 0 });

    await host.editor.document.getState().open(ids.main);
    const game = await gameWithSession(session("(main)"));
    host.setControl(game.control);
    await settle();

    expect(loads(game.sent).map((command) => command.type)).toEqual(["load_scene", "load_session"]);
    expect(game.sent.find((command) => command.type === "load_scene")).toEqual({ type: "load_scene", scene: "(main)", assets: project.assets(), document: "Scene" });
  });

  it("broken_asset_error_names_path_line_and_column", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const message = `asset ${ids.settings} is invalid: 2:17-2:18: Expected integer`;
    const game = await testGame((command) => (command.type === "replace_scene" ? { error: { code: "invalid_scene", message } } : { type: "done" }));
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);

    const failure = await host.editor.document.getState().replace("(main)").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(AssetFailure);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure).toMatchObject({ code: "invalid_scene", path: "assets/settings.ron", line: 2, column: 17 });
    expect((failure as Error).message).toBe(`assets/settings.ron:2:17: ${message}`);
  });
});

describe("assets", () => {
  const settings: TypeSchema = { kind: "struct", name: "demo::Settings", fields: [{ name: "gravity_scale", schema: { kind: "f32" } }] };

  it("asset_create_writes_default_value", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await testGame((command) => (command.type === "new_asset_value" ? { type: "text", text: "(gravity_scale: 1.0)" } : { type: "done" }));
    host.setControl(game.control);

    const created = await host.editor.assets.create("demo::Settings", "assets/low.ron");

    expect(game.sent).toContainEqual({ type: "new_asset_value", asset: "demo::Settings" });
    expect(project.sent).toContainEqual({ type: "create_asset", path: "assets/low.ron", kind: { Data: { type: "demo::Settings" } }, text: "(gravity_scale: 1.0)" });
    expect(created).toEqual({ kind: "created", entry: { path: "assets/low.ron", id: ids.created, kind: { Data: { type: "demo::Settings" } } } });
  });

  it("asset_create_without_default_needs_value", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const message = "asset type `demo::Settings` has no default value; register it with `World::register_asset_with_default`";
    const game = await testGame(() => ({ error: { code: "invalid_command", message } }), { assets: [{ name: "demo::Settings", schema: settings }] });
    host.setControl(game.control);

    const created = await host.editor.assets.create("demo::Settings", "assets/low.ron");

    expect(created).toEqual({ kind: "needs-value", schema: settings });
    expect(project.sent.filter((request) => request.type === "create_asset")).toEqual([]);
  });
});
