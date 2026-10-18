import type { AssetId, Command, ConsoleLevel, ControlClient, FileChange, ProjectClient } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";
import { AssetFailure, locateAssetError } from "../assets";

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
  /** Stores the game's session as the open document's, as `open` does before loading another. */
  store(): Promise<void>;
}

/** The sessions of documents, as the application's crash recovery and close keep them. */
export interface DocumentSessions {
  /** Exports the session of `control`, tagged with the document open when asked (none while one opens). */
  export(control: ControlClient): Promise<SavedSession>;
  /** Stores `saved` (a fresh export by default) as the open document's session; one tagged with another document is not stored. */
  store(saved?: SavedSession): Promise<void>;
  /** `saved` when it belongs to the open document, else the open document's stored session, if any. */
  own(saved: SavedSession | null): Promise<SavedSession | null>;
}

/** Where the store reports failures: the editor's console, at the asset's file when known. */
export type DocumentLog = (level: ConsoleLevel, text: string, location?: { file: string; line: number; column: number }) => void;

/** A Zustand store: one document at a time, as the game keeps one session. */
export type DocumentStore = UseBoundStore<StoreApi<DocumentState>>;

/** A session as `ExportSession` gave it, ready to load, and the tick the timeline showed then. */
export interface SavedSession {
  load: Extract<Command, { type: "load_session" }>;
  tick: number;
  /** The document open when it was exported; null while one was opening or none was open. */
  document?: AssetId | null;
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
  await returnTo(control, saved, tick);
}

/** Seeks back to `tick` once the loaded `saved` is resimulated that far, within what it recorded. */
async function returnTo(control: ControlClient, saved: SavedSession, tick: number): Promise<void> {
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

const assetKind = (change: FileChange) => Object.values(change)[0]!.kind;
const changedPath = (change: FileChange) => Object.values(change)[0]!.path;

export function createDocumentStore(options: {
  project: ProjectClient;
  control: () => ControlClient | null;
  onGameChanged(listener: (control: ControlClient | null) => void): void;
  log: DocumentLog;
}): { store: DocumentStore; sessions: DocumentSessions } {
  const { project, control, log } = options;
  // The project's asset texts the game last accepted, the file of each, and the open replay.
  let texts: { assets: [AssetId, string][]; paths: [AssetId, string][] } = { assets: [], paths: [] };
  let replay = "";
  // The stored session of a document opened without a game, for the next game.
  let pending: SavedSession | null = null;
  // Exports taken while a document opens may belong to either document: they are tagged with none.
  let opening = false;
  let queue: Promise<unknown> = Promise.resolve();
  // One open or reload at a time, in call order.
  const serial = <T>(run: () => Promise<T>): Promise<T> => {
    const result = queue.then(run);
    queue = result.catch(() => {});
    return result;
  };
  const locatedIn = (paths: [AssetId, string][]) => (error: unknown) => {
    throw locateAssetError(error, paths);
  };
  /** Logs `error` in the console, located in the asset at fault, and rethrows it. */
  const reported = (what: string) => (error: unknown) => {
    const text = `${what}: ${error instanceof Error ? error.message : String(error)}`;
    log("error", text, error instanceof AssetFailure ? { file: error.path, line: error.line, column: error.column } : undefined);
    throw error;
  };

  const readTexts = () => project.request({ type: "read_asset_texts" }).then(({ assets, paths }) => ({ assets, paths }));
  const load = (game: ControlClient, document: OpenDocument, text: string | null, source: { texts: typeof texts; replay: string }): Promise<unknown> => {
    const command: Command =
      document.kind === "replay"
        ? { type: "load_replay", replay: source.replay }
        : { type: "load_scene", scene: text!, assets: source.texts.assets, document: document.kind === "prefab" ? "Prefab" : "Scene" };
    return game.request(command).catch(locatedIn(source.texts.paths));
  };
  const storeIn = (document: OpenDocument, saved: SavedSession) =>
    project.request({ type: "store_session", document: document.id, bytes: encode(saved) }).then(() => {});
  const stored = async (document: OpenDocument): Promise<SavedSession | null> => {
    const { bytes } = await project.request({ type: "load_stored_session", document: document.id });
    return bytes ? decode(bytes) : null;
  };
  // The scene and assets on disk now, so a stored session whose files changed replays against them.
  const refreshed = (saved: SavedSession, text: string | null, assets: [AssetId, string][]): SavedSession => {
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
  const tagged = async (game: ControlClient): Promise<SavedSession> => {
    const document = opening ? null : (store.getState().current?.id ?? null);
    return { ...(await exportSession(game)), document };
  };

  const store: DocumentStore = create<DocumentState>()((set, get) => ({
    current: null,
    text: null,
    open: (id) => {
      // The file once known, for the console.
      let name = `the document ${id}`;
      return serial(async () => {
        const { entries } = await project.request({ type: "list_files" });
        const entry = entries.find((file) => file.id === id);
        if (!entry) throw new Error(`no project file has the identifier ${id}`);
        name = entry.path;
        const kind = typeof entry.kind === "string" ? KINDS[entry.kind] : undefined;
        if (!kind) throw new Error(`${entry.path} is not a scene, a prefab, or a replay`);
        const document = { id, kind, path: entry.path };
        const game = control();
        const previous = get().current;
        if (previous && game) await storeIn(previous, await tagged(game));
        opening = true;
        try {
          const fresh = await readTexts();
          let text: string | null = null;
          let bytes = "";
          if (kind === "replay") ({ bytes } = await project.request({ type: "read_binary", path: entry.path }));
          else ({ text } = await project.request({ type: "read_text", path: entry.path }));
          const saved = await stored(document);
          const restoring = saved && refreshed(saved, text, fresh.assets);
          if (game) await load(game, document, text, { texts: fresh, replay: bytes });
          // Only what the game accepted is kept: a refused document leaves the previous one.
          texts = fresh;
          replay = bytes;
          set({ current: document, text });
          pending = game ? null : restoring;
          if (!game) return;
          if (restoring) await restoreSession(game, restoring).catch(locatedIn(texts.paths));
          await nameRoot(game, document);
        } finally {
          opening = false;
        }
      }).catch((error: unknown) => reported(`Cannot open ${name}`)(error));
    },
    replace: (text) =>
      serial(async () => {
        const fresh = await readTexts();
        await control()?.request({ type: "replace_scene", scene: text, assets: fresh.assets }).catch(locatedIn(fresh.paths));
        texts = fresh;
        set({ text });
      }).catch(reported("Cannot reload the scene")),
    store: () => sessions.store(),
  }));

  // A prefab or data asset changed on disk: the open scene or prefab reloads with it. The
  // session goes back with the new texts as a whole (`ReplaceScene` would mark unsaved scene
  // edits saved): it keeps its edits, history, branches, and unsaved state, and replays.
  // Refused, the game keeps the previous session and the previous texts stay.
  const reloadAssets = async () => {
    const reloaded = await serial(async () => {
      const document = store.getState().current;
      if (document?.kind === "replay") return null;
      const fresh = await readTexts();
      const game = control();
      let saved: SavedSession | null = null;
      if (document && game) {
        const exported = await exportSession(game);
        saved = { ...exported, load: { ...exported.load, assets: fresh.assets } };
        await game.request(saved.load).catch(locatedIn(fresh.paths));
      }
      texts = fresh;
      return game && saved && { game, saved };
    }).catch(reported("Cannot reload the changed assets"));
    // Off the queue: a long resimulation holds up no document change.
    if (reloaded) {
      await returnTo(reloaded.game, reloaded.saved, reloaded.saved.tick).catch((error: unknown) =>
        log("warning", `Cannot go back to tick ${reloaded.saved.tick} after reloading the assets: ${error instanceof Error ? error.message : String(error)}`),
      );
    }
  };
  project.on("files_changed", ({ changes }) => {
    const open = store.getState().current?.path;
    const assetChanged = (change: FileChange) => {
      const kind = assetKind(change);
      return changedPath(change) !== open && (kind === "Prefab" || (typeof kind === "object" && "Data" in kind));
    };
    if (changes.some(assetChanged)) reloadAssets().catch(() => {});
  });

  options.onGameChanged((game) => {
    const document = store.getState().current;
    if (!game || !document) return;
    const saved = pending;
    pending = null;
    // Both sent at once: a session the application restores into this game, and its first
    // export, come after them.
    const loaded = load(game, document, store.getState().text, { texts, replay });
    const restored = saved ? restoreSession(game, saved).catch(locatedIn(texts.paths)) : null;
    Promise.all([loaded, restored])
      .then(() => nameRoot(game, document))
      .catch(reported(`Cannot load ${document.path} into the game`))
      .catch(() => {});
  });

  const sessions: DocumentSessions = {
    export: tagged,
    async store(saved) {
      const document = store.getState().current;
      const game = control();
      if (!document || (!saved && !game)) return;
      saved ??= await tagged(game!);
      if (saved.document === document.id) await storeIn(document, saved);
    },
    async own(saved) {
      const document = store.getState().current;
      if ((saved?.document ?? null) === (document?.id ?? null)) return saved;
      return document && stored(document);
    },
  };
  return { store, sessions };
}
