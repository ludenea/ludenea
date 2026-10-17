// @vitest-environment jsdom
import { ProjectClient, fakeTransport, type PluginBundle } from "@ludenea/control-client";
import { expect, it, vi } from "vitest";
import { startEditor } from "../src/app";

/** A plugin module whose definition is named `name` and counts its activations on `globalThis`. */
const pluginModule = (name: string, version: number) =>
  `data:text/javascript,export default { name: ${JSON.stringify(name)}, activate() { globalThis.activations.push(${JSON.stringify(`${name} v${version}`)}); } };`;

/** A page with the root the shell mounts in; jsdom lacks what the dock and viewport probe. */
function page() {
  HTMLCanvasElement.prototype.getContext = () => null;
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  document.body.innerHTML = '<div id="root"></div>';
}

/** A project service answering `list_plugins` with `plugins()`; `push` sends an event. */
async function fakeProject(plugins: () => PluginBundle[]) {
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [JSON.stringify({ type: "welcome", protocol_version: 1 })];
    if (message.request.type === "list_plugins") return [JSON.stringify({ type: "response", id: message.id, result: { ok: { type: "plugins", plugins: plugins() } } })];
    return [];
  });
  const project = await ProjectClient.connect(transport);
  return { project, push: (event: object) => transport.push(JSON.stringify({ type: "event", event })) };
}

const projectBundle = (name: string): PluginBundle => ({ name, url_path: `plugins/${name}.js`, source: { type: "project" } });

it("opening_another_project_swaps_the_project_plugins", async () => {
  page();
  const activations: string[] = [];
  Object.assign(globalThis, { activations });
  let current = "first";
  const { project, push } = await fakeProject(() => [projectBundle(current)]);
  const module = (name: string) =>
    `data:text/javascript,export default { name: ${JSON.stringify(name)}, activate() { globalThis.activations.push("+${name}"); return () => globalThis.activations.push("-${name}"); } };`;
  const app = startEditor(project, (bundle) => module(bundle.name));
  await app.loadPlugins();
  current = "second";
  push({ type: "project_opened", info: { name: "second", root: "/second", default_scene: null, mode: "browser" } });
  await vi.waitFor(() => expect(activations).toEqual(["+first", "-first", "+second"]));
});

it("a_plugin_that_fails_to_load_is_reported_in_the_console_panel", async () => {
  page();
  const { project } = await fakeProject(() => [projectBundle("broken")]);
  const app = startEditor(project, () => "data:text/javascript,export default 42;");
  await app.loadPlugins();
  await vi.waitFor(() => expect(document.body.textContent).toContain("The editor plugin broken failed to load"));
});

it("plugin_rebuilt_reloads_the_plugin_its_bundle_activated", async () => {
  page();
  const activations: string[] = [];
  Object.assign(globalThis, { activations });
  // The crate is `ludenea-physics`; its plugin calls itself `ludenea_physics`.
  const bundle = (version: number): PluginBundle => ({ name: "ludenea-physics", url_path: `plugins/v${version}.js`, source: { type: "engine", crate_name: "ludenea-physics" } });
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [JSON.stringify({ type: "welcome", protocol_version: 1 })];
    if (message.request.type === "list_plugins") return [JSON.stringify({ type: "response", id: message.id, result: { ok: { type: "plugins", plugins: [bundle(1)] } } })];
    return [];
  });
  const project = await ProjectClient.connect(transport);
  const app = startEditor(project, (loaded) => pluginModule("ludenea_physics", Number(loaded.url_path.match(/v(\d)/)![1])));
  const errors = vi.spyOn(console, "error");

  await app.loadPlugins();
  transport.push(JSON.stringify({ type: "event", event: { type: "plugin_rebuilt", ...bundle(2) } }));
  await vi.waitFor(() => expect(activations).toEqual(["ludenea_physics v1", "ludenea_physics v2"]));
  expect(errors).not.toHaveBeenCalled();
});
