import type { PluginDefinition } from "@ludenea/editor-shell";
import { assetsPlugin } from "./assets";
import { consolePlugin } from "./console";
import { projectPlugin } from "./project";
import { sceneTreePlugin } from "./scene-tree";

/** The editor's own panels, plugins on the public `Editor` API like any project plugin. */
export const builtinPlugins: PluginDefinition[] = [projectPlugin, assetsPlugin, sceneTreePlugin, consolePlugin];
