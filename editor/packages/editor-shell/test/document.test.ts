import { ProtocolFailure, type TypeSchema } from "@ludenea/control-client";
import { describe, expect, it, vi } from "vitest";
import { AssetFailure } from "../src";
import { fakeProject, gameWithSession, ids, logged, settle, testGame, testHost, timeline, type Session } from "./support";

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
      document: { Prefab: { id: ids.ball } },
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

    // The recorded hashes go along: the game reports where the new prefab makes it diverge.
    const restored = game.sent.find((command) => command.type === "load_session")!;
    expect(restored.assets).toEqual(project.assets());
    expect(restored.assets).toContainEqual([ids.ball, "(new ball)"]);
    expect(restored.expected).toEqual(session("").hashes);
  });

  it("reopening_seeks_once_resimulated_past_the_tick", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const exported = session("(main)");
    let resimulating = false;
    // A resumed session keeps its recorded hashes while it resimulates: `simulated` counts them.
    const game = await testGame((command) => {
      if (command.type === "export_session") return { type: "session", ...exported };
      // The restored session resimulates.
      if (command.type === "load_session") resimulating = true;
      if (command.type === "get_timeline") return { ...timeline(2), resimulating };
      if (command.type === "load_scene") return { type: "loaded", warnings: [], instances: [], load_addresses: [] };
      return { type: "done" };
    });
    host.setControl(game.control);
    const { open } = host.editor.document.getState();
    await open(ids.main);
    await open(ids.ball);

    game.sent.length = 0;
    const reopened = open(ids.main);
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    for (let turn = 0; turn < 5; turn++) await settle();
    expect(game.sent.filter((command) => command.type === "seek")).toEqual([]);

    game.push({ type: "resimulated", up_to: 2 });
    await reopened;
    expect(game.sent).toContainEqual({ type: "seek", tick: 2 });
  });

  it("switching_documents_waits_for_a_resimulation_to_finish", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    let resimulating = false;
    const game = await testGame((command) => {
      if (command.type === "export_session") return resimulating ? { error: { code: "resimulating", message: "resimulating up to tick 1" } } : { type: "session", ...session("(main)") };
      if (command.type === "get_timeline") return { ...timeline(2), resimulating };
      if (command.type === "load_scene") return { type: "loaded", warnings: [], instances: [], load_addresses: [] };
      return { type: "done" };
    });
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);

    resimulating = true;
    const opened = host.editor.document.getState().open(ids.ball);
    for (let turn = 0; turn < 5; turn++) await settle();
    expect(host.editor.document.getState().current?.id).toBe(ids.main);

    resimulating = false;
    game.push({ type: "timeline_changed", timeline: timeline(2) });
    await opened;
    expect(host.editor.document.getState().current?.id).toBe(ids.ball);
    expect(project.stored(ids.main)?.load.scene).toBe("(main)");
  });

  it("a_root_branch_that_cannot_be_named_still_opens_the_document", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await testGame((command) => {
      if (command.type === "get_timeline") return timeline(0);
      if (command.type === "rename_branch") return { error: { code: "invalid_command", message: "no such branch" } };
      if (command.type === "load_scene") return { type: "loaded", warnings: [], instances: [], load_addresses: [] };
      return { type: "done" };
    });
    host.setControl(game.control);

    await host.editor.document.getState().open(ids.main);

    expect(host.editor.document.getState()).toMatchObject({ current: { id: ids.main }, error: null });
    expect(logged(host)).toContainEqual(["warning", "Cannot name the root branch main: no such branch", undefined]);
  });

  it("a_stored_session_that_fails_to_restore_leaves_the_document_open_from_its_file", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const loaded = { type: "loaded", warnings: ["from the file"], instances: [], load_addresses: [] };
    const game = await testGame((command) => {
      if (command.type === "get_timeline") return timeline(0);
      if (command.type === "load_scene") return loaded;
      if (command.type === "load_session") return { error: { code: "invalid_command", message: "the branch tree cannot be decoded" } };
      return { type: "done" };
    });
    host.setControl(game.control);
    project.store(ids.main, { load: { ...session("(main)", { assets: [[ids.settings, edited]] }), type: "load_session", expected: [] }, tick: 0, document: ids.main, unsavedAssets: [ids.settings] });

    await host.editor.document.getState().open(ids.main);

    expect(host.editor.document.getState()).toMatchObject({ current: { id: ids.main }, unsavedAssets: [], loaded: { warnings: ["from the file"] } });
    const refused = "The game refused the stored session of scenes/main.scene.ron; the document opened from its file: the branch tree cannot be decoded";
    expect(logged(host)).toContainEqual(["error", refused, undefined]);
    // Shown on the scene tree as well.
    expect(host.editor.document.getState().error).toBe(refused);
  });

  it("the_session_of_the_open_document_is_refreshed_from_disk_for_a_new_game", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await gameWithSession(session("(main)", { assets: [[ids.ball, "(old ball)"]] }));
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);
    await host.sessions.store();
    // The latest export was taken while another document was open.
    const other = { ...(await host.sessions.export(game.control)), document: ids.ball };

    const own = await host.sessions.own(other);

    expect(own?.document).toBe(ids.main);
    expect(own?.load.assets).toEqual(project.assets());
  });

  /** A game whose session resimulates after each `LoadSession` until `finish` pushes the end. */
  async function resimulatingGame(resimulatingAtFirst = false) {
    let resimulating = resimulatingAtFirst;
    const game = await testGame((command) => {
      if (command.type === "export_session") return resimulating ? { error: { code: "resimulating", message: "resimulating" } } : { type: "session", ...session("(main)") };
      if (command.type === "load_session") resimulating = true;
      if (command.type === "get_timeline") return { ...timeline(2), resimulating };
      if (command.type === "load_scene" || command.type === "load_session") return { type: "loaded", warnings: [], instances: [], load_addresses: [] };
      return { type: "done" };
    });
    const finish = () => {
      resimulating = false;
      game.push({ type: "timeline_changed", timeline: timeline(2) });
    };
    // A resimulation starts (an edit, a branch switch).
    const start = () => {
      resimulating = true;
    };
    return { ...game, finish, start };
  }

  it("an_asset_reload_waits_for_a_resimulation_then_plays_on", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await resimulatingGame();
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);
    game.sent.length = 0;
    game.finish();

    host.editor.timeline.setState({ playing: true });
    const reloaded = host.editor.document.getState().reloadAssets();
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    for (let turn = 0; turn < 5; turn++) await settle();
    expect(game.sent.filter((command) => command.type === "seek" || command.type === "play")).toEqual([]);
    game.finish();
    await reloaded;

    expect(game.sent.filter((command) => ["export_session", "load_session", "seek", "play"].includes(command.type)).map((command) => command.type)).toEqual([
      "export_session",
      "load_session",
      "seek",
      "play",
    ]);
  });

  it("an_asset_reload_during_a_resimulation_exports_once_it_ends", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await resimulatingGame();
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);
    game.finish();
    // The first reload resimulates when the files change again.
    const first = host.editor.document.getState().reloadAssets();
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    game.sent.length = 0;

    const second = host.editor.document.getState().reloadAssets();
    for (let turn = 0; turn < 5; turn++) await settle();
    expect(game.sent.filter((command) => command.type === "export_session")).toEqual([]);
    game.finish();
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    game.finish();
    await Promise.all([first, second]);
    expect(host.editor.document.getState().error).toBeNull();
  });

  it("an_asset_reload_leaves_a_new_game_where_it_is", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await resimulatingGame();
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);
    game.finish();

    const reloaded = host.editor.document.getState().reloadAssets();
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    game.sent.length = 0;
    host.setControl((await gameWithSession(session("(main)"))).control);
    game.finish();
    await reloaded;

    expect(game.sent.filter((command) => command.type === "seek")).toEqual([]);
  });

  const kinds = (sent: { type: string }[], types: string[]) => sent.filter((command) => types.includes(command.type)).map((command) => command.type);

  it("asset_reloads_asked_for_during_a_resimulation_coalesce_and_play_on", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await resimulatingGame();
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);
    // The session resimulates (an edit) while files change twice, the game playing.
    const first = host.editor.document.getState().reloadAssets();
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    game.finish();
    await first;
    game.start();
    game.sent.length = 0;
    host.editor.timeline.setState({ playing: true });
    const reloads = [host.editor.document.getState().reloadAssets(), host.editor.document.getState().reloadAssets()];
    expect(reloads[0]).toBe(reloads[1]);
    for (let turn = 0; turn < 5; turn++) await settle();
    game.finish();
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    // The load paused the game: a reload asked for now still plays on.
    host.editor.timeline.setState({ playing: false });
    const third = host.editor.document.getState().reloadAssets();
    game.finish();
    await vi.waitFor(() => expect(kinds(game.sent, ["load_session"])).toHaveLength(2));
    game.finish();
    await Promise.all([...reloads, third]);

    expect(kinds(game.sent, ["load_session", "seek", "play"])).toEqual(["load_session", "load_session", "seek", "play"]);
  });

  it("an_open_waiting_for_a_resimulation_says_so_and_keeps_a_reload_from_seeking", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await resimulatingGame();
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);
    const reloaded = host.editor.document.getState().reloadAssets();
    await vi.waitFor(() => expect(game.sent.some((command) => command.type === "load_session")).toBe(true));
    game.sent.length = 0;

    const opened = [host.editor.document.getState().open(ids.ball), host.editor.document.getState().open(ids.ball)];
    expect(opened[0]).toBe(opened[1]);
    await vi.waitFor(() => expect(logged(host)).toContainEqual(["info", "Opening assets/ball.prefab.ron once the game finishes resimulating.", undefined]));
    game.finish();
    await Promise.all([reloaded, ...opened]);

    expect(kinds(game.sent, ["seek", "load_scene"])).toEqual(["load_scene"]);
    expect(host.editor.document.getState().current?.id).toBe(ids.ball);
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
    const host = await testHost(undefined, project.answer);
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
    const host = await testHost(undefined, project.answer);
    await host.editor.document.getState().open(ids.main);
    const message = `asset ${ids.settings} is invalid: 3:5: Expected comma`;

    const game = await testGame((command) => (command.type === "load_scene" ? { error: { code: "invalid_scene", message } } : { type: "done" }));
    host.setControl(game.control);
    await settle();

    expect(logged(host)).toContainEqual(["error", expect.stringContaining("Expected comma"), { file: "assets/settings.ron", line: 3, column: 5 }]);
  });

  it("an_error_names_the_file_of_each_asset_without_a_position", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const message = `asset ${ids.ball} is a \`demo::Settings\`, but entity 2 expects a \`ludenea::Prefab\``;
    const game = await testGame((command) => (command.type === "load_scene" ? { error: { code: "invalid_scene", message } } : { type: "done" }));
    host.setControl(game.control);

    const failure = await host.editor.document.getState().open(ids.main).catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect((failure as Error).message).toContain(`asset ${ids.ball} (assets/ball.prefab.ron) is a`);
  });

  // The session's text of the data asset after a tick 0 edit, which "Save asset" has not written.
  const edited = `AssetFile(type: "demo::Settings", value: (gravity_scale: 2.0))`;

  it("an_unsaved_asset_survives_switching_documents", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await gameWithSession(session("(main)", { assets: [[ids.ball, "(ball)"], [ids.settings, edited]] }));
    host.setControl(game.control);
    const { open } = host.editor.document.getState();
    await open(ids.main);
    host.editor.document.setState({ unsavedAssets: [ids.settings] });

    await open(ids.ball);
    expect(host.editor.document.getState().unsavedAssets).toEqual([]);
    game.sent.length = 0;
    await open(ids.main);

    const restored = game.sent.find((command) => command.type === "load_session")!;
    expect(restored.assets).toEqual([
      [ids.ball, "(ball)"],
      [ids.settings, edited],
    ]);
    expect(host.editor.document.getState().unsavedAssets).toEqual([ids.settings]);
  });

  it("an_unsaved_asset_survives_closing_and_reopening_the_editor", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await gameWithSession(session("(main)", { assets: [[ids.ball, "(ball)"], [ids.settings, edited]] }));
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);
    host.editor.document.setState({ unsavedAssets: [ids.settings] });
    // As closing the editor does.
    await host.sessions.store();

    const reopened = await testHost(undefined, project.answer);
    await reopened.editor.document.getState().open(ids.main);
    expect(reopened.editor.document.getState().unsavedAssets).toEqual([ids.settings]);
    const next = await gameWithSession(session("(main)"));
    reopened.setControl(next.control);
    await vi.waitFor(() => expect(next.sent).toContainEqual({ type: "seek", tick: 2 }));

    expect(next.sent.find((command) => command.type === "load_session")?.assets).toContainEqual([ids.settings, edited]);
    expect(reopened.editor.document.getState().unsavedAssets).toEqual([ids.settings]);
  });

  it("an_unsaved_asset_the_disk_lost_is_no_longer_unsaved", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    project.store(ids.main, {
      load: { ...session("(main)", { assets: [[ids.created, "(gone)"]] }), type: "load_session", expected: [] },
      tick: 0,
      document: ids.main,
      unsavedAssets: [ids.created],
    });

    await host.editor.document.getState().open(ids.main);

    expect(host.editor.document.getState().unsavedAssets).toEqual([]);
  });

  it("a_restored_session_marks_its_unsaved_assets_again", async () => {
    const project = fakeProject();
    const host = await testHost(undefined, project.answer);
    const game = await gameWithSession(session("(main)", { assets: [[ids.ball, "(ball)"], [ids.settings, edited]] }));
    host.setControl(game.control);
    await host.editor.document.getState().open(ids.main);
    host.editor.document.setState({ unsavedAssets: [ids.settings] });

    // A rebuild or a crash: the session exported from the old game goes into a new one.
    const saved = await host.sessions.export(game.control);
    expect(saved.unsavedAssets).toEqual([ids.settings]);
    const next = await gameWithSession(session("(main)"));
    host.setControl(next.control);
    expect(host.editor.document.getState().unsavedAssets).toEqual([]);
    await settle();
    await host.sessions.restore(next.control, saved);

    expect(next.sent.find((command) => command.type === "load_session")?.assets).toContainEqual([ids.settings, edited]);
    expect(host.editor.document.getState().unsavedAssets).toEqual([ids.settings]);
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
    expect((failure as Error).message).toBe(`assets/settings.ron:2:17: asset ${ids.settings} (assets/settings.ron) is invalid: 2:17-2:18: Expected integer`);
    expect(logged(host)).toContainEqual(["error", expect.stringContaining("Expected integer"), { file: "assets/settings.ron", line: 2, column: 17 }]);
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
