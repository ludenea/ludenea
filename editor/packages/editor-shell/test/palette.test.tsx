import { ProtocolFailure } from "@ludenea/control-client";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { normalizeShortcut } from "../src";
import { CommandPalette, installShortcuts } from "../src/app";
import { logged, settle, testHost } from "./support";

describe("command palette and shortcuts", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("a_failing_command_run_from_the_palette_is_logged_in_the_editor_console", async () => {
    const devtools = vi.spyOn(console, "error");
    const host = await testHost();
    const { editor } = host;
    editor.commands.register({
      id: "scene.despawn-selection",
      title: "Despawn selected entities",
      run() {
        throw new Error("nothing selected");
      },
    });
    render(<CommandPalette editor={editor} />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
    await act(settle);

    expect(logged(host)).toEqual([["error", "Despawn selected entities failed: nothing selected", undefined]]);
    expect(screen.getByRole("alert").textContent).toBe("Despawn selected entities failed: nothing selected");
    expect(devtools).not.toHaveBeenCalled();
  });

  it("the_palette_alert_of_a_failed_command_goes_away", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const { editor } = await testHost();
    editor.commands.register({ id: "scene.save", title: "Save scene", run: () => Promise.reject(new Error("the disk is full")) });
    render(<CommandPalette editor={editor} />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
    await act(() => Promise.resolve());
    expect(screen.getByRole("alert").textContent).toBe("Save scene failed: the disk is full");

    act(() => vi.runAllTimers());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("a_failing_command_run_from_a_shortcut_is_logged_in_the_editor_console", async () => {
    const devtools = vi.spyOn(console, "error");
    const host = await testHost();
    const { editor, registry } = host;
    editor.commands.register({ id: "edit.undo", title: "Undo", run: () => Promise.reject(new ProtocolFailure("nothing_to_undo", "there is nothing to undo")) });
    editor.shortcuts.register({ key: "Ctrl+Z", command: "edit.undo" });
    editor.shortcuts.register({ key: "Ctrl+Y", command: "edit.unknown" });
    const uninstall = installShortcuts(editor, registry, window);

    fireEvent.keyDown(document.body, { key: "z", ctrlKey: true });
    await settle();
    // A shortcut whose command is not registered fails the same way.
    fireEvent.keyDown(document.body, { key: "y", ctrlKey: true });
    await settle();

    expect(logged(host)).toEqual([
      ["error", "Undo failed: there is nothing to undo", undefined],
      ["error", 'edit.unknown failed: no command "edit.unknown" is registered', undefined],
    ]);
    expect(devtools).not.toHaveBeenCalled();
    uninstall();
  });

  it("palette_lists_and_runs_registered_commands", async () => {
    const { editor } = await testHost();
    const heal = vi.fn();
    editor.commands.register({ id: "combat.heal-all", title: "Heal all", run: heal });
    editor.commands.register({ id: "scene.save", title: "Save scene", run: () => {} });
    render(<CommandPalette editor={editor} />);
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["Heal all", "Save scene"]);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "hal" } });
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["Heal all"]);
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });

    expect(heal).toHaveBeenCalledWith(editor);
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("palette_closes_on_escape_and_on_a_click_outside_the_panel", async () => {
    const { editor } = await testHost();
    editor.commands.register({ id: "scene.save", title: "Save scene", run: () => {} });
    render(<CommandPalette editor={editor} />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.click(screen.getByRole("option"));
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.click(screen.getByRole("combobox"));
    expect(screen.getByRole("dialog")).toBeTruthy();
    // A drag that selects the query text and ends outside the panel is not a click outside.
    fireEvent.mouseDown(screen.getByRole("combobox"));
    fireEvent.click(screen.getByTestId("palette-backdrop"));
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.mouseDown(screen.getByTestId("palette-backdrop"));
    fireEvent.click(screen.getByTestId("palette-backdrop"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("palette_is_modal_and_keeps_the_focus", async () => {
    const { editor } = await testHost();
    render(<CommandPalette editor={editor} />);
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });

    expect(screen.getByRole("dialog").getAttribute("aria-modal")).toBe("true");
    const input = screen.getByRole("combobox");
    expect(document.activeElement).toBe(input);
    expect(fireEvent.keyDown(input, { key: "Tab" })).toBe(false);
    expect(fireEvent.keyDown(input, { key: "Tab", shiftKey: true })).toBe(false);
    expect(document.activeElement).toBe(input);
  });

  it("arrow_down_without_results_keeps_the_first_row_highlighted", async () => {
    const { editor } = await testHost();
    const run = vi.fn();
    const { rerender } = render(<CommandPalette editor={editor} />);
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "zzz" } });
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });

    editor.commands.register({ id: "zzz", title: "Zzz", run });
    rerender(<CommandPalette editor={editor} />);
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
    expect(run).toHaveBeenCalledOnce();
  });

  it("palette_points_at_the_highlighted_option_and_keeps_it_in_view", async () => {
    const { editor } = await testHost();
    for (const title of ["Alpha", "Beta", "Gamma"]) editor.commands.register({ id: title, title, run: () => {} });
    const scrolled: string[] = [];
    const scrollIntoView = vi.mocked(Element.prototype.scrollIntoView).mockImplementation(function (this: Element) {
      scrolled.push(this.textContent ?? "");
    });
    render(<CommandPalette editor={editor} />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    const input = screen.getByRole("combobox");
    const options = screen.getAllByRole("option");
    expect(input.getAttribute("aria-activedescendant")).toBe(options[0]!.id);
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input.getAttribute("aria-activedescendant")).toBe(options[1]!.id);
    expect(options[0]!.id).not.toBe(options[1]!.id);
    expect(scrolled.at(-1)).toBe("Beta");
    expect(scrollIntoView).toHaveBeenLastCalledWith({ block: "nearest" });
  });

  it("palette_shows_the_shortcut_of_each_command", async () => {
    const { editor, registry } = await testHost();
    editor.commands.register({ id: "scene.save", title: "Save scene", run: () => {} });
    editor.commands.register({ id: "scene.close", title: "Close scene", run: () => {} });
    editor.shortcuts.register({ key: "Ctrl+S", command: "scene.save" });
    render(<CommandPalette editor={editor} registry={registry} />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["Close scene", "Save sceneCtrl+S"]);
  });

  it("shortcut_ignored_inside_text_input", async () => {
    const { editor, registry } = await testHost();
    const save = vi.fn();
    editor.commands.register({ id: "scene.save", title: "Save scene", run: save });
    editor.shortcuts.register({ key: "Ctrl+S", command: "scene.save" });
    const uninstall = installShortcuts(editor, registry, window);
    const input = document.body.appendChild(document.createElement("input"));

    fireEvent.keyDown(input, { key: "s", ctrlKey: true });
    expect(save).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "s", ctrlKey: true });
    expect(save).toHaveBeenCalledOnce();

    uninstall();
    input.remove();
  });

  it("named_keys_match_keyboard_event_values", async () => {
    expect(normalizeShortcut("shift+arrowup")).toBe("Shift+ArrowUp");
    expect(normalizeShortcut("delete")).toBe("Delete");
    expect(normalizeShortcut("Ctrl+PAGEDOWN")).toBe("Ctrl+PageDown");
    expect(normalizeShortcut("f5")).toBe("F5");
    expect(() => normalizeShortcut("Ctrl+Arrowup2")).toThrow("Arrowup2");

    const { editor, registry } = await testHost();
    const up = vi.fn();
    editor.commands.register({ id: "viewport.pan-up", title: "Pan up", run: up });
    editor.shortcuts.register({ key: "shift+arrowup", command: "viewport.pan-up" });
    const uninstall = installShortcuts(editor, registry, window);
    fireEvent.keyDown(document.body, { key: "ArrowUp", shiftKey: true });
    expect(up).toHaveBeenCalledOnce();
    uninstall();
  });

  it("mod_is_cmd_on_macos_and_ctrl_elsewhere", () => {
    expect(normalizeShortcut("Mod+S", true)).toBe("Cmd+S");
    expect(normalizeShortcut("mod+shift+z", false)).toBe("Ctrl+Shift+Z");
  });

  it("captured_input_leaves_only_f5_and_mod_shortcuts_to_the_editor", async () => {
    const { editor, registry } = await testHost();
    const ran: string[] = [];
    for (const [key, id] of [["F5", "play"], ["Mod+Z", "undo"], ["F", "frame"], [".", "step"], ["Shift+ArrowLeft", "pan"]]) {
      editor.commands.register({ id: id!, title: id!, run: () => void ran.push(id!) });
      editor.shortcuts.register({ key: key!, command: id! });
    }
    const uninstall = installShortcuts(editor, registry, window);
    const mod = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? { metaKey: true } : { ctrlKey: true };
    const press = () => {
      fireEvent.keyDown(document.body, { key: "F5" });
      fireEvent.keyDown(document.body, { key: "z", ...mod });
      fireEvent.keyDown(document.body, { key: "f" });
      fireEvent.keyDown(document.body, { key: "." });
      fireEvent.keyDown(document.body, { key: "ArrowLeft", shiftKey: true });
    };

    // Only the shell changes it.
    expect(() => {
      // @ts-expect-error `captured` is read-only
      editor.shortcuts.captured = true;
    }).toThrow(TypeError);
    const first = editor.shortcuts.capture();
    const second = editor.shortcuts.capture();
    first.dispose();
    first.dispose();
    expect(editor.shortcuts.captured).toBe(true);
    press();
    expect(ran).toEqual(["play", "undo"]);

    second.dispose();
    expect(editor.shortcuts.captured).toBe(false);
    ran.length = 0;
    press();
    expect(ran).toEqual(["play", "undo", "frame", "step", "pan"]);
    uninstall();
  });
});
