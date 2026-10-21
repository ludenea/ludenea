import { act, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { captureInput, registerPlayerCommands } from "../src/input/capture";
import { settle, testEditor, testGame } from "./support";

const idle = { type: "set_live_input", devices: { keys: [], gamepads: [] } };

describe("live input capture", () => {
  afterEach(() => {
    document.body.replaceChildren();
    vi.unstubAllGlobals();
  });

  it("capture_sends_pressed_codes_and_idle_on_blur", async () => {
    const host = await testEditor([]);
    const game = await testGame(undefined, { input: { kind: "unit" } });
    act(() => host.setControl(game.client));
    const element = document.body.appendChild(document.createElement("div"));
    element.tabIndex = 0;
    const stop = captureInput(host.editor, element);
    const live = () => game.sent.filter((command) => command.type === "set_live_input");

    // Paused: keys stay with the editor.
    element.focus();
    fireEvent.keyDown(element, { code: "KeyA", key: "a" });
    await settle();
    expect(live()).toEqual([]);
    expect(host.editor.shortcuts.captured).toBe(false);

    act(() => host.editor.timeline.setState({ playing: true }));
    fireEvent.keyDown(element, { code: "ArrowLeft", key: "ArrowLeft" });
    await waitFor(() => expect(live().at(-1)).toEqual({ type: "set_live_input", devices: { keys: ["KeyA", "ArrowLeft"], gamepads: [] } }));
    expect(host.editor.shortcuts.captured).toBe(true);
    fireEvent.keyUp(element, { code: "KeyA", key: "a" });
    await waitFor(() => expect(live().at(-1)).toEqual({ type: "set_live_input", devices: { keys: ["ArrowLeft"], gamepads: [] } }));
    const sent = live().length;
    await settle();
    // Unchanged input is not sent again.
    expect(live()).toHaveLength(sent);

    element.blur();
    expect(live().at(-1)).toEqual(idle);
    expect(host.editor.shortcuts.captured).toBe(false);
    stop();
  });

  it("editor_keys_stay_out_of_the_live_input", async () => {
    const host = await testEditor([]);
    const game = await testGame(undefined, { input: { kind: "unit" } });
    act(() => host.setControl(game.client));
    const element = document.body.appendChild(document.createElement("div"));
    element.tabIndex = 0;
    const stop = captureInput(host.editor, element);
    const live = () => game.sent.filter((command) => command.type === "set_live_input");
    element.focus();
    act(() => host.editor.timeline.setState({ playing: true }));

    fireEvent.keyDown(element, { code: "F5", key: "F5" });
    fireEvent.keyDown(element, { code: "KeyZ", key: "z", ctrlKey: true, metaKey: true });
    fireEvent.keyDown(element, { code: "KeyD", key: "d" });
    // A modifier pressed alone is the game's: browsers set its own flag on its keydown.
    fireEvent.keyDown(element, { code: "ControlLeft", key: "Control", ctrlKey: true });
    fireEvent.keyDown(element, { code: "MetaLeft", key: "Meta", metaKey: true });
    await waitFor(() =>
      expect(live().at(-1)).toEqual({ type: "set_live_input", devices: { keys: ["KeyD", "ControlLeft", "MetaLeft"], gamepads: [] } }),
    );
    stop();
  });

  it("live_input_the_game_refuses_is_reported_in_the_editor_console", async () => {
    const host = await testEditor([]);
    const game = await testGame((command) => (command.type === "set_live_input" ? { error: { code: "invalid_command", message: "the game is resimulating" } } : { type: "done" }), {
      input: { kind: "unit" },
    });
    act(() => host.setControl(game.client));
    const element = document.body.appendChild(document.createElement("div"));
    element.tabIndex = 0;
    const stop = captureInput(host.editor, element);
    element.focus();
    act(() => host.editor.timeline.setState({ playing: true }));

    fireEvent.keyDown(element, { code: "KeyD", key: "d" });
    await waitFor(() =>
      expect(host.editor.console.list().map(({ level, text }) => [level, text])).toContainEqual(["error", "Sending the live input to the game failed: the game is resimulating"]),
    );
    stop();
  });

  it("a_game_without_an_input_type_captures_nothing", async () => {
    const host = await testEditor([]);
    const game = await testGame();
    act(() => host.setControl(game.client));
    const element = document.body.appendChild(document.createElement("div"));
    element.tabIndex = 0;
    const stop = captureInput(host.editor, element);
    element.focus();
    act(() => host.editor.timeline.setState({ playing: true }));

    fireEvent.keyDown(element, { code: "KeyD", key: "d" });
    await settle();
    await settle();
    expect(host.editor.shortcuts.captured).toBe(false);
    expect(game.sent.filter((command) => command.type === "set_live_input")).toEqual([]);
    stop();
  });

  it("pausing_sends_idle_and_releases_the_keys", async () => {
    const host = await testEditor([]);
    const game = await testGame(undefined, { input: { kind: "unit" } });
    act(() => host.setControl(game.client));
    const element = document.body.appendChild(document.createElement("div"));
    element.tabIndex = 0;
    const stop = captureInput(host.editor, element);
    const live = () => game.sent.filter((command) => command.type === "set_live_input");
    element.focus();
    act(() => host.editor.timeline.setState({ playing: true }));
    fireEvent.keyDown(element, { code: "KeyD", key: "d" });
    await waitFor(() => expect(live().at(-1)).toEqual({ type: "set_live_input", devices: { keys: ["KeyD"], gamepads: [] } }));

    act(() => host.editor.timeline.setState({ playing: false }));
    await waitFor(() => expect(live().at(-1)).toEqual(idle));
    expect(host.editor.shortcuts.captured).toBe(false);
    stop();
  });

  it("sends_every_gamepad_slot_with_nulls_for_empty_ones", async () => {
    const pad = { buttons: [{ value: 0 }, { value: 1 }], axes: [0.5, -1] };
    vi.stubGlobal("navigator", { ...navigator, getGamepads: () => [null, pad, null, null] });
    const host = await testEditor([]);
    const game = await testGame(undefined, { input: { kind: "unit" } });
    act(() => host.setControl(game.client));
    const element = document.body.appendChild(document.createElement("div"));
    element.tabIndex = 0;
    const stop = captureInput(host.editor, element);
    element.focus();
    act(() => host.editor.timeline.setState({ playing: true }));

    await waitFor(() =>
      expect(game.sent.filter((command) => command.type === "set_live_input").at(-1)).toEqual({
        type: "set_live_input",
        devices: { keys: [], gamepads: [null, { buttons: [0, 1], axes: [0.5, -1] }, null, null] },
      }),
    );
    stop();
  });

  it("set_player_count_sends_set_players", async () => {
    const claims = [
      { keyboard: "Left", gamepad: 0 },
      { keyboard: "Right", gamepad: null },
    ];
    const host = await testEditor([]);
    const game = await testGame((command) => (command.type === "set_players" ? { type: "players", claims } : { type: "done" }), { input: { kind: "unit" } });
    act(() => host.setControl(game.client));
    registerPlayerCommands(host.editor);

    const titles = host.editor.commands.list().filter(({ id }) => id.startsWith("input.set-players-")).map(({ title }) => title);
    expect(titles).toHaveLength(4);
    expect(titles.every((title) => title.includes("replaces the recorded inputs"))).toBe(true);

    await host.editor.commands.run("input.set-players-2");

    expect(game.sent.filter((command) => command.type === "set_players")).toEqual([{ type: "set_players", players: 2, claims: null }]);
    expect(host.editor.console.list().map(({ text }) => text)).toContain("2 players, player 1: the left half of the keyboard and gamepad 1; player 2: the right half of the keyboard");
  });

  it("set_player_count_without_a_game_fails_naming_the_cause", async () => {
    const host = await testEditor([]);
    registerPlayerCommands(host.editor);
    await expect(host.editor.commands.run("input.set-players-1")).rejects.toThrow(/no game is running/);
  });
});
