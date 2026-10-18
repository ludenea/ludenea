import { act, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { captureInput } from "../src/input/capture";
import { settle, testEditor, testGame } from "./support";

const idle = { type: "set_live_input", players: [{ keys: [], gamepads: [] }] };

describe("live input capture", () => {
  afterEach(() => document.body.replaceChildren());

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
    await waitFor(() => expect(live().at(-1)).toEqual({ type: "set_live_input", players: [{ keys: ["KeyA", "ArrowLeft"], gamepads: [] }] }));
    expect(host.editor.shortcuts.captured).toBe(true);
    fireEvent.keyUp(element, { code: "KeyA", key: "a" });
    await waitFor(() => expect(live().at(-1)).toEqual({ type: "set_live_input", players: [{ keys: ["ArrowLeft"], gamepads: [] }] }));
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
      expect(live().at(-1)).toEqual({ type: "set_live_input", players: [{ keys: ["KeyD", "ControlLeft", "MetaLeft"], gamepads: [] }] }),
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
});
