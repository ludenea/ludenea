// @vitest-environment jsdom
import { ProjectClient, fakeTransport, type PluginBundle } from "@ludenea/control-client";
import { expect, it, vi } from "vitest";
import { startEditor } from "../src/app";

/** A plugin module whose definition is named `name` and counts its activations on `globalThis`. */
const pluginModule = (name: string, version: number) =>
  `data:text/javascript,export default { name: ${JSON.stringify(name)}, activate() { globalThis.activations.push(${JSON.stringify(`${name} v${version}`)}); } };`;

it("plugin_rebuilt_reloads_the_plugin_its_bundle_activated", async () => {
  // jsdom has no `ResizeObserver`, which the dock needs, and no canvas, which the viewport tries.
  HTMLCanvasElement.prototype.getContext = () => null;
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  document.body.innerHTML = '<div id="root"></div>';
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
