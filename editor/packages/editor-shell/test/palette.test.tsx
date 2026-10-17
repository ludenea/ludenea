import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CommandPalette, installShortcuts } from "../src";
import { testHost } from "./support";

describe("command palette and shortcuts", () => {
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
});
