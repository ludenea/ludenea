import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { builtinPlugins } from "../src";
import { renderPanel, settle, testEditor } from "./support";

const info = { type: "info", name: "demo", root: "/work/demo", default_scene: null, mode: "desktop" };

describe("project", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

  it("new_project_defaults_engine_path_and_is_remembered", async () => {
    const host = await testEditor(builtinPlugins, (request) => (request.type === "create" ? info : request.type === "info" ? undefined : { type: "done" }));
    host.editor.settings.set("ludenea.engine-path", "/engine");
    renderPanel(host, "ludenea.project");

    await act(() => host.editor.commands.run("project.new"));
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "demo" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Parent folder" }), { target: { value: "/work" } });
    expect((screen.getByRole("textbox", { name: "Engine path" }) as HTMLInputElement).value).toBe("/engine");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await settle();

    expect(host.project.sent).toContainEqual({ type: "create", parent: "/work", name: "demo", engine_path: "/engine" });
    expect(host.editor.settings.get("ludenea.recent-projects")).toEqual(["/work/demo"]);
    expect(screen.getByRole("heading").textContent).toBe("demo");
  });

  it("build_progress_shows_building_state", async () => {
    const host = await testEditor(builtinPlugins, (request) => (request.type === "build" ? { type: "done" } : undefined));
    renderPanel(host, "ludenea.project");

    await act(() => host.editor.commands.run("project.build"));
    act(() => host.project.push({ type: "build", event: { type: "progress", done: 3, total: 10 } }));

    expect(host.project.sent).toContainEqual({ type: "build" });
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("3");
    expect(screen.getByText(/Building/)).toBeTruthy();
    act(() => host.project.push({ type: "build", event: { type: "finished", ok: true } }));
    expect(screen.queryByRole("progressbar")).toBeNull();
  });
});
