import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Shell, definePlugin, type Editor } from "../src";
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
      ["panels.open.tools.panel", "Open tools"],
      ["panels.open.outline.panel", "Open outline"],
    ]);

    await act(() => host.editor.commands.run("panels.open.tools.panel"));
    expect(screen.getByText("tools content")).toBeTruthy();
    host.loader.deactivate("outline");
    await act(async () => {});
    expect(host.editor.commands.list().map((command) => command.id)).toEqual(["panels.open.tools.panel"]);
  });
});
