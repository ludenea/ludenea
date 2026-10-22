import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DuplicateRegistration, definePlugin, type EditCommand, type Editor, type ViewportTool } from "../src";
import { Shell } from "../src/app";
import { logged, settle, testGame, testHost, timeline } from "./support";

const entity = { index: 3, generation: 1 };

// jsdom has no layout; dockview only needs the observer to exist.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

function panelPlugin(name: string, text: string, defaultPosition: "left" | "center") {
  return definePlugin({
    name,
    activate(editor) {
      editor.panels.register({ id: `${name}.panel`, title: name, component: () => createElement("p", null, text), defaultPosition });
    },
  });
}

describe("loader", () => {
  afterEach(cleanup);

  it("reload_keeps_selection_and_layout", async () => {
    const modules: Record<string, unknown> = {
      "/plugins/tools.1.js": { default: panelPlugin("tools", "tools one", "center") },
      "/plugins/tools.2.js": { default: panelPlugin("tools", "tools two", "center") },
    };
    const host = await testHost(async (url) => modules[url]);
    host.loader.activate(panelPlugin("outline", "outline content", "left"));
    await host.loader.load("/plugins/tools.1.js");
    render(createElement(Shell, { host }));
    await act(async () => {});
    host.editor.selection.getState().select([entity]);
    const groupBefore = screen.getByText("tools one").closest(".dv-groupview");
    expect(groupBefore).not.toBeNull();

    await act(() => host.loader.reload("tools", "/plugins/tools.2.js"));

    expect(screen.queryByText("tools one")).toBeNull();
    expect(screen.getByText("tools two").closest(".dv-groupview")).toBe(groupBefore);
    const outline = screen.getByText("outline content").closest(".dv-groupview");
    expect(outline).not.toBeNull();
    expect(outline).not.toBe(groupBefore);
    expect(host.editor.selection.getState().entities).toEqual([entity]);
  });

  it("built_in_and_project_plugins_receive_the_same_editor_object", async () => {
    const received: Editor[] = [];
    const capture = (name: string) => definePlugin({ name, activate: (editor) => void received.push(editor) });
    const host = await testHost(async () => ({ default: capture("project") }));

    host.loader.activate(capture("builtin"));
    await host.loader.load("/plugins/project.1.js");

    expect(received).toHaveLength(2);
    expect(received[0]).toBe(host.editor);
    expect(received[1]).toBe(received[0]);
  });

  it("load_rejects_a_module_without_a_plugin_definition", async () => {
    const host = await testHost(async () => ({ default: 42 }));
    await expect(host.loader.load("/plugins/broken.js")).rejects.toThrow("/plugins/broken.js");
  });

  it("closed_panel_stays_closed_until_its_open_command", async () => {
    const host = await testHost();
    host.loader.activate(panelPlugin("tools", "tools content", "center"));
    render(createElement(Shell, { host }));
    await act(async () => {});

    await act(async () => fireEvent.click(screen.getByText("tools").parentElement!.querySelector(".dv-default-tab-action")!));
    expect(screen.queryByText("tools content")).toBeNull();
    await act(async () => host.loader.activate(panelPlugin("outline", "outline content", "left")));
    expect(screen.queryByText("tools content")).toBeNull();
    expect(host.editor.commands.list().map((command) => [command.id, command.title])).toEqual([
      ["panels.open.tools.panel", "Show tools panel"],
      ["panels.open.outline.panel", "Show outline panel"],
    ]);

    await act(() => host.editor.commands.run("panels.open.tools.panel"));
    expect(screen.getByText("tools content")).toBeTruthy();
    host.loader.deactivate("outline");
    await act(async () => {});
    expect(host.editor.commands.list().map((command) => command.id)).toEqual(["panels.open.tools.panel"]);
  });

  it("reload_whose_activate_throws_restores_the_previous_plugin_in_place", async () => {
    const broken = definePlugin({
      name: "tools",
      activate() {
        throw new Error("tools two is broken");
      },
    });
    const modules: Record<string, unknown> = {
      "/plugins/tools.1.js": { default: panelPlugin("tools", "tools one", "center") },
      "/plugins/tools.2.js": { default: broken },
    };
    const host = await testHost(async (url) => modules[url]);
    host.loader.activate(panelPlugin("outline", "outline content", "left"));
    await host.loader.load("/plugins/tools.1.js");
    render(createElement(Shell, { host }));
    await act(async () => {});
    const groupBefore = screen.getByText("tools one").closest(".dv-groupview");

    await act(() => expect(host.loader.reload("tools", "/plugins/tools.2.js")).rejects.toThrow("tools two is broken"));

    expect(screen.getByText("tools one").closest(".dv-groupview")).toBe(groupBefore);
    // Still active under its name: the next reload replaces it.
    modules["/plugins/tools.3.js"] = { default: panelPlugin("tools", "tools three", "center") };
    await act(() => host.loader.reload("tools", "/plugins/tools.3.js"));
    expect(screen.getByText("tools three")).toBeTruthy();
  });

  it("reload_reports_the_new_error_when_restoring_the_previous_plugin_fails_too", async () => {
    let activations = 0;
    const flaky = definePlugin({
      name: "tools",
      activate() {
        activations += 1;
        if (activations > 1) throw new Error("tools one cannot come back");
      },
    });
    const broken = definePlugin({
      name: "tools",
      activate() {
        throw new Error("tools two is broken");
      },
    });
    const host = await testHost(async () => ({ default: broken }));
    host.loader.activate(flaky);

    await expect(host.loader.reload("tools", "/plugins/tools.2.js")).rejects.toThrow("tools two is broken");
    expect(logged(host)).toEqual([["error", 'Restoring the previous plugin "tools" failed: tools one cannot come back', undefined]]);
  });

  it("deactivate_disposes_everything_even_when_a_disposal_throws", async () => {
    const host = await testHost();
    const later = vi.fn();
    host.loader.activate(
      definePlugin({
        name: "tools",
        activate(editor) {
          editor.commands.register({ id: "tools.first", title: "First", run: () => {} });
          host.registry.own(() => {
            throw new Error("disposal failed");
          });
          editor.commands.register({ id: "tools.last", title: "Last", run: () => {} });
          return () => {
            later();
            throw new Error("cleanup failed");
          };
        },
      }),
    );

    const failure = (() => {
      try {
        host.loader.deactivate("tools");
      } catch (error) {
        return error;
      }
    })();

    expect(failure).toBeInstanceOf(AggregateError);
    expect((failure as AggregateError).errors.map(String)).toEqual(["Error: disposal failed", "Error: cleanup failed"]);
    expect(later).toHaveBeenCalledOnce();
    expect(host.editor.commands.list()).toEqual([]);
    host.loader.activate(definePlugin({ name: "tools", activate: (editor) => void editor.commands.register({ id: "tools.first", title: "First", run: () => {} }) }));
  });

  it("cleanup_failures_around_activation_and_reload_are_reported_in_the_editor_console", async () => {
    const leaky = (name: string, failure: string) =>
      definePlugin({
        name,
        activate: () => () => {
          throw new Error(failure);
        },
      });
    const host = await testHost(async () => ({ default: definePlugin({ name: "tools", activate() {} }) }));
    host.loader.activate(leaky("tools", "the old panel would not close"));

    await host.loader.reload("tools", "/plugins/tools.2.js");
    expect(() =>
      host.loader.activate(
        definePlugin({
          name: "broken",
          activate() {
            host.registry.own(() => {
              throw new Error("the half-made panel would not close");
            });
            throw new Error("broken cannot start");
          },
        }),
      ),
    ).toThrow("broken cannot start");

    expect(logged(host)).toEqual([
      ["error", 'Deactivating the previous plugin "tools" failed: plugin "tools" did not deactivate cleanly: the old panel would not close', undefined],
      ["error", 'Cleaning up the failed activation of plugin "broken" failed: plugin "broken" did not deactivate cleanly: the half-made panel would not close', undefined],
    ]);
  });

  it("registrations_after_activate_belong_to_the_editor_until_disposed", async () => {
    const host = await testHost();
    let late: { dispose(): void } | undefined;
    host.loader.activate(
      definePlugin({
        name: "tools",
        activate(editor) {
          queueMicrotask(() => (late = editor.commands.register({ id: "tools.late", title: "Late", run: () => {} })));
          return () => late?.dispose();
        },
      }),
    );
    await Promise.resolve();
    expect(() => host.editor.commands.register({ id: "tools.late", title: "Again", run: () => {} })).toThrow(DuplicateRegistration);
    expect(() => host.editor.commands.register({ id: "tools.late", title: "Again", run: () => {} })).toThrow(/by "editor"/);

    // Only the cleanup the plugin returned releases it.
    host.loader.deactivate("tools");
    expect(host.editor.commands.list()).toEqual([]);
  });

  it("a_project_plugin_registers_an_asset_inspector_and_reads_texture_frames", async () => {
    const meta = { pixels_per_unit: 16, grid: { frame_width: 8, frame_height: 8, margin: 0, spacing: 0 }, pivot: null, filter: "Nearest" } as const;
    const entries = [
      { path: "assets/hero.png", id: "0000000000000000000000000000000d", kind: { Texture: meta } },
      { path: "assets/broken.png", id: "00000000000000000000000000000000", kind: { Texture: { ...meta, grid: null } } },
      { path: "scenes/main.scene.ron", id: "0000000000000000000000000000000a", kind: "Scene" },
    ];
    const frames = { width: 16, height: 8, frames: [{ x: 0, y: 0, width: 8, height: 8, pivot: [4, 4] as [number, number] }, { x: 8, y: 0, width: 8, height: 8, pivot: [4, 4] as [number, number] }] };
    const cut: { png: string; meta: unknown }[] = [];
    const HeroInspector = () => createElement("p", null, "hero");
    let read: Promise<unknown> | undefined;
    const plugin = definePlugin({
      name: "my_game",
      activate(editor) {
        editor.inspectors.registerAsset("Texture", HeroInspector);
        read = editor.textures.frames("assets/hero.png");
      },
    });
    const host = await testHost(
      async () => ({ default: plugin }),
      (request) => {
        if (request.type === "list_files") return { type: "files", entries };
        if (request.type === "read_binary") return { type: "bytes", bytes: btoa(`png of ${request.path}`) };
        return undefined;
      },
      async (png, meta) => {
        cut.push({ png: new TextDecoder().decode(png), meta });
        if (cut.length > 1) throw new Error("cannot decode the PNG: invalid signature");
        return frames;
      },
    );
    await host.loader.load("/plugins/my_game.1.js");

    expect(host.editor.inspectors.getAsset("Texture")).toBe(HeroInspector);
    expect(host.editor.pluginOf(HeroInspector)).toBe("my_game");
    // The frames come with the PNG they were cut from, read once.
    const { png, ...cutFrames } = (await read) as { png: Uint8Array };
    expect([cutFrames, new TextDecoder().decode(png)]).toEqual([frames, "png of assets/hero.png"]);
    expect(cut).toEqual([{ png: "png of assets/hero.png", meta }]);
    expect(new TextDecoder().decode(await host.editor.textures.bytes("assets/hero.png"))).toBe("png of assets/hero.png");
    // A refusal is the renderer's own message; a file that is no readable texture says why.
    await expect(host.editor.textures.frames("assets/hero.png")).rejects.toThrow(/^cannot decode the PNG: invalid signature$/);
    await expect(host.editor.textures.frames("scenes/main.scene.ron")).rejects.toThrow("scenes/main.scene.ron is not a texture");
    await expect(host.editor.textures.frames("assets/broken.png")).rejects.toThrow("assets/broken.png: its meta cannot be read; the project's warning names the cause");
    await expect(host.editor.textures.frames("assets/gone.png")).rejects.toThrow("no project file is at assets/gone.png");

    // The textures whose meta can be read, from the listing `frames` started, and followed.
    const changes: number[] = [];
    const stop = host.editor.textures.subscribe(() => changes.push(host.editor.textures.list().length));
    await settle();
    expect(host.editor.textures.list()).toEqual([{ id: "0000000000000000000000000000000d", path: "assets/hero.png", meta }]);
    expect(host.editor.textures.find("0000000000000000000000000000000d")).toBe(host.editor.textures.list()[0]);
    expect(host.editor.textures.find("00000000000000000000000000000000")).toBe("unreadable");
    expect(host.editor.textures.find("0000000000000000000000000000000a")).toBe("missing");
    entries.splice(0, 1);
    host.pushProject({ type: "files_changed", changes: [{ Removed: { path: "assets/hero.png", kind: { Texture: meta } } }] });
    await settle();
    expect(changes).toEqual([0]);
    // A change to no texture lists nothing again.
    host.pushProject({ type: "files_changed", changes: [{ Changed: { path: "scenes/main.scene.ron", kind: "Scene" } }] });
    await settle();
    expect(changes).toEqual([0]);
    stop();

    host.loader.deactivate("my_game");
    expect(host.editor.inspectors.getAsset("Texture")).toBeUndefined();
  });

  it("an_editor_without_a_texture_cutter_says_so", async () => {
    const host = await testHost(undefined, (request) => {
      if (request.type === "list_files") return { type: "files", entries: [{ path: "assets/hero.png", id: "0000000000000000000000000000000d", kind: { Texture: { pixels_per_unit: 16, grid: null, pivot: null, filter: "Nearest" } } }] };
      if (request.type === "read_binary") return { type: "bytes", bytes: "" };
      return undefined;
    });
    await expect(host.editor.textures.frames("assets/hero.png")).rejects.toThrow("this editor cannot cut textures into frames: it was created without `textureFrames`");
  });

  it("reading_the_texture_list_requests_nothing_until_someone_subscribes", async () => {
    const requests: string[] = [];
    const host = await testHost(undefined, (request) => {
      requests.push(request.type);
      return request.type === "list_files" ? { type: "files", entries: [] } : undefined;
    });

    // Snapshot getters, as `useSyncExternalStore` calls them while rendering: no side effect.
    expect(host.editor.textures.list()).toEqual([]);
    expect(host.editor.textures.find("0000000000000000000000000000000d")).toBe("listing");
    await settle();
    expect(requests).toEqual([]);

    const stop = host.editor.textures.subscribe(() => {});
    await settle();
    expect(requests).toEqual(["list_files"]);
    stop();
  });

  it("a_failed_texture_listing_is_shown_and_tried_again", async () => {
    const meta = { pixels_per_unit: 16, grid: null, pivot: null, filter: "Nearest" } as const;
    let failures = 1;
    const host = await testHost(
      undefined,
      (request) => {
        if (request.type === "list_files") {
          if (failures-- > 0) return { error: { code: "io", message: "the project folder is gone" } };
          return { type: "files", entries: [{ path: "assets/hero.png", id: "0000000000000000000000000000000d", kind: { Texture: meta } }] };
        }
        if (request.type === "read_binary") return { type: "bytes", bytes: btoa("png") };
        return undefined;
      },
      async () => ({ width: 1, height: 1, frames: [] }),
    );

    await expect(host.editor.textures.frames("assets/hero.png")).rejects.toThrow("the project folder is gone");
    // The failure, not a texture that is missing.
    expect(host.editor.textures.find("0000000000000000000000000000000d")).toEqual({ failed: "the project folder is gone" });
    // The same answer each time, as a snapshot getter must give.
    expect(host.editor.textures.find("0000000000000000000000000000000d")).toBe(host.editor.textures.find("0000000000000000000000000000000a"));
    // The next call lists again.
    await expect(host.editor.textures.frames("assets/hero.png")).resolves.toMatchObject({ width: 1, height: 1 });
    expect(host.editor.textures.find("0000000000000000000000000000000d")).toMatchObject({ path: "assets/hero.png" });
  });
  it("a_project_plugin_registers_a_viewport_tool_and_edits", async () => {
    const tool: ViewportTool = { id: "my_game.paint", title: "Paint", available: (editor) => editor.control !== null };
    const edited: string[] = [];
    const plugin = definePlugin({
      name: "my_game",
      activate(editor) {
        editor.viewport.tools.register(tool);
        editor.onEdited((command) => edited.push(command.type));
      },
    });
    const host = await testHost(async () => ({ default: plugin }));
    // A listener that throws is reported; the edit stands and the other listeners hear it.
    host.loader.activate(
      definePlugin({
        name: "broken",
        activate(editor) {
          editor.onEdited(() => {
            throw new Error("no score panel");
          });
        },
      }),
    );
    await host.loader.load("/plugins/my_game.1.js");
    expect(host.editor.viewport.tools.list()).toEqual([tool]);
    expect(host.editor.pluginOf(tool)).toBe("my_game");
    expect(() => host.editor.viewport.tools.register({ ...tool })).toThrow(DuplicateRegistration);

    // Without a game the tool cannot be used and nothing can be edited.
    expect(host.editor.viewport.activeTool.getState()).toBeNull();
    expect(() => host.editor.viewport.setTool("my_game.paint")).toThrow('the viewport tool "my_game.paint" is not available now');
    expect(() => host.editor.viewport.setTool("my_game.erase")).toThrow('no viewport tool "my_game.erase" is registered');
    await expect(host.editor.edit({ type: "undo" })).rejects.toThrow("no game is running");
    // Only edits type-check: a query or the document's own scene reload is no edit.
    // @ts-expect-error `get_hash` edits nothing
    await expect(host.editor.edit({ type: "get_hash" })).rejects.toThrow("no game is running");
    // @ts-expect-error `replace_scene` loads saved files, which the document store sends
    await expect(host.editor.edit({ type: "replace_scene", scene: "", assets: [] })).rejects.toThrow("no game is running");

    const game = await testGame((command) => {
      if (command.type === "get_timeline") return timeline();
      if (command.type === "spawn") return { type: "spawned", entity };
      if (command.type === "redo") return { error: { code: "invalid_command", message: "nothing to redo" } };
      return { type: "done" };
    });
    act(() => host.setControl(game.control));
    act(() => host.editor.viewport.setTool("my_game.paint"));
    expect(host.editor.viewport.activeTool.getState()).toBe("my_game.paint");

    const score = (value: number): EditCommand => ({ type: "set_field", target: { type: "resource", resource: "demo::Score" }, path: [], value });
    const lives: EditCommand = { type: "set_field", target: { type: "resource", resource: "demo::Lives" }, path: [], value: 3 };
    // Each edit resolves with the game's reply: a spawn gives the entity it made.
    const spawned = await host.editor.edit({ type: "spawn", components: {} });
    expect(spawned.entity).toEqual(entity);
    expect(await host.editor.edit(score(1))).toEqual({ type: "done" });
    expect(await host.editor.batched([{ type: "spawn", components: {} }])).toEqual({ type: "spawned", entity });
    expect(await host.editor.batched([score(3), lives])).toEqual({ type: "done" });
    // Nothing to send: the reply an empty batch gets.
    expect(await host.editor.batched([])).toEqual({ type: "done" });
    await expect(host.editor.edit({ type: "redo" })).rejects.toThrow("nothing to redo");
    await settle();
    const edits = game.sent.filter((command) => !["get_timeline", "get_hash"].includes(command.type));
    // One command is sent alone, several as one batch, none not at all.
    expect(edits).toEqual([{ type: "spawn", components: {} }, score(1), { type: "spawn", components: {} }, { type: "batch", commands: [score(3), lives] }, { type: "redo" }]);
    // Listeners hear the accepted edits only; every edit sent, refused or not, refreshes the timeline.
    expect(edited).toEqual(["spawn", "set_field", "spawn", "batch"]);
    expect(logged(host).filter(([level]) => level === "error").map(([, text]) => text)).toEqual([
      'Reacting to the edit "spawn" failed: no score panel',
      'Reacting to the edit "set_field" failed: no score panel',
      'Reacting to the edit "spawn" failed: no score panel',
      'Reacting to the edit "batch" failed: no score panel',
    ]);
    expect(game.sent.filter((command) => command.type === "get_timeline")).toHaveLength(6);

    act(() => host.editor.timeline.setState({ timeline: { ...timeline(), resimulating: true } }));
    await expect(host.editor.edit({ type: "undo" })).rejects.toThrow("the game is resimulating; edit once it finishes");

    // Deactivating the plugin removes its tool, which stops being the active one, and its listener.
    act(() => host.editor.timeline.setState({ timeline: { ...timeline(), resimulating: false } }));
    act(() => host.loader.deactivate("my_game"));
    expect(host.editor.viewport.tools.list()).toEqual([]);
    expect(host.editor.viewport.activeTool.getState()).toBeNull();
    await host.editor.edit({ type: "undo" });
    expect(edited).toHaveLength(4);
  });
});
