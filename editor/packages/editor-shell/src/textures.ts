import type { FileChange, FileEntry, ProjectClient, TextureMeta } from "@ludenea/control-client";
import type { EditorConsole } from "./console";
import type { Observed } from "./editor";
import type { TextureFrames, TextureInfo } from "./plugin";

/**
 * The project's textures: `list` gives those whose meta can be read, listed once a first
 * listener subscribes (empty until then), then again when the project opens or a texture
 * changes on disk. Each listing gives new objects: one held from before is out of date once
 * its listing is replaced.
 */
export interface TextureAccess extends Observed<TextureInfo> {
  /** The PNG at `path`, as the project holds it now. */
  bytes(path: string): Promise<Uint8Array>;
  /** How the meta of the texture at `path` cuts its PNG now; rejects with the renderer's message when it cannot. */
  frames(path: string): Promise<TextureFrames>;
}

/** Cuts `png` into frames as `meta` says, rejecting with the reason it cannot: the renderer's `textureFrames`. */
export type TextureFrameCutter = (png: Uint8Array, meta: TextureMeta) => Promise<TextureFrames>;

/** The identifier the project gives every file whose meta cannot be read; its own warning names the file and the cause. */
const UNREADABLE = /^0+$/;

/**
 * What the listed file `entry` is as a texture: its `TextureInfo`, `"unreadable"` for a texture
 * whose meta cannot be read (the project warns about it once), or null for any other file.
 */
export function textureOf(entry: FileEntry): TextureInfo | "unreadable" | null {
  const { id, path, kind } = entry;
  if (typeof kind !== "object" || !("Texture" in kind)) return null;
  return UNREADABLE.test(id) ? "unreadable" : { id, path, meta: kind.Texture };
}

function decode(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

const changedPath = (change: FileChange) => Object.values(change)[0] as { path: string; kind: FileEntry["kind"] };

const refuseToCut: TextureFrameCutter = () => Promise.reject(new Error("this editor cannot cut textures into frames: it was created without `textureFrames`"));

export function createTextureAccess(project: ProjectClient, report: EditorConsole["report"], cut: TextureFrameCutter = refuseToCut): TextureAccess {
  let snapshot: TextureInfo[] = [];
  let started = false;
  // Counts the listings started: only the latest one's answer is kept.
  let listings = 0;
  const listeners = new Set<() => void>();
  const refresh = () => {
    const listing = ++listings;
    project.request({ type: "list_files" }).then(
      ({ entries }) => {
        if (listing !== listings) return;
        snapshot = entries.flatMap((entry) => {
          const texture = textureOf(entry);
          return texture && texture !== "unreadable" ? [texture] : [];
        });
        for (const listener of [...listeners]) listener();
      },
      (error: unknown) => {
        if (listing !== listings) return;
        // Not the previous project's textures, nor the previous version as if it were current.
        snapshot = [];
        for (const listener of [...listeners]) listener();
        report(error, "Listing the project's textures");
      },
    );
  };
  const start = () => {
    if (started) return;
    started = true;
    refresh();
  };
  project.on("project_opened", () => started && refresh());
  project.on("files_changed", ({ changes }) => {
    if (!started) return;
    // A file that is a texture now, or was one: a texture whose PNG became something else goes.
    const known = new Set(snapshot.map((texture) => texture.path));
    if (changes.map(changedPath).some(({ path, kind }) => known.has(path) || (typeof kind === "object" && "Texture" in kind))) refresh();
  });

  const bytes = async (path: string) => decode((await project.request({ type: "read_binary", path })).bytes);
  return {
    // Pure: a snapshot getter that `useSyncExternalStore` calls while rendering.
    list: () => snapshot,
    subscribe(listener) {
      start();
      listeners.add(listener);
      return () => void listeners.delete(listener);
    },
    bytes,
    async frames(path) {
      const { entries } = await project.request({ type: "list_files" });
      const entry = entries.find((file) => file.path === path);
      if (!entry) throw new Error(`no project file is at ${path}`);
      const texture = textureOf(entry);
      if (texture === null) throw new Error(`${path} is not a texture`);
      if (texture === "unreadable") throw new Error(`${path}: its meta cannot be read; the project's warning names the cause`);
      return cut(await bytes(path), texture.meta);
    },
  };
}
