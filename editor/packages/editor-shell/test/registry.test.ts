import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DuplicateRegistration, definePlugin } from "../src";
import { logged, testGame, testHost } from "./support";

const Empty = () => null;

describe("registry", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it("registrations_are_disposed_on_deactivate", async () => {
    const { editor, loader, setControl } = await testHost();
    const cleanup = vi.fn();
    const gameChanged = vi.fn();
    loader.activate(
      definePlugin({
        name: "combat",
        activate(editor) {
          editor.panels.register({ id: "combat.teams", title: "Teams", component: Empty });
          editor.commands.register({ id: "combat.heal-all", title: "Heal all", run: () => {} });
          editor.shortcuts.register({ key: "Ctrl+H", command: "combat.heal-all" });
          editor.overlays.register({ id: "combat.ranges", draw: () => {} });
          editor.inspectors.register("my_game::Health" as never, Empty);
          editor.settings.register({ id: "combat.radius", title: "Radius", default: 2, kind: "number" });
          editor.onGameChanged(gameChanged);
          return cleanup;
        },
      }),
    );
    expect(editor.commands.list().map((command) => command.id)).toEqual(["combat.heal-all"]);
    expect(editor.overlays.list()).toHaveLength(1);
    expect(editor.inspectors.get("my_game::Health")).toBe(Empty);

    loader.deactivate("combat");

    expect(cleanup).toHaveBeenCalledOnce();
    expect(editor.commands.list()).toEqual([]);
    expect(editor.overlays.list()).toEqual([]);
    expect(editor.inspectors.get("my_game::Health")).toBeUndefined();
    expect(() => editor.settings.get("combat.radius")).toThrow("combat.radius");
    setControl((await testGame()).control);
    expect(gameChanged).not.toHaveBeenCalled();
    // Every id is free again.
    loader.activate(
      definePlugin({
        name: "again",
        activate(editor) {
          editor.panels.register({ id: "combat.teams", title: "Teams", component: Empty });
          editor.shortcuts.register({ key: "Ctrl+H", command: "combat.heal-all" });
        },
      }),
    );
  });

  it("plugin_of_names_who_registered_a_value", async () => {
    const { editor, loader } = await testHost();
    const panel = { id: "combat.teams", title: "Teams", component: Empty };
    const column = { id: "combat.team", title: "Team", width: 40, value: () => null };
    loader.activate(
      definePlugin({
        name: "combat",
        activate(editor) {
          editor.panels.register(panel);
          editor.inspectors.register("my_game::Health" as never, Empty);
        },
      }),
    );
    const later = editor.explorer.columns.register(column);

    expect(editor.pluginOf(panel)).toBe("combat");
    expect(editor.pluginOf(Empty)).toBe("combat");
    expect(editor.pluginOf(column)).toBe("editor");
    // An equal value is not the registered one.
    expect(editor.pluginOf({ ...panel })).toBeUndefined();
    later.dispose();
    loader.deactivate("combat");
    expect(editor.pluginOf(panel)).toBeUndefined();
    expect(editor.pluginOf(column)).toBeUndefined();
  });

  it("duplicate_command_id_names_both_plugins", async () => {
    const { editor, loader } = await testHost();
    const command = { id: "scene.save", title: "Save", run: () => {} };
    loader.activate(definePlugin({ name: "first", activate: (editor) => void editor.commands.register(command) }));
    const second = definePlugin({
      name: "second",
      activate(editor) {
        editor.panels.register({ id: "second.panel", title: "Second", component: Empty });
        editor.commands.register(command);
      },
    });

    const error = (() => {
      try {
        loader.activate(second);
      } catch (error) {
        return error;
      }
    })();

    expect(error).toBeInstanceOf(DuplicateRegistration);
    expect(String(error)).toMatch(/"scene\.save".*"first".*"second"/);
    // The failed activation leaves nothing behind and can be retried under the same name.
    expect(editor.commands.list()).toHaveLength(1);
    loader.activate(definePlugin({ name: "second", activate: () => {} }));
  });

  it("duplicate_shortcut_key_is_refused_in_any_spelling", async () => {
    const { editor } = await testHost();
    editor.shortcuts.register({ key: "Ctrl+Shift+Z", command: "edit.redo" });
    expect(() => editor.shortcuts.register({ key: "shift+ctrl+z", command: "other" })).toThrow(DuplicateRegistration);
  });

  it("settings_persist_per_installation_and_fall_back_to_default", async () => {
    const first = await testHost();
    first.editor.settings.register({ id: "ludenea.engine-path", title: "Engine", default: "", kind: "path" });
    expect(first.editor.settings.get("ludenea.engine-path")).toBe("");
    first.editor.settings.set("ludenea.engine-path", "/engine");

    const second = await testHost();
    second.editor.settings.register({ id: "ludenea.engine-path", title: "Engine", default: "", kind: "path" });
    expect(second.editor.settings.get("ludenea.engine-path")).toBe("/engine");

    localStorage.setItem("ludenea.settings.ludenea.engine-path", "{not json");
    expect(second.editor.settings.get("ludenea.engine-path")).toBe("");
  });

  it("a_setting_the_browser_cannot_store_is_reported_in_the_editor_console", async () => {
    const host = await testHost();
    host.editor.settings.register({ id: "ludenea.engine-path", title: "Engine", default: "", kind: "path" });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("the storage quota is exceeded");
    });

    host.editor.settings.set("ludenea.engine-path", "/engine");

    expect(logged(host)).toEqual([["error", 'Saving the setting "ludenea.engine-path" failed: the storage quota is exceeded', undefined]]);
  });
});
