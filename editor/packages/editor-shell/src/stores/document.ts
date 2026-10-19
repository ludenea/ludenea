import type { AssetId, Command, ControlClient, FileChange, ProjectClient, Reply } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";
import { AssetFailure, locateAssetError } from "../assets";
import type { EditorConsole } from "../console";

/** What the game reports about the scene it loaded: stale override warnings, prefab instances, and load addresses. */
export type Loaded = Extract<Reply, { type: "loaded" }>;

// A host that answers something else (a test double) reports nothing.
const loadedIn = (reply: Reply): Loaded | null => (reply.type === "loaded" ? reply : null);
const differs = (reply: Reply | null): boolean => reply?.type === "replay_loaded" && reply.fingerprint_differs;

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
  /** What the game's last load of the open scene or prefab reported; null for a replay or without a game. */
  loaded: Loaded | null;
  /** The open replay was recorded with other game types (its schema fingerprint differs); false for a scene or prefab. */
  fingerprintDiffers: boolean;
  /**
   * The prefab and data assets whose text in the session differs from their file (a tick 0
   * edit, "Apply to asset", "Apply to prefab"): plugins add an asset when they change it and
   * remove it when they save it.
   * Reloading changed assets keeps their session text. Stored sessions carry it: a document
   * that opens, or a new game, starts from the files' texts and marks those of the session it
   * restores again.
   */
  unsavedAssets: AssetId[];
  /** Why the last open, reload, or load into a new game was refused (the previous document and session stay); null once one succeeds. */
  error: string | null;
  /**
   * Stores the current session under `.ludenea/`, then loads the document `id` with the
   * project's asset texts and restores its stored session, if any. Without a game, the next
   * game loads it. A failure keeps the previous document and session.
   */
  open(id: AssetId): Promise<void>;
  /** Replaces the open scene or prefab with `text` (a change on disk), with the project's asset texts: the session replays. */
  replace(text: string): Promise<void>;
  /** Reloads the project's prefab and data asset texts into the session, keeping its edits and the text of each unsaved asset. */
  reloadAssets(): Promise<void>;
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
  /** `restoreSession` that also marks the session's unsaved assets unsaved again. */
  restore(control: ControlClient, saved: SavedSession, tick?: number): Promise<Loaded | null>;
}

/** A Zustand store: one document at a time, as the game keeps one session. */
export type DocumentStore = UseBoundStore<StoreApi<DocumentState>>;

/** A session as `ExportSession` gave it, ready to load, and the tick the timeline showed then. */
export interface SavedSession {
  load: Extract<Command, { type: "load_session" }>;
  tick: number;
  /** The document open when it was exported; null while one was opening or none was open. */
  document?: AssetId | null;
  /** The prefab and data assets unsaved when it was exported: its text of each differs from the file. */
  unsavedAssets?: AssetId[];
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
 * Resolves once the game has simulated `tick` (a seek further than the resimulation fails),
 * by default once it no longer resimulates; rejects if the game goes away, panics, or takes
 * longer than `RESIMULATION_TIMEOUT_MS`.
 */
function simulated(control: ControlClient, tick = Infinity, onWait = () => {}): Promise<void> {
  return new Promise((resolve, reject) => {
    const stops: (() => void)[] = [];
    const settle = (error?: Error) => {
      stops.forEach((stop) => stop());
      clearTimeout(timer);
      if (error) reject(error);
      else resolve();
    };
    const timer = setTimeout(() => settle(new Error(`the game did not resimulate ${tick === Infinity ? "its session" : `to tick ${tick}`} in time`)), RESIMULATION_TIMEOUT_MS);
    stops.push(
      control.on("resimulated", ({ up_to }) => {
        if (up_to >= tick) settle();
      }),
      control.on("timeline_changed", ({ timeline }) => {
        if (!timeline.resimulating) settle();
      }),
      control.on("paused", ({ reason }) => {
        if (reason.type === "panicked") settle(new Error(`the game panicked at tick ${reason.tick} while resimulating: ${reason.message}`));
      }),
      control.onClose((reason) => settle(new Error(`the game stopped while resimulating: ${reason}`))),
    );
    // Already resimulated: nothing to wait for. Otherwise `Resimulated` tells how far it got.
    control.request({ type: "get_timeline" }).then((timeline) => {
      if (!timeline.resimulating) settle();
      else onWait();
    }, settle);
  });
}

/**
 * Loads `saved` into a game and goes back to `tick` (the saved one by default), within what
 * the session recorded. A game whose code, scene, or assets changed reports `Diverged` while
 * it resimulates. Send it after the game's `LoadScene` (which the document store sends to
 * every new game), so the restored session is what stays loaded.
 */
export async function restoreSession(control: ControlClient, saved: SavedSession, tick = saved.tick, onLoaded = () => {}): Promise<Loaded | null> {
  const loaded = await control.request(saved.load);
  onLoaded();
  await returnTo(control, saved, tick);
  return loadedIn(loaded);
}

/**
 * Seeks back to `tick` once the loaded `saved` is resimulated that far, within what it
 * recorded, unless `still()` says the session changed meanwhile; resolves whether it sought.
 */
async function returnTo(control: ControlClient, saved: SavedSession, tick: number, still = () => true): Promise<boolean> {
  const target = Math.max(0, Math.min(tick, saved.load.expected.length - 1));
  await simulated(control, target);
  if (!still()) return false;
  await control.request({ type: "seek", tick: target });
  return true;
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

/** The `fresh` asset texts, with the `session`'s text of each `unsaved` asset instead. */
const withUnsaved = (fresh: [AssetId, string][], session: [AssetId, string][], unsaved: AssetId[]) => {
  const texts = new Map(session.filter(([id]) => unsaved.includes(id)));
  return { assets: fresh.map(([id, text]): [AssetId, string] => [id, texts.get(id) ?? text]), texts };
};
/** The unsaved assets of `saved` it still holds. */
const unsavedIn = (saved: SavedSession | null): AssetId[] =>
  saved ? (saved.unsavedAssets ?? []).filter((id) => saved.load.assets.some(([asset]) => asset === id)) : [];

const assetKind = (change: FileChange) => Object.values(change)[0]!.kind;
const changedPath = (change: FileChange) => Object.values(change)[0]!.path;

export function createDocumentStore(options: {
  project: ProjectClient;
  control: () => ControlClient | null;
  onGameChanged(listener: (control: ControlClient | null) => void): void;
  /** Where the store reports failures: the editor's console, at the asset's file when known. */
  log: EditorConsole["log"];
  /** Whether the game plays: a reload of the assets plays on once it is back. */
  playing?: () => boolean;
}): { store: DocumentStore; sessions: DocumentSessions } {
  const { project, control, log } = options;
  // The project's asset texts the game last accepted, the file of each, and the open replay.
  let texts: { assets: [AssetId, string][]; paths: [AssetId, string][] } = { assets: [], paths: [] };
  let replay = "";
  // The stored session of a document opened without a game, for the next game.
  let pending: SavedSession | null = null;
  // Exports taken while a document opens may belong to either document: they are tagged with none.
  let opening = false;
  // The opens asked for and not finished, by document: a second open of one already waiting is the same.
  const opens = new Map<AssetId, Promise<void>>();
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
    store.setState({ error: text });
    log("error", text, error instanceof AssetFailure ? { file: error.path, line: error.line, column: error.column } : undefined);
    throw error;
  };

  /**
   * The project's `fresh` asset texts, with the `session`'s text of each unsaved asset instead,
   * and `kept`, which filters a later unsaved list: an unsaved asset the session no longer holds
   * has nothing left to save (one marked meanwhile stays).
   */
  const keepingUnsaved = (fresh: [AssetId, string][], session: [AssetId, string][]) => {
    const unsaved = store.getState().unsavedAssets;
    const { assets, texts } = withUnsaved(fresh, session, unsaved);
    return {
      assets,
      kept: (now: AssetId[]) => now.filter((id) => texts.has(id) || !unsaved.includes(id)),
    };
  };

  const readTexts = () => project.request({ type: "read_asset_texts" }).then(({ assets, paths }) => ({ assets, paths }));
  const load = (game: ControlClient, document: OpenDocument, text: string | null, source: { texts: typeof texts; replay: string }): Promise<Reply> => {
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
  // The scene and assets on disk now, so a stored session whose files changed replays against
  // them, except its unsaved assets, which keep the session's text (an asset gone from disk is dropped).
  const refreshed = (saved: SavedSession, text: string | null, fresh: [AssetId, string][]): SavedSession => {
    if (saved.load.keeps_assets) return saved;
    const scene = saved.load.saved_scene === null && text !== null ? text : saved.load.scene;
    const { assets } = withUnsaved(fresh, saved.load.assets, saved.unsavedAssets ?? []);
    return { ...saved, load: { ...saved.load, scene, assets } };
  };
  const restore = (game: ControlClient, saved: SavedSession, tick = saved.tick) =>
    restoreSession(game, saved, tick, () => store.setState({ unsavedAssets: unsavedIn(saved) }));
  const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
  /** Names the root branch after the document; the document is open either way. */
  const nameRoot = async (game: ControlClient, document: OpenDocument) => {
    const name = document.path.split("/").at(-1)!.split(".")[0]!;
    try {
      const timeline = await game.request({ type: "get_timeline" });
      // A host that answers something else (a test double) keeps its names.
      const root = timeline.type === "timeline" ? timeline.branches.find((branch) => branch.parent === null) : undefined;
      if (root?.name === ROOT_NAME) await game.request({ type: "rename_branch", branch: root.id, name });
    } catch (error) {
      log("warning", `Cannot name the root branch ${name}: ${message(error)}`);
    }
  };
  const tagged = async (game: ControlClient): Promise<SavedSession> => {
    const document = opening ? null : (store.getState().current?.id ?? null);
    const saved = await exportSession(game);
    return { ...saved, document, unsavedAssets: store.getState().unsavedAssets };
  };

  const store: DocumentStore = create<DocumentState>()((set, get) => ({
    current: null,
    text: null,
    loaded: null,
    fingerprintDiffers: false,
    unsavedAssets: [],
    error: null,
    open: (id) => {
      const waiting = opens.get(id);
      if (waiting) return waiting;
      // The file once known, for the console.
      let name = `the document ${id}`;
      const opened = serial(async () => {
        const { entries } = await project.request({ type: "list_files" });
        const entry = entries.find((file) => file.id === id);
        if (!entry) throw new Error(`no project file has the identifier ${id}`);
        name = entry.path;
        const kind = typeof entry.kind === "string" ? KINDS[entry.kind] : undefined;
        if (!kind) throw new Error(`${entry.path} is not a scene, a prefab, or a replay`);
        const document = { id, kind, path: entry.path };
        const game = control();
        const previous = get().current;
        // A resimulating session cannot be exported: the switch waits for it.
        if (previous && game) {
          await simulated(game, Infinity, () => log("info", `Opening ${entry.path} once the game finishes resimulating.`));
          await storeIn(previous, await tagged(game));
        }
        opening = true;
        try {
          const fresh = await readTexts();
          let text: string | null = null;
          let bytes = "";
          if (kind === "replay") ({ bytes } = await project.request({ type: "read_binary", path: entry.path }));
          else ({ text } = await project.request({ type: "read_text", path: entry.path }));
          const saved = await stored(document);
          const restoring = saved && refreshed(saved, text, fresh.assets);
          const reply = game ? await load(game, document, text, { texts: fresh, replay: bytes }) : null;
          const loaded = reply && loadedIn(reply);
          // Only what the game accepted is kept: a refused document leaves the previous one.
          texts = fresh;
          replay = bytes;
          // A stored session's load reports what stays loaded, and marks its unsaved assets once
          // the game holds them: the scene load's report and marks are not kept.
          const restores = game !== null && restoring !== null;
          set({ current: document, text, loaded: restores ? null : loaded, fingerprintDiffers: differs(reply), unsavedAssets: restores ? [] : unsavedIn(restoring), error: null });
          pending = game ? null : restoring;
          if (!game) return;
          if (restoring) {
            // The document is open, from its file: a stored session it cannot take back is
            // reported (on the scene tree too when refused), not a refused open.
            let accepted = false;
            const restored = await restoreSession(game, restoring, restoring.tick, () => {
              accepted = true;
              set({ unsavedAssets: unsavedIn(restoring) });
            }).then(
              (reply) => reply,
              (error: unknown) => {
                const failure = locateAssetError(error, texts.paths);
                const text = accepted
                  ? `The stored session of ${entry.path} is loaded, but cannot go back to tick ${restoring.tick}: ${message(failure)}`
                  : `The game refused the stored session of ${entry.path}; the document opened from its file: ${message(failure)}`;
                log(accepted ? "warning" : "error", text, failure instanceof AssetFailure ? { file: failure.path, line: failure.line, column: failure.column } : undefined);
                if (!accepted) set({ error: text });
                return accepted ? null : loaded;
              },
            );
            set({ loaded: restored });
          }
          await nameRoot(game, document);
        } finally {
          opening = false;
        }
      }).catch((error: unknown) => reported(`Cannot open ${name}`)(error));
      const done = opened.finally(() => opens.delete(id));
      opens.set(id, done);
      return done;
    },
    replace: (text) =>
      serial(async () => {
        const fresh = await readTexts();
        const game = control();
        // The session's texts are only needed to keep those of unsaved assets.
        const session = game && get().unsavedAssets.length > 0 ? (await exportSession(game)).load.assets : [];
        const { assets, kept } = keepingUnsaved(fresh.assets, session);
        const loaded = game && (await game.request({ type: "replace_scene", scene: text, assets }).catch(locatedIn(fresh.paths)));
        texts = fresh;
        set({ text, error: null, ...(loaded && { loaded: loadedIn(loaded) }), ...(game && { unsavedAssets: kept(get().unsavedAssets) }) });
      }).catch(reported("Cannot reload the scene")),
    reloadAssets: () => reloadAssets(),
    store: () => sessions.store(),
  }));

  // A prefab or data asset changed on disk: the open scene or prefab reloads with it. The
  // session goes back with the new texts as a whole (`ReplaceScene` would mark unsaved scene
  // edits saved): it keeps its edits, history, branches, and unsaved state, and replays. An
  // unsaved asset keeps its session text over the file's. Refused, the game keeps the
  // previous session and the previous texts stay.
  //
  // Reloads coalesce: one asked for while another waits its turn or for a resimulation is the
  // same reload, which reads the texts on disk once it runs. Each reload plays on when the game
  // played when the first of them was asked for, or when the reload it follows was to.
  let queued: Promise<void> | null = null;
  // The latest reload: an earlier one leaves the seek and play to it.
  let latest = 0;
  let resuming = false;
  const reloadAssets = (): Promise<void> => {
    if (queued) return queued;
    const reload = ++latest;
    const playing = resuming || (options.playing?.() ?? false);
    resuming = playing;
    // Once it reads the texts, a new change on disk needs a reload of its own.
    const release = () => {
      if (queued === mine) queued = null;
    };
    const mine: Promise<void> = reloadOnce(reload, playing, release).finally(release);
    queued = mine;
    return mine;
  };
  const reloadOnce = async (reload: number, playing: boolean, reading: () => void) => {
    const reloaded = await serial(async () => {
      const document = store.getState().current;
      const game = control();
      // A resimulating session cannot be exported: the reload waits for it.
      if (document?.kind !== "replay" && document && game) await simulated(game);
      reading();
      if (document?.kind === "replay") return null;
      const fresh = await readTexts();
      let saved: SavedSession | null = null;
      if (document && game) {
        const exported = await exportSession(game);
        const { assets, kept } = keepingUnsaved(fresh.assets, exported.load.assets);
        saved = { ...exported, load: { ...exported.load, assets } };
        const loaded = loadedIn(await game.request(saved.load).catch(locatedIn(fresh.paths)));
        store.setState({ loaded, unsavedAssets: kept(store.getState().unsavedAssets), error: null });
      }
      texts = fresh;
      return game && saved && document && { game, saved, document };
    }).catch(reported("Cannot reload the changed assets"));
    // Off the queue: a long resimulation holds up no document change, and a document opened
    // (or asked for), a game started, or a later reload meanwhile is left as it is.
    if (reloaded) {
      const { game, saved, document } = reloaded;
      const still = () => reload === latest && opens.size === 0 && !opening && control() === game && store.getState().current?.id === document.id;
      try {
        // Loading the session paused the game: it plays on from where it was.
        if ((await returnTo(game, saved, saved.tick, still)) && playing && still()) await game.request({ type: "play" });
      } catch (error) {
        log("warning", `Cannot go back to tick ${saved.tick} after reloading the assets: ${message(error)}`);
      }
    }
    if (reload === latest) resuming = false;
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
    // The game holds the files' texts until a session is restored into it.
    store.setState({ unsavedAssets: [] });
    // Both sent at once: a session the application restores into this game, and its first
    // export, come after them.
    const loaded = load(game, document, store.getState().text, { texts, replay });
    const restored = saved ? restore(game, saved).catch(locatedIn(texts.paths)) : null;
    Promise.all([loaded, restored])
      .then(([first, second]) => {
        store.setState({ loaded: second ?? loadedIn(first), fingerprintDiffers: differs(first), error: null });
        return nameRoot(game, document);
      })
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
      // Like a document that opens: against the scene and assets on disk now.
      const own = document && (await stored(document));
      return own && refreshed(own, store.getState().text, texts.assets);
    },
    restore,
  };
  return { store, sessions };
}
