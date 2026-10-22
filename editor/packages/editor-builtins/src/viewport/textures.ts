import type { FileChange } from "@ludenea/control-client";
import { textureOf, type Editor, type TextureInfo } from "@ludenea/editor-shell";
import { errorText, textureFrames } from "../shared";

/** The project's textures, as the renderer's `set_textures` takes them: `pngs[i]` is `sources[i]`'s. */
export interface LoadedTextures {
  sources: TextureInfo[];
  pngs: Uint8Array[];
}

/**
 * Every `Texture` asset of the project with its PNG. A texture whose meta cannot be read is
 * left out: the project reports it once. One whose PNG cannot be read (removed meanwhile) is
 * left out too, and `skipped` hears `<path>: <cause>`.
 */
export async function loadTextures(editor: Editor, skipped: (text: string) => void = () => {}): Promise<LoadedTextures> {
  const { entries } = await editor.project.request({ type: "list_files" });
  const textures = entries.flatMap((entry) => {
    const texture = textureOf(entry);
    return texture && texture !== "unreadable" ? [texture] : [];
  });
  const read = await Promise.allSettled(textures.map(({ path }) => editor.textures.bytes(path)));
  const loaded: LoadedTextures = { sources: [], pngs: [] };
  read.forEach((result, index) => {
    const source = textures[index]!;
    if (result.status === "rejected") return skipped(`${source.path}: ${errorText(result.reason)}`);
    loaded.sources.push(source);
    loaded.pngs.push(result.value);
  });
  return loaded;
}

const changeOf = (change: FileChange): { path: string; removed: boolean; texture: boolean } => {
  const [kind, { path, kind: asset }] = Object.entries(change)[0] as [string, { path: string; kind: unknown }];
  return { path, removed: kind === "Removed", texture: typeof asset === "object" && asset !== null && "Texture" in asset };
};

/**
 * `current` after `changes`: the changed and created textures read again, the removed ones (and
 * those that stopped being textures) dropped. A new version that cannot be used (unreadable
 * meta or PNG, or refused by `textureFrames`) leaves the previous one in the set, and `warn`
 * hears `<path>: <cause>` (an unreadable meta is the project's own warning), and `loaded` hears
 * the path of each version that works. Returns `current` itself when no texture is concerned.
 */
export async function reloadChanged(current: LoadedTextures, changes: FileChange[], editor: Editor, warn: (text: string) => void = () => {}, loaded: (path: string) => void = () => {}): Promise<LoadedTextures> {
  const known = (path: string) => current.sources.some((source) => source.path === path);
  const concerned = changes.map(changeOf).filter(({ path, texture }) => texture || known(path));
  if (concerned.length === 0) return current;
  const reread = concerned.filter(({ removed }) => !removed).map(({ path }) => path);
  const { entries } = reread.length > 0 ? await editor.project.request({ type: "list_files" }) : { entries: [] };
  const listed = new Map(entries.map((entry) => [entry.path, entry]));
  const sources = [...current.sources];
  const pngs = [...current.pngs];
  const drop = (path: string) => {
    const index = sources.findIndex((source) => source.path === path);
    if (index < 0) return;
    sources.splice(index, 1);
    pngs.splice(index, 1);
  };
  for (const { path, removed } of concerned) {
    if (removed) {
      drop(path);
      continue;
    }
    const entry = listed.get(path);
    const texture: TextureInfo | "unreadable" | null = entry ? textureOf(entry) : null;
    if (texture === null) {
      drop(path);
      continue;
    }
    // The project warns about an unreadable meta itself, naming the file and the cause: the previous version stays.
    if (texture === "unreadable") continue;
    try {
      const png = await editor.textures.bytes(path);
      await textureFrames(png, texture.meta);
      loaded(path);
      const source = texture;
      const index = sources.findIndex((candidate) => candidate.path === path);
      if (index < 0) {
        sources.push(source);
        pngs.push(png);
      } else {
        sources[index] = source;
        pngs[index] = png;
      }
    } catch (error) {
      warn(`${path}: ${errorText(error)}`);
    }
  }
  return { sources, pngs };
}
