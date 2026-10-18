import { ProtocolFailure, type TypeSchema } from "@ludenea/control-client";
import { describe, expect, it, vi } from "vitest";
import { AssetFailure } from "../src";
import { fakeProject, gameWithSession, ids, settle, testGame, testHost, timeline, type Session } from "./support";

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

  it("reopening_seeks_once_resimulated_past_the_tick", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const exported = session("(main)");
    let resimulating = false;
    // A resumed session keeps its recorded hashes while it resimulates: `simulated` counts them.
    const game = await testGame((command) => {
      if (command.type === "export_session") return { type: "session", ...exported };
      if (command.type === "get_timeline") return { ...timeline(2), resimulating };
      if (command.type === "load_scene") return { type: "loaded", warnings: [], instances: [], load_addresses: [] };
      return { type: "done" };
    });
    host.setControl(game.control);
    const { open } = host.editor.document.getState();
    await open(ids.main);
    await open(ids.ball);

    resimulating = true;
    game.sent.length = 0;
    const reopened = open(ids.main);
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    for (let turn = 0; turn < 5; turn++) await settle();
    expect(game.sent.filter((command) => command.type === "seek")).toEqual([]);

    game.push({ type: "resimulated", up_to: 2 });
    await reopened;
    expect(game.sent).toContainEqual({ type: "seek", tick: 2 });
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

  it("a_replay_that_fails_to_open_leaves_the_open_one_for_the_next_game", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer, vi.fn());
    const game = await testGame((command) =>
      command.type === "load_replay" && command.replay === btoa("replays/second.replay") ? { error: { code: "invalid_replay", message: "edit of tick 3 failed to decode" } } : { type: "done" },
    );
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.first);

    await expect(host.editor.document.getState().open(ids.second)).rejects.toThrow("edit of tick 3");
    const next = await testGame(() => ({ type: "done" }));
    host.setControl(next.control);

    expect(host.editor.document.getState().current?.id).toBe(ids.first);
    expect(next.sent.filter((command) => command.type === "load_replay")).toEqual([{ type: "load_replay", replay: btoa("replays/first.replay") }]);
  });

  it("a_document_a_new_game_refuses_is_logged_at_the_asset", async () => {
    const project = fakeProject();
    const log = vi.fn();
    const host = await testHost(undefined, project.answer, log);
    await host.editor.document.getState().open(ids.main);
    const message = `asset ${ids.settings} is invalid: 3:5: Expected comma`;

    const game = await testGame((command) => (command.type === "load_scene" ? { error: { code: "invalid_scene", message } } : { type: "done" }));
    host.setControl(game.control);
    await settle();

    expect(log).toHaveBeenCalledWith("error", expect.stringContaining("Expected comma"), { file: "assets/settings.ron", line: 3, column: 5 });
  });

  it("an_error_names_the_file_of_each_asset_without_a_position", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer, vi.fn());
    const message = `asset ${ids.ball} is a \`demo::Settings\`, but entity 2 expects a \`ludenea::Prefab\``;
    const game = await testGame((command) => (command.type === "load_scene" ? { error: { code: "invalid_scene", message } } : { type: "done" }));
    host.setControl(game.control);

    const failure = await host.editor.document.getState().open(ids.main).catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect((failure as Error).message).toContain(`asset ${ids.ball} (assets/ball.prefab.ron) is a`);
  });

  it("broken_asset_error_names_path_line_and_column", async () => {
    const project = fakeProject();
    const log = vi.fn();
    const host = await testHost(undefined, project.answer, log);
    const message = `asset ${ids.settings} is invalid: 2:17-2:18: Expected integer`;
    const game = await testGame((command) => (command.type === "replace_scene" ? { error: { code: "invalid_scene", message } } : { type: "done" }));
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);

    const failure = await host.editor.document.getState().replace("(main)").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(AssetFailure);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure).toMatchObject({ code: "invalid_scene", path: "assets/settings.ron", line: 2, column: 17 });
    expect((failure as Error).message).toBe(`assets/settings.ron:2:17: asset ${ids.settings} (assets/settings.ron) is invalid: 2:17-2:18: Expected integer`);
    expect(log).toHaveBeenCalledWith("error", expect.stringContaining("Expected integer"), { file: "assets/settings.ron", line: 2, column: 17 });
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
