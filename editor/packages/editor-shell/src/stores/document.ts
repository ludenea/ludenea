import { ProtocolFailure, type AssetId, type Command, type ControlClient, type FileChange, type ProjectClient, type Reply } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";
import { AssetFailure, locateAssetError, nameAssets } from "../assets";
import type { ConsoleLocation, EditorConsole } from "../console";
import { decodeStoredSession, encodeStoredSession, type StoredSession } from "./stored-session";

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
  /**
   * Reloads the project's prefab and data asset texts into the session, keeping its edits and
   * the text of each unsaved asset (the game's timeline lists them), except those of `discard`,
   * which take their file's text.
   */
  reloadAssets(discard?: AssetId[]): Promise<void>;
  /** Stores the game's session as the open document's, as `open` does before loading another. */
  store(): Promise<void>;
  /** `text` with each project asset the game names by identifier followed by the asset's file. */
  nameAssets(text: string): string;
}

/** The sessions of documents, as the application's crash recovery and close keep them. */
export interface DocumentSessions {
  /** Exports the session of `control`, tagged with the document open when asked (none while one opens). */
  export(control: ControlClient): Promise<SavedSession>;
  /**
   * Stores `saved` (a fresh export by default) as the open document's session; one tagged with
   * another document is not stored, nor any while a document opens, nor a stored session the
   * game refused through `restore`.
   */
  store(saved?: SavedSession): Promise<void>;
  /** `saved` when it belongs to the open document, else the open document's stored session, if any: restore it with `restore`. */
  own(saved: SavedSession | null): Promise<SavedSession | null>;
  /**
   * `restoreSession`, except for a session `own` gave: when the game refuses it, the document's
   * stored session is set aside before any later store of its document, with a warning, and the
   * restore resolves to null.
   */
  restore(control: ControlClient, saved: SavedSession, tick?: number): Promise<Loaded | null>;
}

/** A Zustand store: one document at a time, as the game keeps one session. */
export type DocumentStore = UseBoundStore<StoreApi<DocumentState>>;

/**
 * A session as `ExportSession` gave it, ready to load, and the tick the timeline showed then.
 * Its unsaved scene and assets stay unsaved once loaded (`saved_scene`, `saved_assets`).
 */
export interface SavedSession {
  load: Extract<Command, { type: "load_session" }>;
  tick: number;
  /** The document open when it was exported; null while one was opening or none was open. */
  document?: AssetId | null;
}

export async function exportSession(control: ControlClient): Promise<SavedSession> {
  const [{ scene, saved_scene, inputs, edits, undo, hashes, assets, saved_assets, branches, document, keeps_assets }, timeline] = await Promise.all([
    control.request({ type: "export_session" }),
    control.request({ type: "get_timeline" }),
  ]);
  return {
    load: { type: "load_session", scene, saved_scene, inputs, edits, undo, expected: hashes, assets, saved_assets, branches, document, keeps_assets },
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

/** Where the project keeps the stored session of the document `id`, as messages name it. */
const sessionFile = (id: AssetId) => `.ludenea/sessions/${id}.session`;

const KINDS: { [kind: string]: OpenDocument["kind"] } = { Scene: "scene", Prefab: "prefab", Replay: "replay" };
/** What became of a document whose stored session was not restored: it opened, or a new game loaded it again. */
const OPENED = "the document opened from its file";
const LOADED_AGAIN = "the new game loaded the document from its file";
type Outcome = typeof OPENED | typeof LOADED_AGAIN;
/** How the session names its root branch until the editor renames it after the document. */
const ROOT_NAME = "Branch 1";

/**
 * The session `load` with the `fresh` asset texts on disk: an unsaved asset, unless in
 * `discard`, keeps its session text, unsaved against its file's; an asset gone from disk is gone.
 */
const onDisk = (load: SavedSession["load"], fresh: [AssetId, string][], discard: AssetId[] = []): SavedSession["load"] => {
  const session = new Map(load.assets);
  const kept = new Set(load.saved_assets.map(([id]) => id).filter((id) => session.has(id) && !discard.includes(id)));
  return {
    ...load,
    assets: fresh.map(([id, text]): [AssetId, string] => [id, kept.has(id) ? session.get(id)! : text]),
    saved_assets: fresh.filter(([id]) => kept.has(id)),
  };
};

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
/** Where the console points for `error`: the asset at fault, when known. */
const locationOf = (error: unknown): ConsoleLocation | undefined => (error instanceof AssetFailure ? { file: error.path, line: error.line, column: error.column } : undefined);

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
    const text = `${what}: ${message(error)}`;
    store.setState({ error: text });
    log("error", text, locationOf(error));
    throw error;
  };

  const readTexts = () => project.request({ type: "read_asset_texts" }).then(({ assets, paths }) => ({ assets, paths }));
  const load = (game: ControlClient, document: OpenDocument, text: string | null, source: { texts: typeof texts; replay: string }): Promise<Reply> => {
    const command: Command =
      document.kind === "replay"
        ? { type: "load_replay", replay: source.replay }
        : { type: "load_scene", scene: text!, assets: source.texts.assets, document: document.kind === "prefab" ? { Prefab: { id: document.id } } : "Scene" };
    return game.request(command).catch(locatedIn(source.texts.paths));
  };
  // Whether the stored session of each document being restored, or that the editor could not
  // restore, is safe to store over: the game took it (true), or it was set aside (true) or
  // cannot be (false). The stores of its document wait for it, so none overwrites that session,
  // and make none while it cannot be set aside, until a game takes it or it is gone.
  const asides = new Map<AssetId, Promise<boolean>>();
  // The stored sessions a game refused: one still held elsewhere (the seed of a session guard)
  // is never stored back over the export that replaced it.
  const refused = new WeakSet<SavedSession>();
  const storeIn = async (document: OpenDocument, saved: SavedSession) => {
    if (refused.has(saved) || (await asides.get(document.id)) === false) return;
    await project.request({ type: "store_session", document: document.id, bytes: encodeStoredSession(saved) });
  };
  /** The stored session of `document` as read, if any. */
  const stored = async (document: OpenDocument): Promise<StoredSession | null> => {
    const { bytes } = await project.request({ type: "load_stored_session", document: document.id });
    // One that reads waits for a game to take it.
    if (bytes !== null) return decodeStoredSession(bytes);
    // Gone: its document's stores go on.
    asides.delete(document.id);
    return null;
  };
  /**
   * Sets the stored session of `document` aside, where no store of the document made after
   * this call overwrites it, and logs `why` it was not restored, the `outcome` for the
   * document, and where the session is kept. `kept` resolves to whether it was set aside,
   * `warning` to the logged warning.
   */
  const setAside = (document: OpenDocument, why: string, outcome: Outcome, location?: ConsoleLocation): { kept: Promise<boolean>; warning: Promise<string> } => {
    const request = project.request({ type: "set_aside_stored_session", document: document.id });
    const kept = request.then(
      () => true,
      () => false,
    );
    asides.set(document.id, kept);
    const warning = request
      .then(
        ({ path }) => `${why}; ${outcome}${path === null ? "" : `, and the session is kept as ${path}`}.`,
        (error: unknown) =>
          `${why}; ${outcome}, but the session cannot be set aside (${message(error)}), so its session is not stored until ${sessionFile(document.id)} is moved away and the document opened again.`,
      )
      .then((text) => {
        log("warning", text, location);
        return text;
      });
    return { kept, warning };
  };
  const unreadable = (document: OpenDocument, reason: string, outcome: Outcome) =>
    setAside(document, `The stored session of ${document.path} (${sessionFile(document.id)}) cannot be read: ${reason}`, outcome).warning;
  /**
   * Restores `saved`, the stored session of `document` or one the previous game `exported` of
   * it, into `game` at `tick`: resolves to what the game reports loaded (null when the session
   * loaded but cannot go back to its tick), or, when the game refused it, to the warning that the
   * stored session was set aside, which gives the `outcome` for the document. An export the
   * game never answers (its connection closed) is not refused: the call rejects and nothing is
   * set aside. The document's stores wait until the game takes it or the stored session is set aside.
   */
  const restoreStored = async (
    game: ControlClient,
    document: OpenDocument,
    outcome: Outcome,
    saved: SavedSession,
    tick = saved.tick,
    exported = false,
  ): Promise<{ loaded: Loaded | null } | { refused: string }> => {
    let decide!: (kept: boolean | Promise<boolean>) => void;
    asides.set(document.id, new Promise((resolve) => (decide = resolve)));
    let accepted = false;
    try {
      return {
        loaded: await restoreSession(game, saved, tick, () => {
          accepted = true;
          decide(true);
        }),
      };
    } catch (error) {
      // The export stays the latest session of its document, to store over the stored one.
      if (exported && !accepted && !(error instanceof ProtocolFailure)) {
        decide(true);
        throw error;
      }
      const failure = locateAssetError(error, texts.paths);
      if (!accepted) {
        refused.add(saved);
        const why = exported
          ? `The game refused the session of ${document.path} that the previous game exported: ${message(failure)}. Its stored session (${sessionFile(document.id)}) is set aside`
          : `The game refused the stored session of ${document.path} (${sessionFile(document.id)}): ${message(failure)}`;
        const { kept, warning } = setAside(document, why, outcome, locationOf(failure));
        decide(kept);
        return { refused: await warning };
      }
      log("warning", `The ${exported ? "" : "stored "}session of ${document.path} is loaded, but cannot go back to tick ${tick}: ${message(failure)}`, locationOf(failure));
      return { loaded: null };
    }
  };
  // The sessions `own` gave, with their document and whether the previous game exported them
  // (else they are stored), for `restore`.
  const ownStored = new WeakMap<SavedSession, { document: OpenDocument; exported: boolean }>();
  // The scene and assets on disk now, so a stored session whose files changed replays against
  // them, except its unsaved scene and assets, which keep the session's text.
  const refreshed = (saved: SavedSession, text: string | null, fresh: [AssetId, string][]): SavedSession => {
    if (saved.load.keeps_assets) return saved;
    const scene = saved.load.saved_scene === null && text !== null ? text : saved.load.scene;
    return { ...saved, load: { ...onDisk(saved.load, fresh), scene } };
  };
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
    return { ...(await exportSession(game)), document };
  };

  const store: DocumentStore = create<DocumentState>()((set, get) => ({
    current: null,
    text: null,
    loaded: null,
    fingerprintDiffers: false,
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
          const read = await stored(document);
          const restoring = read && "saved" in read ? refreshed(read.saved, text, fresh.assets) : null;
          const reply = game ? await load(game, document, text, { texts: fresh, replay: bytes }) : null;
          const loaded = reply && loadedIn(reply);
          // Only what the game accepted is kept: a refused document leaves the previous one.
          texts = fresh;
          replay = bytes;
          // A stored session that cannot be read is set aside before its document is current.
          const aside = read && "unreadable" in read ? unreadable(document, read.unreadable, OPENED) : null;
          // A stored session's load reports what stays loaded: the scene load's report is not kept.
          const restores = game !== null && restoring !== null;
          set({ current: document, text, loaded: restores ? null : loaded, fingerprintDiffers: differs(reply), error: null });
          pending = game ? null : restoring;
          // The document is open, from its file: a stored session it cannot take back is
          // reported, on the scene tree too when it is set aside, not a refused open.
          if (aside) set({ error: await aside });
          if (!game) return;
          if (restoring) {
            const restored = await restoreStored(game, document, OPENED, restoring);
            set("refused" in restored ? { loaded, error: restored.refused } : { loaded: restored.loaded });
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
        // The game keeps the text of each unsaved asset.
        const loaded = game && (await game.request({ type: "replace_scene", scene: text, assets: fresh.assets }).catch(locatedIn(fresh.paths)));
        texts = fresh;
        set({ text, error: null, ...(loaded && { loaded: loadedIn(loaded) }) });
      }).catch(reported("Cannot reload the scene")),
    reloadAssets: (discard) => reloadAssets(discard),
    store: () => sessions.store(),
    nameAssets: (text) => nameAssets(text, texts.paths),
  }));

  // A prefab or data asset changed on disk: the open scene or prefab reloads with it. The
  // session goes back with the new texts as a whole (`ReplaceScene` would mark unsaved scene
  // edits saved): it keeps its edits, history, branches, and unsaved state, and replays. An
  // unsaved asset keeps its session text over the file's. Refused, the game keeps the
  // previous session and the previous texts stay.
  //
  // Reloads coalesce: one asked for while another waits its turn or for a resimulation is the
  // same reload, which reads the texts on disk, and takes the assets to discard, once it runs.
  // Each reload plays on when the game played when the first of them was asked for, or when the
  // reload it follows was to.
  let queued: Promise<void> | null = null;
  let discarding: AssetId[] = [];
  // The latest reload: an earlier one leaves the seek and play to it.
  let latest = 0;
  let resuming = false;
  const reloadAssets = (discard: AssetId[] = []): Promise<void> => {
    discarding = [...discarding, ...discard];
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
      const discard = discarding;
      discarding = [];
      if (document?.kind === "replay") return null;
      const fresh = await readTexts();
      let saved: SavedSession | null = null;
      if (document && game) {
        const exported = await exportSession(game);
        saved = { ...exported, load: onDisk(exported.load, fresh.assets, discard) };
        const loaded = loadedIn(await game.request(saved.load).catch(locatedIn(fresh.paths)));
        store.setState({ loaded, error: null });
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
    // Both sent at once: a session the application restores into this game, and its first
    // export, come after them.
    const loaded = load(game, document, store.getState().text, { texts, replay });
    const restored = saved ? restoreStored(game, document, LOADED_AGAIN, saved) : null;
    Promise.all([loaded, restored])
      .then(([first, second]) => {
        const report = { loaded: loadedIn(first), fingerprintDiffers: differs(first) };
        // A refused stored session leaves the scene load's report, and its warning on the scene
        // tree. A restored one's load reports what stays loaded, as when the document opens.
        store.setState(second && "refused" in second ? { ...report, error: second.refused } : { ...report, loaded: second ? second.loaded : report.loaded, error: null });
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
      // While a document opens, its stored session is being restored: an earlier export of it
      // would overwrite that before the game accepts or refuses it.
      if (!document || opening || (!saved && !game)) return;
      saved ??= await tagged(game!);
      if (saved.document === document.id) await storeIn(document, saved);
    },
    async own(saved) {
      const document = store.getState().current;
      if ((saved?.document ?? null) === (document?.id ?? null)) {
        // Refused, it sets the stored session aside: the next store would overwrite it.
        if (saved && document) ownStored.set(saved, { document, exported: true });
        return saved;
      }
      if (!document) return null;
      // Like a document that opens: against the scene and assets on disk now.
      const read = await stored(document);
      if (read && "unreadable" in read) {
        store.setState({ error: await unreadable(document, read.unreadable, LOADED_AGAIN) });
        return null;
      }
      if (!read) return null;
      const own = refreshed(read.saved, store.getState().text, texts.assets);
      ownStored.set(own, { document, exported: false });
      return own;
    },
    async restore(control, saved, tick = saved.tick) {
      const owned = ownStored.get(saved);
      if (!owned) return restoreSession(control, saved, tick);
      const { document, exported } = owned;
      const restored = await restoreStored(control, document, LOADED_AGAIN, saved, tick, exported);
      if ("loaded" in restored) return restored.loaded;
      // Shown on the scene tree too, as when the document opens.
      if (store.getState().current?.id === document.id) store.setState({ error: restored.refused });
      return null;
    },
  };
  return { store, sessions };
}
