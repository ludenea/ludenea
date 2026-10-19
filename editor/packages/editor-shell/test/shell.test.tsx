import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Component, useEffect, type ComponentType, type ReactNode } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { definePlugin, PluginBoundary, type Editor } from "../src";
import { mountShell, Shell, type EditorHost } from "../src/app";
import { logged, testHost } from "./support";

// jsdom has neither layout nor `ResizeObserver`; dockview needs both to lay out.
beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 400 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 600 });
});

/** A plugin named `name` adding the panel `id` titled `title`. */
const panelPlugin = (name: string, id: string, title: string, component: ComponentType<{ editor: Editor }>, defaultPosition?: "left") =>
  definePlugin({ name, activate: (editor) => void editor.panels.register({ id, title, component, defaultPosition }) });

describe("shell", () => {
  const mounted: (() => void)[] = [];
  /** Mounts the shell as the applications do, with the error handlers of its root; unmounted after the test. */
  const mount = (host: EditorHost) => {
    const element = document.body.appendChild(document.createElement("div"));
    act(() => {
      const unmount = mountShell(element, host);
      mounted.push(() => {
        unmount();
        element.remove();
      });
    });
  };

  afterEach(() => {
    for (const unmount of mounted.splice(0)) act(unmount);
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("a_panel_that_throws_while_rendering_leaves_the_other_panels_and_the_palette_working", async () => {
    const devtools = vi.spyOn(console, "error");
    const stats = panelPlugin("my_game", "my_game.stats", "Stats", () => <p>stats</p>);
    const host = await testHost(async () => ({ default: stats }));
    host.loader.activate(panelPlugin("notes", "notes.list", "Notes", () => <p>notes</p>, "left"));
    host.loader.activate(
      panelPlugin("my_game", "my_game.stats", "Stats", () => {
        throw new Error("value.kind is undefined");
      }),
    );
    mount(host);
    await act(() => Promise.resolve());

    const failure = 'The "Stats" panel of plugin "my_game" failed: value.kind is undefined';
    // Dockview keeps an empty alert of its own.
    const shown = () => screen.queryByText(failure)?.closest("[role=alert]");
    expect(screen.getByText("notes")).toBeTruthy();
    expect(shown()).toBeTruthy();
    expect(logged(host)).toEqual([["error", failure, undefined]]);
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });

    // Rendered again, it fails again: shown, not reported twice.
    fireEvent.click(screen.getByRole("button", { name: "Reload panel" }));
    expect(shown()).toBeTruthy();
    expect(logged(host)).toHaveLength(1);

    // The plugin rebuilt: its new panel replaces the failure.
    await act(() => host.loader.reload("my_game", "my_game.js"));
    await act(() => Promise.resolve());
    expect(screen.getByText("stats")).toBeTruthy();
    expect(shown()).toBeFalsy();
    expect(screen.getByText("notes")).toBeTruthy();
    expect(devtools).not.toHaveBeenCalled();
  });

  it("a_plugin_isolates_its_own_slots_with_a_plugin_boundary", async () => {
    const devtools = vi.spyOn(console, "error");
    const preview = { asset: "assets/ball.prefab.ron" };
    const Preview = (): ReactNode => {
      throw new Error("no sprite for ball");
    };
    const host = await testHost();
    host.loader.activate(
      panelPlugin("my_game", "my_game.prefabs", "Prefabs", ({ editor }) => (
        <>
          <p>prefab list</p>
          <PluginBoundary editor={editor} registration={preview} name="The preview" fallback={({ text }) => <p>{text}</p>}>
            <Preview />
          </PluginBoundary>
        </>
      )),
    );
    mount(host);
    await act(() => Promise.resolve());

    // Not a registration: the line names the slot alone.
    const failure = "The preview failed: no sprite for ball";
    expect(screen.getByText("prefab list")).toBeTruthy();
    expect(screen.getByText(failure)).toBeTruthy();
    expect(logged(host)).toEqual([["error", failure, undefined]]);
    expect(devtools).not.toHaveBeenCalled();
  });

  it("an_error_that_a_plugins_own_boundary_catches_is_reported_once", async () => {
    const devtools = vi.spyOn(console, "error");
    class Guard extends Component<{ children: ReactNode }, { failed: boolean }> {
      state = { failed: false };
      static getDerivedStateFromError = () => ({ failed: true });
      render = () => (this.state.failed ? <p>no sprite</p> : this.props.children);
    }
    const Sprite = (): ReactNode => {
      throw new Error("missing sprite");
    };
    const host = await testHost();
    host.loader.activate(
      panelPlugin("my_game", "my_game.sprite", "Sprite", () => (
        <Guard>
          <Sprite />
        </Guard>
      )),
    );
    mount(host);
    await act(() => Promise.resolve());

    expect(screen.getByText("no sprite")).toBeTruthy();
    expect(logged(host)).toEqual([["error", "Rendering the editor failed: missing sprite", undefined]]);
    expect(devtools).not.toHaveBeenCalled();
  });

  it("an_error_outside_every_plugin_slot_is_reported_in_place_of_the_window", async () => {
    const devtools = vi.spyOn(console, "error");
    const host = await testHost();
    host.loader.activate(panelPlugin("notes", "notes.list", "Notes", () => <p>notes</p>));
    mount(host);
    await act(() => Promise.resolve());
    vi.spyOn(host.editor.commands, "list").mockImplementation(() => {
      throw new Error("the command list is unreadable");
    });

    // Inside act(), which fireEvent uses, React throws what no boundary catches instead of handing it to the root.
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", false);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }));

    const failure = "Rendering the editor failed: the command list is unreadable";
    await waitFor(() => expect(screen.getByRole("alert").textContent).toBe(`${failure}Reload the editor to continue.`));
    expect(logged(host)).toEqual([["error", failure, undefined]]);
    expect(screen.queryByText("notes")).toBeNull();
    expect(devtools).not.toHaveBeenCalled();
  });

  it("panel_in_a_hidden_tab_is_unmounted", async () => {
    const host = await testHost();
    const mounted = new Set<string>();
    const tracked = (id: string) => () => {
      useEffect(() => {
        mounted.add(id);
        return () => void mounted.delete(id);
      }, []);
      return <p>{id}</p>;
    };
    host.editor.panels.register({ id: "first", title: "First", defaultPosition: "bottom", component: tracked("first") });
    host.editor.panels.register({ id: "second", title: "Second", defaultPosition: "bottom", component: tracked("second") });
    render(<Shell host={host} />);
    await act(() => Promise.resolve());
    // Both share the bottom group: the last opened shows, the other is hidden.
    expect([...mounted]).toEqual(["second"]);

    await act(() => host.editor.commands.run("panels.open.first"));
    expect([...mounted]).toEqual(["first"]);
  });

  it("panel_whose_open_command_is_taken_is_reported_and_still_shown", async () => {
    const host = await testHost();
    const errors = () => host.editor.console.list().filter((entry) => entry.level === "error");
    const taker = host.editor.commands.register({ id: "panels.open.first", title: "Taken", run() {} });
    render(<Shell host={host} />);
    await act(() => Promise.resolve());
    // Registered after the dock is ready: synced in a microtask.
    const first = host.editor.panels.register({ id: "first", title: "First", component: () => <p>first</p> });
    host.editor.panels.register({ id: "second", title: "Second", component: () => <p>second</p> });
    await act(() => Promise.resolve());

    expect(errors().map((entry) => entry.text)).toEqual([
      'Adding the command that shows panel "first" failed: command "panels.open.first" is already registered by "editor"; "editor" cannot register it again',
    ]);
    expect(host.editor.commands.list().map(({ id }) => id)).toContain("panels.open.second");

    // Later syncs try again, without repeating the report.
    host.editor.panels.register({ id: "third", title: "Third", component: () => <p>third</p> });
    await act(() => Promise.resolve());
    expect(errors()).toHaveLength(1);

    // Once the shell holds the command, a later conflict is news again.
    taker.dispose();
    host.editor.panels.register({ id: "fourth", title: "Fourth", component: () => <p>fourth</p> });
    await act(() => Promise.resolve());
    expect(host.editor.commands.list().map(({ id }) => id)).toContain("panels.open.first");
    first.dispose();
    await act(() => Promise.resolve());
    host.editor.commands.register({ id: "panels.open.first", title: "Taken again", run() {} });
    host.editor.panels.register({ id: "first", title: "First", component: () => <p>first</p> });
    await act(() => Promise.resolve());
    expect(errors()).toHaveLength(2);
  });
});
