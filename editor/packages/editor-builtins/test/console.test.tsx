import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, settle, testEditor, testGame } from "./support";

const RENDERED = "error[E0425]: cannot find value `speed` in this scope\n --> src/lib.rs:12:5\n";

describe("console", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

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
    fireEvent.click(screen.getByRole("button", { name: "src/lib.rs:12:5" }));
    expect((screen.getByRole("searchbox", { name: "Filter assets" }) as HTMLInputElement).value).toBe("src/lib.rs");
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
});
