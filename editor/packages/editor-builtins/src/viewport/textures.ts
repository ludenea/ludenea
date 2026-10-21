import type { ProjectClient, TextureMeta } from "@ludenea/control-client";
import { errorText } from "../shared";

/** The project's textures, as the renderer's `set_textures` takes them: `pngs[i]` is `sources[i]`'s. */
export interface LoadedTextures {
  sources: { id: string; path: string; meta: TextureMeta }[];
  pngs: Uint8Array[];
}

/** The id the project gives every file whose meta cannot be read; its own warning names the file. */
const UNREADABLE = /^0+$/;

const decode = (base64: string) => Uint8Array.from(atob(base64), (character) => character.charCodeAt(0));

/**
 * Every `Texture` asset of the project with its PNG. A texture whose meta cannot be read is
 * left out: the project reports it once. One whose PNG cannot be read (removed meanwhile) is
 * left out too, and `skipped` hears `<path>: <cause>`.
 */
export async function loadTextures(project: ProjectClient, skipped: (text: string) => void = () => {}): Promise<LoadedTextures> {
  const { entries } = await project.request({ type: "list_files" });
  const textures = entries.flatMap(({ path, id, kind }) => (typeof kind === "object" && "Texture" in kind && !UNREADABLE.test(id) ? [{ id, path, meta: kind.Texture }] : []));
  const read = await Promise.allSettled(textures.map(async ({ path }) => decode((await project.request({ type: "read_binary", path })).bytes)));
  const loaded: LoadedTextures = { sources: [], pngs: [] };
  read.forEach((result, index) => {
    const source = textures[index]!;
    if (result.status === "rejected") return skipped(`${source.path}: ${errorText(result.reason)}`);
    loaded.sources.push(source);
    loaded.pngs.push(result.value);
  });
  return loaded;
}
