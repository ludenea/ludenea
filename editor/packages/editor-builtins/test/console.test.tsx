import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { builtinPlugins, logToConsole } from "../src";
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

  it("console_shows_what_the_application_reports", async () => {
    const host = await testEditor(builtinPlugins);
    renderPanel(host, "ludenea.console");
    act(() => logToConsole(host.editor, "error", "The game crashed at tick 4: boom"));
    expect(screen.getByText("The game crashed at tick 4: boom")).toBeTruthy();
  });
});
