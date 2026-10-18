import type { AssetId, Command, ControlClient, ProjectClient } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";
import { locateAssetError } from "../assets";

/** The file the game's session edits or plays. */
export interface OpenDocument {
  id: AssetId;
  kind: "scene" | "prefab" | "replay";
  /** Relative to the project root. */
  path: string;
}

export interface DocumentState {
  current: OpenDocument | null;
  /** The open scene or prefab as last loaded, replaced, or saved: what each new game loads. Null for a replay. */
  text: string | null;
  /**
   * Stores the current session under `.ludenea/`, then loads the document `id` with the
   * project's asset texts and restores its stored session, if any. Without a game, the next
   * game loads it. A failure keeps the previous document and session.
   */
  open(id: AssetId): Promise<void>;
  /** Replaces the open scene or prefab with `text` (a change on disk), with the project's asset texts: the session replays. */
  replace(text: string): Promise<void>;
}

/** A Zustand store: one document at a time, as the game keeps one session. */
export type DocumentStore = UseBoundStore<StoreApi<DocumentState>>;

/** A session as `ExportSession` gave it, ready to load, and the tick the timeline showed then. */
export interface SavedSession {
  load: Extract<Command, { type: "load_session" }>;
  tick: number;
}

export async function exportSession(control: ControlClient): Promise<SavedSession> {
  const [{ scene, saved_scene, inputs, edits, undo, hashes, assets, branches, document, keeps_assets }, timeline] = await Promise.all([
    control.request({ type: "export_session" }),
    control.request({ type: "get_timeline" }),
  ]);
  return {
    load: { type: "load_session", scene, saved_scene, inputs, edits, undo, expected: hashes, assets, branches, document, keeps_assets },
    tick: timeline.current,
  };
}

/** How long a restore may wait for resimulation before it gives up. */
const RESIMULATION_TIMEOUT_MS = 5 * 60 * 1000;

/**
 * Resolves once the game has simulated `tick` (a seek further than the resimulation fails);
 * rejects if the game goes away, panics, or takes longer than `RESIMULATION_TIMEOUT_MS`.
 */
function simulated(control: ControlClient, tick: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const stops: (() => void)[] = [];
    const settle = (error?: Error) => {
      stops.forEach((stop) => stop());
      clearTimeout(timer);
      if (error) reject(error);
      else resolve();
    };
    const timer = setTimeout(() => settle(new Error(`the game did not resimulate to tick ${tick} in time`)), RESIMULATION_TIMEOUT_MS);
    stops.push(
      control.on("resimulated", ({ up_to }) => {
        if (up_to >= tick) settle();
      }),
      control.on("paused", ({ reason }) => {
        if (reason.type === "panicked") settle(new Error(`the game panicked at tick ${reason.tick} while resimulating: ${reason.message}`));
      }),
      control.onClose((reason) => settle(new Error(`the game stopped while resimulating: ${reason}`))),
    );
    control.request({ type: "get_timeline" }).then((timeline) => {
      if (!timeline.resimulating || timeline.simulated >= tick) settle();
    }, settle);
  });
}

/**
 * Loads `saved` into a game and goes back to `tick` (the saved one by default), within what
 * the session recorded. A game whose code, scene, or assets changed reports `Diverged` while
 * it resimulates. Send it after the game's `LoadScene` (which the document store sends to
 * every new game), so the restored session is what stays loaded.
 */
export async function restoreSession(control: ControlClient, saved: SavedSession, tick = saved.tick): Promise<void> {
  await control.request(saved.load);
  const target = Math.max(0, Math.min(tick, saved.load.expected.length - 1));
  await simulated(control, target);
  await control.request({ type: "seek", tick: target });
}

// Stored sessions are the JSON of a `SavedSession`, as base64 bytes.
const encode = (saved: SavedSession): string => {
  let binary = "";
  for (const byte of new TextEncoder().encode(JSON.stringify(saved))) binary += String.fromCharCode(byte);
  return btoa(binary);
};
const decode = (bytes: string): SavedSession => JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(bytes), (char) => char.charCodeAt(0))));

const KINDS: { [kind: string]: OpenDocument["kind"] } = { Scene: "scene", Prefab: "prefab", Replay: "replay" };
/** How the session names its root branch until the editor renames it after the document. */
const ROOT_NAME = "Branch 1";

export function createDocumentStore(options: {
  project: ProjectClient;
  control: () => ControlClient | null;
  onGameChanged(listener: (control: ControlClient | null) => void): void;
}): { store: DocumentStore; storeSession(saved?: SavedSession): Promise<void> } {
  const { project, control } = options;
  // The project's asset texts as last read, and the file of each.
  let assets: [AssetId, string][] = [];
  let paths: [AssetId, string][] = [];
  let replay = "";
  // The stored session of a document opened without a game, for the next game.
  let pending: SavedSession | null = null;
  // Exports taken while a document opens may belong to either document: they are not stored.
  let opening = false;
  let queue: Promise<unknown> = Promise.resolve();
  // One open or replace at a time, in call order.
  const serial = <T>(run: () => Promise<T>): Promise<T> => {
    const result = queue.then(run);
    queue = result.catch(() => {});
    return result;
  };
  const located = (error: unknown) => {
    throw locateAssetError(error, paths);
  };

  const readAssets = async () => {
    ({ assets, paths } = await project.request({ type: "read_asset_texts" }));
  };
  const load = (game: ControlClient, document: OpenDocument, text: string | null): Promise<unknown> => {
    const command: Command =
      document.kind === "replay" ? { type: "load_replay", replay } : { type: "load_scene", scene: text!, assets, document: document.kind === "prefab" ? "Prefab" : "Scene" };
    return game.request(command).catch(located);
  };
  const storeIn = (document: OpenDocument, saved: SavedSession) =>
    project.request({ type: "store_session", document: document.id, bytes: encode(saved) }).then(() => {});
  // The scene and assets on disk now, so a stored session whose files changed replays against them.
  const refreshed = (saved: SavedSession, text: string | null): SavedSession => {
    if (saved.load.keeps_assets) return saved;
    const scene = saved.load.saved_scene === null && text !== null ? text : saved.load.scene;
    return { ...saved, load: { ...saved.load, scene, assets } };
  };
  const nameRoot = async (game: ControlClient, document: OpenDocument) => {
    const timeline = await game.request({ type: "get_timeline" });
    // A host that answers something else (a test double) keeps its names.
    const root = timeline.type === "timeline" ? timeline.branches.find((branch) => branch.parent === null) : undefined;
    const name = document.path.split("/").at(-1)!.split(".")[0]!;
    if (root?.name === ROOT_NAME) await game.request({ type: "rename_branch", branch: root.id, name });
  };
  const store: DocumentStore = create<DocumentState>()((set, get) => ({
    current: null,
    text: null,
    open: (id) =>
      serial(async () => {
        const { entries } = await project.request({ type: "list_files" });
        const entry = entries.find((file) => file.id === id);
        if (!entry) throw new Error(`no project file has the identifier ${id}`);
        const kind = typeof entry.kind === "string" ? KINDS[entry.kind] : undefined;
        if (!kind) throw new Error(`${entry.path} is not a scene, a prefab, or a replay`);
        const document = { id, kind, path: entry.path };
        opening = true;
        try {
          const game = control();
          const previous = get().current;
          if (previous && game) await storeIn(previous, await exportSession(game));
          await readAssets();
          let text: string | null = null;
          if (kind === "replay") ({ bytes: replay } = await project.request({ type: "read_binary", path: entry.path }));
          else ({ text } = await project.request({ type: "read_text", path: entry.path }));
          const { bytes } = await project.request({ type: "load_stored_session", document: id });
          const stored = bytes ? refreshed(decode(bytes), text) : null;
          if (game) await load(game, document, text);
          set({ current: document, text });
          pending = game ? null : stored;
          if (!game) return;
          if (stored) await restoreSession(game, stored).catch(located);
          await nameRoot(game, document);
        } finally {
          opening = false;
        }
      }),
    replace: (text) =>
      serial(async () => {
        await readAssets();
        await control()?.request({ type: "replace_scene", scene: text, assets }).catch(located);
        set({ text });
      }),
  }));

  options.onGameChanged((game) => {
    const document = store.getState().current;
    if (!game || !document) return;
    const saved = pending;
    pending = null;
    // Both sent at once: a session the application restores into this game, and its first
    // export, come after them.
    const loaded = load(game, document, store.getState().text);
    const restored = saved ? restoreSession(game, saved).catch(located) : null;
    Promise.all([loaded, restored])
      .then(() => nameRoot(game, document))
      .catch((error) => console.error(`${document.path} not loaded into the game: ${error instanceof Error ? error.message : String(error)}`));
  });

  return {
    store,
    async storeSession(saved) {
      const document = store.getState().current;
      const game = control();
      if (opening || !document || (!saved && !game)) return;
      await storeIn(document, saved ?? (await exportSession(game!)));
    },
  };
}
