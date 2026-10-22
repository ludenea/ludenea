import type { PluginDefinition } from "@ludenea/editor-shell";
import { assetsPlugin } from "./assets";
import { consolePlugin } from "./console";
import { divergencePlugin } from "./divergence";
import { explorerPlugin } from "./explorer";
import { inspectorPlugin } from "./inspector";
import { prefabsPlugin } from "./prefabs";
import { projectPlugin } from "./project";
import { sceneTreePlugin } from "./scene-tree";
import { texturesPlugin } from "./textures";
import { tilemapPlugin } from "./tilemap";
import { timelinePlugin } from "./timeline";
import { viewportPlugin } from "./viewport";

/** The editor's own panels, plugins on the public `Editor` API like any project plugin. */
export const builtinPlugins: PluginDefinition[] = [projectPlugin, assetsPlugin, sceneTreePlugin, prefabsPlugin, explorerPlugin, inspectorPlugin, texturesPlugin, viewportPlugin, tilemapPlugin, timelinePlugin, divergencePlugin, consolePlugin];
