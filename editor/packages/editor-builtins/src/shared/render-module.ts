/// <reference types="vite/client" />
import type * as Renderer from "@ludenea/render-wasm";
// The renderer's JavaScript and WebAssembly as files of their own, never bundled: each
// instance of the module is loaded from them.
import glue from "@ludenea/render-wasm/ludenea_render.js?url";
import binary from "@ludenea/render-wasm/ludenea_render_bg.wasm?url";

export type RenderModule = typeof Renderer;

let instances = 0;

/**
 * A new instance of the renderer's module: its JavaScript imported under a URL of its own (the
 * query names the instance, which servers ignore), so a module record of its own with a new
 * WebAssembly instance. The glue keeps its WebAssembly instance in module scope and loads it
 * only once, so a module aborted by a panic can only be replaced this way.
 *
 * Its limit: an ES module record cannot be evicted, so each aborted instance and its WebAssembly
 * memory stay alive until the page reloads, one per panic the user retries after.
 */
export async function instantiate(): Promise<RenderModule> {
  const url = new URL(glue, document.baseURI);
  url.searchParams.set("instance", String(++instances));
  const module = (await import(/* @vite-ignore */ url.href)) as RenderModule;
  await module.default({ module_or_path: binary });
  return module;
}
