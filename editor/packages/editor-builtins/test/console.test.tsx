import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { definePlugin } from "@ludenea/editor-shell";
import { builtinPlugins } from "../src";
import { consolePlugin } from "../src/console";
import { renderPanel, settle, testEditor, testGame } from "./support";

const RENDERED = "error[E0425]: cannot find value `speed` in this scope\n --> src/lib.rs:12:5\n";

describe("console", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("console_shows_build_diagnostic_with_location", async () => {
    const host = await testEditor(builtinPlugins, (request) => (request.type === "list_files" ? { type: "files", entries: [] } : undefined));
    renderPanel(host, "ludenea.console");
    renderPanel(host, "ludenea.assets");
    await settle();

    act(() => {
      host.project.push({ type: "build", event: { type: "line", text: "   Compiling demo v0.1.0" } });
      host.project.push({ type: "build", event: { type: "line", text: RENDERED } });
      host.project.push({
        type: "build",
        event: { type: "diagnostic", level: "error", message: "cannot find value `speed` in this scope", file: "src/lib.rs", line: 12, column: 5 },
      });
      host.project.push({ type: "build", event: { type: "finished", ok: false } });
    });

    expect(screen.getAllByText(/cannot find value `speed`/)).toHaveLength(1);
    expect(screen.getByText(/Compiling demo/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "src/lib.rs:12:5" })).toBeTruthy();
  });

  it("a_source_location_is_copied_and_an_asset_location_revealed", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const host = await testEditor(builtinPlugins, (request) => (request.type === "list_files" ? { type: "files", entries: [] } : undefined));
    renderPanel(host, "ludenea.console");
    renderPanel(host, "ludenea.assets");
    await settle();
    const diagnostic = (file: string) => ({ type: "build", event: { type: "diagnostic", level: "warning", message: "unused", file, line: 3, column: null } });
    act(() => {
      host.project.push(diagnostic("src/lib.rs"));
      host.project.push(diagnostic("scenes/main.scene.ron"));
    });
    const filter = () => (screen.getByRole("searchbox", { name: "Filter assets" }) as HTMLInputElement).value;

    // The asset browser never lists sources: the location goes to the clipboard instead.
    fireEvent.click(screen.getByRole("button", { name: "src/lib.rs:3" }));
    await settle();
    expect(writeText).toHaveBeenCalledWith("src/lib.rs:3");
    expect(screen.getByRole("status").textContent).toBe("Copied src/lib.rs:3");
    expect(filter()).toBe("");

    fireEvent.click(screen.getByRole("button", { name: "scenes/main.scene.ron:3" }));
    expect(filter()).toBe("scenes/main.scene.ron");
  });

  it("console_shows_game_output_and_host_console", async () => {
    const host = await testEditor(builtinPlugins);
    const game = await testGame();
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.console");

    act(() => {
      host.project.push({ type: "game_output", stream: "Stdout", line: "hello from the game" });
      host.project.push({ type: "warning", message: "orphan meta scenes/old.scene.ron.meta" });
      game.push({ type: "console", level: "error", text: "system `move` panicked at tick 12" });
    });

    expect(screen.getByText("hello from the game")).toBeTruthy();
    expect(screen.getByText(/orphan meta/)).toBeTruthy();
    expect(screen.getByText(/panicked at tick 12/)).toBeTruthy();

    fireEvent.click(screen.getByRole("checkbox", { name: "Errors" }));
    expect(screen.queryByText(/panicked at tick 12/)).toBeNull();
    expect(screen.getByText("hello from the game")).toBeTruthy();
  });

  it("a_message_naming_a_project_asset_by_identifier_names_its_file_too", async () => {
    const SCENE = "0000000000000000000000000000000a";
    const BALL = "6c90cde358504084bfb84f6d1d83c6d9";
    const host = await testEditor(builtinPlugins, (request) => {
      if (request.type === "list_files") return { type: "files", entries: [{ path: "scenes/main.scene.ron", id: SCENE, kind: "Scene" }] };
      if (request.type === "read_text") return { type: "text", text: "(scene)" };
      if (request.type === "read_asset_texts") return { type: "asset_texts", assets: [[BALL, "(ball)"]], paths: [[BALL, "assets/ball.prefab.ron"]] };
      if (request.type === "load_stored_session") return { type: "stored_session", bytes: null };
      return undefined;
    });
    const game = await testGame();
    act(() => host.setControl(game.client));
    renderPanel(host, "ludenea.console");
    await act(() => host.editor.document.getState().open(SCENE));

    // A panic while playing, and a request that failed on one.
    act(() => game.push({ type: "console", level: "error", text: `the game panicked at tick 3 in system \`game::spawn\`: prefab ${BALL} has no entity with local identifier 5` }));
    act(() => host.editor.console.log("error", `Step failed: prefab ${BALL} has no entity with local identifier 5`));
    // One the document store located already.
    act(() => host.editor.console.log("error", `Cannot reload: asset ${BALL} (assets/ball.prefab.ron) is invalid`));

    const lines = screen.getByRole("list", { name: "Console" }).textContent;
    expect(lines).toContain(`system \`game::spawn\`: prefab ${BALL} (assets/ball.prefab.ron) has no entity`);
    expect(lines).toContain(`Step failed: prefab ${BALL} (assets/ball.prefab.ron) has no entity`);
    expect(lines).toContain(`Cannot reload: asset ${BALL} (assets/ball.prefab.ron) is invalid`);
  });

  it("console_shows_what_the_application_reports", async () => {
    const host = await testEditor(builtinPlugins);
    renderPanel(host, "ludenea.console");
    act(() => host.editor.console.log("error", "The game crashed at tick 4: boom"));
    expect(screen.getByText("The game crashed at tick 4: boom")).toBeTruthy();
  });

  it("a_project_plugin_logs_to_the_console_through_the_public_editor", async () => {
    // What a project plugin can import: `@ludenea/editor-shell` only.
    const project = definePlugin({
      name: "my_game",
      activate(editor) {
        editor.console.log("warning", "my_game: the level has no spawn point", { file: "assets/level.ron", line: 4, column: null });
        editor.console.report(new Error("no game is connected"), "Reset the level");
      },
    });
    // It activates before the Console: what it logged waits for the panel.
    const host = await testEditor([project, consolePlugin]);
    renderPanel(host, "ludenea.console");

    expect(screen.getByText("my_game: the level has no spawn point")).toBeTruthy();
    expect(screen.getByRole("button", { name: "assets/level.ron:4" })).toBeTruthy();
    expect(screen.getByText("Reset the level failed: no game is connected")).toBeTruthy();
    act(() => host.editor.console.log("info", "my_game: level reset"));
    expect(screen.getByText("my_game: level reset")).toBeTruthy();
    expect(screen.getAllByRole("listitem")).toHaveLength(3);
  });

  it("scroll_position_survives_hiding_the_panel", async () => {
    const host = await testEditor(builtinPlugins);
    renderPanel(host, "ludenea.console");
    const lines = () => screen.getByRole("list", { name: "Console" });
    lines().scrollTop = 300;
    fireEvent.scroll(lines());

    // The shell unmounts a panel while its tab is hidden.
    cleanup();
    renderPanel(host, "ludenea.console");
    expect(lines().scrollTop).toBe(300);
  });

  it("locations_are_classified_relative_to_the_project_root", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const info = { type: "info", name: "demo", root: "/work/demo", default_scene: null, mode: "browser" };
    const host = await testEditor(builtinPlugins, (request) => (request.type === "info" ? info : request.type === "list_files" ? { type: "files", entries: [] } : undefined));
    renderPanel(host, "ludenea.console");
    renderPanel(host, "ludenea.assets");
    await settle();
    const diagnostic = (file: string) => ({ type: "build", event: { type: "diagnostic", level: "warning", message: file, file, line: 1, column: null } });
    act(() => ["/work/demo/scenes/main.scene.ron", "./assets/logo.png", "/work/demo/src/lib.rs"].forEach((file) => host.project.push(diagnostic(file))));
    const filter = () => (screen.getByRole("searchbox", { name: "Filter assets" }) as HTMLInputElement).value;

    fireEvent.click(screen.getByRole("button", { name: "/work/demo/scenes/main.scene.ron:1" }));
    expect(filter()).toBe("scenes/main.scene.ron");
    fireEvent.click(screen.getByRole("button", { name: "./assets/logo.png:1" }));
    expect(filter()).toBe("assets/logo.png");
    fireEvent.click(screen.getByRole("button", { name: "/work/demo/src/lib.rs:1" }));
    await settle();
    expect(writeText).toHaveBeenCalledWith("/work/demo/src/lib.rs:1");
  });
});
