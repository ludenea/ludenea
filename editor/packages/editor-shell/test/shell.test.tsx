import { act, cleanup, render } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { Shell } from "../src";
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
  afterEach(cleanup);

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
});
