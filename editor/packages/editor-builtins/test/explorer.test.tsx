import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, settle, testEditor, testGame } from "./support";

const UNKNOWN = 'no component is named `Bdy`; registered components: ["ludenea::Body", "ludenea::Pose"]';
const INTROSPECTION = { type: "introspection" };

// jsdom has no layout: give the list a height so the virtualizer shows rows.
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 400 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 300 });
});

async function explorer(total = 3, project?: (request: { type: string }) => object | undefined, failSubscribe = false) {
  const host = await testEditor(builtinPlugins, project);
  const game = await testGame((command) => {
    if (failSubscribe && command.type === "subscribe") return { error: { code: "unknown_topic", message: "introspection is off" } };
    const filter = (command.filter ?? (command.topic as { filter?: string } | undefined)?.filter) as string | undefined;
    if (filter === "With<Bdy>") return { error: { code: "invalid_filter", message: UNKNOWN } };
    if (command.type === "query_entities") {
      const offset = command.offset as number;
      const count = Math.max(0, Math.min(command.limit as number, total - offset));
      const rows = Array.from({ length: count }, (_, row) => ({ entity: { index: offset + row, generation: 0 }, hash: (offset + row).toString(16).padStart(16, "0") }));
      return { type: "entities", total, rows };
    }
    if (command.type === "get_schedule") {
      return { type: "schedule", systems: [{ name: "physics_step", reads: ["component ludenea::Collider"], writes: ["component ludenea::Pose"], micros: null }] };
    }
    if (command.type === "get_tick_events") return { type: "tick_events", events: [] };
    if (command.type === "get_tick_commands") return { type: "tick_commands", sync_points: [] };
    return { type: "done" };
  });
  act(() => host.setControl(game.client));
  renderPanel(host, "ludenea.explorer");
  await settle();
  return { host, game };
}

describe("explorer", () => {
  afterEach(cleanup);

  it("filter_error_shows_candidates", async () => {
    const { game } = await explorer();
    const query = screen.getByRole("searchbox", { name: "Entity query" });
    fireEvent.change(query, { target: { value: "With<Bdy>" } });
    fireEvent.submit(query);
    await settle();

    expect(game.sent).toContainEqual({ type: "query_entities", filter: "With<Bdy>", offset: 0, limit: 200 });
    expect(screen.getByRole("alert").textContent).toContain('"ludenea::Body", "ludenea::Pose"');
    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });

  it("a_column_that_notifies_renders_its_new_values_in_place", async () => {
    const { host } = await explorer();
    const listeners = new Set<() => void>();
    let tick = "10";
    act(() => {
      host.editor.explorer.columns.register({ id: "test.first", title: "First", width: 40, value: () => "a" });
      host.editor.explorer.columns.register({
        id: "test.tick",
        title: "Tick",
        width: 40,
        value: () => tick,
        subscribe: (listener) => {
          listeners.add(listener);
          return () => listeners.delete(listener);
        },
      });
    });
    await settle();
    expect(screen.getAllByRole("option")[0]!.textContent).toContain("a10");

    tick = "20";
    act(() => listeners.forEach((listener) => listener()));

    expect(screen.getAllByRole("option")[0]!.textContent).toContain("a20");
    expect(screen.getAllByRole("columnheader").map((header) => header.textContent)).toEqual(["Entity", "Diverged at", "First", "Tick"]);
    cleanup();
    expect(listeners.size).toBe(0);
  });

  it("list_renders_only_visible_rows_for_100000_entities", async () => {
    await explorer(100_000);
    const rows = screen.getAllByRole("option");
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.length).toBeLessThan(100);
    expect(rows[0]!.textContent).toContain("0v0");
    expect(rows[0]!.textContent).toContain("0000000000000000");
  });

  it("introspection_subscribed_only_while_tab_visible", async () => {
    const { game } = await explorer();
    const sent = () => game.sent.filter((message) => JSON.stringify(message.topic) === JSON.stringify(INTROSPECTION)).map((message) => message.type);
    expect(sent()).toEqual([]);

    fireEvent.click(screen.getByRole("tab", { name: "Schedule" }));
    await settle();
    expect(sent()).toEqual(["subscribe"]);
    expect(screen.getByText("physics_step")).toBeTruthy();
    expect(screen.getByText("not measured")).toBeTruthy();

    fireEvent.click(screen.getByRole("tab", { name: "Events" }));
    await settle();
    expect(sent()).toEqual(["subscribe"]);

    fireEvent.click(screen.getByRole("tab", { name: "Entities" }));
    await settle();
    expect(sent()).toEqual(["subscribe", "unsubscribe"]);
  });

  it("scroll_position_survives_hiding_the_panel", async () => {
    const { host } = await explorer(1000);
    const list = () => document.querySelector(".ludenea-entity-list") as HTMLElement;
    list().scrollTop = 2200;
    fireEvent.scroll(list());
    await settle();

    cleanup();
    renderPanel(host, "ludenea.explorer");
    await settle();
    expect(list().scrollTop).toBe(2200);
    // A browser reports the restored position with a scroll event; jsdom does not.
    fireEvent.scroll(list());
    await settle();
    // Row 100 is at the top, after ten rows of overscan.
    expect(screen.getAllByRole("option")[0]!.textContent).toMatch(/^90v0 /);
  });

  it("timings_say_why_the_browser_has_none", async () => {
    const info = { type: "info", name: "demo", root: "/demo", default_scene: null, mode: "browser" };
    await explorer(3, (request) => (request.type === "info" ? info : undefined));
    fireEvent.click(screen.getByRole("tab", { name: "Schedule" }));
    await settle();
    expect(screen.getByText("not measured in the browser")).toBeTruthy();
  });

  it("introspection_subscribe_failure_is_shown", async () => {
    await explorer(3, undefined, true);
    fireEvent.click(screen.getByRole("tab", { name: "Schedule" }));
    await settle();
    expect(screen.getByRole("alert").textContent).toContain("introspection is off");
  });
});
