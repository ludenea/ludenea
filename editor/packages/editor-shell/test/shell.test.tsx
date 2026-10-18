import { act, cleanup, render } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Shell } from "../src/app";
import { testHost } from "./support";

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

describe("shell", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
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
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    host.editor.commands.register({ id: "panels.open.first", title: "Taken", run() {} });
    render(<Shell host={host} />);
    await act(() => Promise.resolve());
    // Registered after the dock is ready: synced in a microtask.
    host.editor.panels.register({ id: "first", title: "First", component: () => <p>first</p> });
    host.editor.panels.register({ id: "second", title: "Second", component: () => <p>second</p> });
    await act(() => Promise.resolve());

    expect(error).toHaveBeenCalledWith(expect.stringContaining('"panels.open.first"'));
    expect(host.editor.commands.list().map(({ id }) => id)).toContain("panels.open.second");

    // Later syncs try again, without repeating the report.
    host.editor.panels.register({ id: "third", title: "Third", component: () => <p>third</p> });
    await act(() => Promise.resolve());
    expect(error).toHaveBeenCalledTimes(1);
  });
});
