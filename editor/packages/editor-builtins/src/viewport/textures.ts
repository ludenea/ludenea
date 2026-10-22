import type { Editor, TextureInfo } from "@ludenea/editor-shell";
import { errorText } from "../shared";

/** The project's textures, as the renderer's `set_textures` takes them: `pngs[i]` is `sources[i]`'s. */
export interface LoadedTextures {
  sources: TextureInfo[];
  pngs: Uint8Array[];
}

/**
 * `current` brought to the latest listing of `editor.textures`, or null while it has none (still
 * listing, or failed: the access reports it, and the viewport keeps what it holds).
 *
 * A texture whose object is the one held is kept as is. A new one is read with
 * `editor.textures.bytes`; one that cannot be read (removed meanwhile) is left out and `warn`
 * hears `<path>: <cause>`. A new version of a held texture must be cut by
 * `editor.textures.frames`, or the previous version stays and `warn` hears why. `loaded` hears
 * the path of each texture read anew. A held texture the listing leaves out for an
 * unreadable meta stays too (the project warns about it itself); one no longer listed goes.
 */
export async function updateTextures(current: LoadedTextures, editor: Editor, warn: (text: string) => void, loaded: (path: string) => void): Promise<LoadedTextures | null> {
  const status = editor.textures.status();
  if (status === "listing" || "failed" in status) return null;
  const listed = editor.textures.list();
  const held = new Map(current.sources.map((source, index) => [source.path, { source, png: current.pngs[index]! }]));
  const next: LoadedTextures = { sources: [], pngs: [] };
  const keep = (source: TextureInfo, png: Uint8Array) => {
    next.sources.push(source);
    next.pngs.push(png);
  };
  for (const texture of listed) {
    const before = held.get(texture.path);
    if (before?.source === texture) {
      keep(before.source, before.png);
      continue;
    }
    try {
      const png = before ? (await editor.textures.frames(texture.path)).png : await editor.textures.bytes(texture.path);
      loaded(texture.path);
      keep(texture, png);
    } catch (error) {
      warn(`${texture.path}: ${errorText(error)}`);
      if (before) keep(before.source, before.png);
    }
  }
  for (const path of status.unreadable) {
    const before = held.get(path);
    if (before) keep(before.source, before.png);
  }
  return next;
}
