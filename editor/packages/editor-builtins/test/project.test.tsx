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

  describe("start document", () => {
    const MAIN = "0000000000000000000000000000000a";
    const OTHER = "0000000000000000000000000000000b";
    const template = { ...info, default_scene: "scenes/main.scene.ron" };
    const files = [
      { path: "scenes/main.scene.ron", id: MAIN, kind: "Scene" },
      { path: "scenes/other.scene.ron", id: OTHER, kind: "Scene" },
    ];
    async function started(projectInfo: object, entries = files, opened: object = projectInfo) {
      const host = await testEditor(builtinPlugins, (request) => {
        if (request.type === "info") return projectInfo;
        if (request.type === "open") return opened;
        if (request.type === "list_files") return { type: "files", entries };
        if (request.type === "read_text") return { type: "text", text: `(${request.path as string})` };
        if (request.type === "read_asset_texts") return { type: "asset_texts", assets: [], paths: [] };
        if (request.type === "load_stored_session") return { type: "stored_session", bytes: null };
        return { type: "done" };
      });
      renderPanel(host, "ludenea.console");
      await settle();
      await settle();
      return host;
    }

    it("a_project_opens_its_default_scene", async () => {
      const host = await started(template);
      expect(host.editor.document.getState()).toMatchObject({ current: { id: MAIN, kind: "scene", path: "scenes/main.scene.ron" }, text: "(scenes/main.scene.ron)" });
    });

    it("a_project_reopens_the_document_last_open_in_it", async () => {
      const first = await started(template);
      await act(() => first.editor.document.getState().open(OTHER));
      cleanup();

      const second = await started(template);
      expect(second.editor.document.getState().current?.id).toBe(OTHER);
    });

    it("a_missing_default_scene_opens_nothing_and_says_so", async () => {
      const host = await started(template, files.slice(1));
      expect(host.editor.document.getState().current).toBeNull();
      expect(screen.getByText("No document opened for demo: its default scene scenes/main.scene.ron is missing.")).toBeTruthy();
    });

    it("another_project_opens_its_own_start_document", async () => {
      const other = { ...template, root: "/work/other", name: "other", default_scene: "scenes/other.scene.ron" };
      const host = await started(template, files, other);
      renderPanel(host, "ludenea.project");
      await act(() => host.editor.commands.run("project.open"));
      fireEvent.change(screen.getByRole("textbox", { name: "Project folder" }), { target: { value: "/work/other" } });
      fireEvent.click(screen.getByRole("button", { name: "Open" }));
      await settle();
      await settle();
      expect(host.editor.document.getState().current?.id).toBe(OTHER);
    });
  });

  it("failed_build_request_leaves_building_state", async () => {
    const failure = { error: { code: "no_project", message: "no project is open" } };
    const host = await testEditor(builtinPlugins, (request) => (request.type === "build" ? failure : undefined));
    renderPanel(host, "ludenea.project");

    await act(() => expect(host.editor.commands.run("project.build")).rejects.toThrow("no project is open"));

    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.getByText(/Build failed/)).toBeTruthy();
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
