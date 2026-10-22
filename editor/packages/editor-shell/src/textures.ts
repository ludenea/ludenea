import { fromBase64, type AssetId, type FileChange, type FileEntry, type ProjectClient, type TextureMeta } from "@ludenea/control-client";
import type { EditorConsole } from "./console";
import type { Observed } from "./editor";
import type { TextureCut, TextureFrames, TextureInfo } from "./plugin";

/**
 * The project's textures: `list` gives those whose meta can be read, listed once a first
 * listener subscribes or `frames` is called (empty until then), then again when the project
 * opens or a texture changes on disk. A texture keeps its object across listings until its
 * file or meta changes, so a reader comparing objects sees which textures changed.
 */
export interface TextureAccess extends Observed<TextureInfo> {
  /**
   * The texture whose identifier is `id` in the latest listing, `"unreadable"` when it names
   * textures whose meta cannot be read, `"missing"` when it names none, `"listing"` before the
   * first listing answers, or `{ failed }` with the reason the latest listing failed (the next
   * `subscribe` or `frames` lists again). Pure, like `list`.
   */
  find(id: AssetId): TextureInfo | "unreadable" | "missing" | "listing" | { failed: string };
  /**
   * How the latest listing went: `"listing"` before the first answers, `{ failed }` with the
   * reason it failed (the object `find` gives), or `{ unreadable }`, the paths of the listed
   * textures whose meta cannot be read, which `list` leaves out (the project warns about each
   * once). Pure, like `list`: the same object until the next listing.
   */
  status(): "listing" | { failed: string } | { unreadable: readonly string[] };
  /** The PNG at `path`, as the project holds it now. */
  bytes(path: string): Promise<Uint8Array>;
  /**
   * The texture at `path` of the latest listing, its PNG read once and cut as its meta says;
   * rejects with the renderer's message when it cannot be cut.
   */
  frames(path: string): Promise<TextureCut>;
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

const changedPath = (change: FileChange) => Object.values(change)[0] as { path: string; kind: FileEntry["kind"] };

const refuseToCut: TextureFrameCutter = () => Promise.reject(new Error("this editor cannot cut textures into frames: it was created without `textureFrames`"));

const sameTexture = (a: TextureInfo, b: TextureInfo) => a.id === b.id && a.path === b.path && JSON.stringify(a.meta) === JSON.stringify(b.meta);

export function createTextureAccess(project: ProjectClient, report: EditorConsole["report"], cut: TextureFrameCutter = refuseToCut): TextureAccess {
  let snapshot: TextureInfo[] = [];
  // The latest listing's files, null before the first answer; and the listing awaited by `frames`.
  let entries: FileEntry[] | null = null;
  let listing: Promise<FileEntry[]> | null = null;
  // Whether anyone asked for the textures: only then do project events list them again.
  let started = false;
  // Why the latest listing failed, until one answers: one object, so `find` answers the same one each time.
  let failed: { failed: string } | null = null;
  // The paths of the latest listing's textures whose meta cannot be read: one object per listing.
  let unreadable: { unreadable: readonly string[] } = { unreadable: [] };
  // The paths changed on disk since the last listing answered; null after the project opened (all of them).
  let changed: Set<string> | null = new Set();
  // Counts the listings started: only the latest one's answer is kept.
  let listings = 0;
  const listeners = new Set<() => void>();
  const notify = () => {
    for (const listener of [...listeners]) listener();
  };
  const refresh = () => {
    const number = ++listings;
    const request = project.request({ type: "list_files" }).then(({ entries }) => entries);
    listing = request;
    request.then(
      (listed) => {
        if (number !== listings) return;
        const previous = new Map(snapshot.map((texture) => [texture.path, texture]));
        const kept = (texture: TextureInfo) => {
          const before = previous.get(texture.path);
          return before && changed && !changed.has(texture.path) && sameTexture(before, texture) ? before : texture;
        };
        entries = listed;
        failed = null;
        unreadable = { unreadable: listed.filter((entry) => textureOf(entry) === "unreadable").map((entry) => entry.path) };
        snapshot = listed.flatMap((entry) => {
          const texture = textureOf(entry);
          return texture && texture !== "unreadable" ? [kept(texture)] : [];
        });
        changed = new Set();
        notify();
      },
      (error: unknown) => {
        if (number !== listings) return;
        // Not the previous project's textures, nor the previous version as if it were current.
        // The next `subscribe` or `frames` lists again.
        entries = null;
        listing = null;
        failed = { failed: error instanceof Error ? error.message : String(error) };
        snapshot = [];
        changed = new Set();
        notify();
        report(error, "Listing the project's textures");
      },
    );
  };
  const start = () => {
    started = true;
    if (!listing) refresh();
    return listing!;
  };
  project.on("project_opened", () => {
    if (!started) return;
    changed = null;
    refresh();
  });
  project.on("files_changed", ({ changes }) => {
    if (!started) return;
    // A file that is a texture now, or was one: a texture whose PNG became something else goes.
    const known = new Set(snapshot.map((texture) => texture.path));
    const concerned = changes.map(changedPath).filter(({ path, kind }) => known.has(path) || (typeof kind === "object" && "Texture" in kind));
    if (concerned.length === 0) return;
    for (const { path } of concerned) changed?.add(path);
    refresh();
  });

  const bytes = async (path: string) => fromBase64((await project.request({ type: "read_binary", path })).bytes);
  return {
    // Pure: a snapshot getter that `useSyncExternalStore` calls while rendering.
    list: () => snapshot,
    find(id) {
      if (failed !== null) return failed;
      if (entries === null) return "listing";
      const found = snapshot.find((texture) => texture.id === id);
      if (found) return found;
      return entries.some((entry) => entry.id === id && textureOf(entry) === "unreadable") ? "unreadable" : "missing";
    },
    status: () => failed ?? (entries === null ? "listing" : unreadable),
    subscribe(listener) {
      void start().catch(() => {});
      listeners.add(listener);
      return () => void listeners.delete(listener);
    },
    bytes,
    async frames(path) {
      const listed = await start();
      const entry = listed.find((file) => file.path === path);
      if (!entry) throw new Error(`no project file is at ${path}`);
      const texture = textureOf(entry);
      if (texture === null) throw new Error(`${path} is not a texture`);
      if (texture === "unreadable") throw new Error(`${path}: its meta cannot be read; the project's warning names the cause`);
      const png = await bytes(path);
      return { ...(await cut(png, texture.meta)), png };
    },
  };
}
