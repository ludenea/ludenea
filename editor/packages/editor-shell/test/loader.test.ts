import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DuplicateRegistration, definePlugin, type Editor } from "../src";
import { Shell } from "../src/app";
import { testHost } from "./support";

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
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
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
    expect(error).toHaveBeenCalledWith(expect.stringContaining("tools"), expect.objectContaining({ message: "tools one cannot come back" }));
    error.mockRestore();
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
});
