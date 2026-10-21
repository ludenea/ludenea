import type { FileChange, ProjectClient, TextureMeta } from "@ludenea/control-client";
import { errorText, loadRenderModule } from "../shared";

/** The project's textures, as the renderer's `set_textures` takes them: `pngs[i]` is `sources[i]`'s. */
export interface LoadedTextures {
  sources: { id: string; path: string; meta: TextureMeta }[];
  pngs: Uint8Array[];
}

/** The id the project gives every file whose meta cannot be read; its own warning names the file. */
const UNREADABLE = /^0+$/;

function decode(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

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

const changeOf = (change: FileChange): { path: string; removed: boolean; texture: boolean } => {
  const [kind, { path, kind: asset }] = Object.entries(change)[0] as [string, { path: string; kind: unknown }];
  return { path, removed: kind === "Removed", texture: typeof asset === "object" && asset !== null && "Texture" in asset };
};

/**
 * `current` after `changes`: the changed and created textures read again, the removed ones (and
 * those that stopped being textures) dropped. A new version that cannot be used (unreadable
 * meta or PNG, or refused by `textureFrames`) leaves the previous one in the set, and `warn`
 * hears `<path>: <cause>`. Returns `current` itself when no texture is concerned.
 */
export async function reloadChanged(current: LoadedTextures, changes: FileChange[], project: ProjectClient, warn: (text: string) => void = () => {}): Promise<LoadedTextures> {
  const known = (path: string) => current.sources.some((source) => source.path === path);
  const concerned = changes.map(changeOf).filter(({ path, texture }) => texture || known(path));
  if (concerned.length === 0) return current;
  const reread = concerned.filter(({ removed }) => !removed).map(({ path }) => path);
  const { entries } = reread.length > 0 ? await project.request({ type: "list_files" }) : { entries: [] };
  const { textureFrames } = await loadRenderModule();
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
    const entry = entries.find((candidate) => candidate.path === path);
    if (!entry || typeof entry.kind !== "object" || !("Texture" in entry.kind)) {
      drop(path);
      continue;
    }
    if (UNREADABLE.test(entry.id)) {
      warn(`${path}: its meta cannot be read`);
      continue;
    }
    try {
      const png = decode((await project.request({ type: "read_binary", path })).bytes);
      textureFrames(png, JSON.stringify(entry.kind.Texture));
      const source = { id: entry.id, path, meta: entry.kind.Texture };
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
