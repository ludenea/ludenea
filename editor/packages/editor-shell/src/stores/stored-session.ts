import type { SavedSession } from "./document";

/**
 * The version of the stored session format (`.ludenea/sessions/<id>.session`): the JSON of
 * `{ version, saved }`, a `SavedSession` in an envelope, as base64 UTF-8 bytes. The
 * unversioned `SavedSession` stored before it reads as version 0.
 */
export const STORED_SESSION_VERSION = 1;

/** A stored session as read: the session, or why it cannot be. */
export type StoredSession = { saved: SavedSession } | { unreadable: string };

// `fromCharCode` takes the bytes as arguments, which a large session would overflow at once.
const CHUNK = 0x8000;

export function encodeStoredSession(saved: SavedSession): string {
  const bytes = new TextEncoder().encode(JSON.stringify({ version: STORED_SESSION_VERSION, saved }));
  const chunks: string[] = [];
  for (let start = 0; start < bytes.length; start += CHUNK) chunks.push(String.fromCharCode(...bytes.subarray(start, start + CHUNK)));
  return btoa(chunks.join(""));
}

/** Reads stored session bytes, of this version or version 0, without throwing. */
export function decodeStoredSession(bytes: string): StoredSession {
  let binary: string;
  try {
    binary = atob(bytes);
  } catch {
    return { unreadable: "it is not base64" };
  }
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(binary, (char) => char.charCodeAt(0)));
  } catch {
    return { unreadable: "it is not UTF-8 text" };
  }
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    return { unreadable: `it is not JSON (${error instanceof Error ? error.message : String(error)})` };
  }
  if (!isObject(value)) return { unreadable: "it is not a JSON object" };
  let saved: unknown;
  if (!("version" in value)) saved = fromVersion0(value);
  else if (value.version === STORED_SESSION_VERSION) saved = value.saved;
  else return { unreadable: `it has version ${JSON.stringify(value.version)}, and this editor reads version ${STORED_SESSION_VERSION} and the unversioned form before it` };
  const fault = faultOf(saved);
  return fault === null ? { saved: saved as SavedSession } : { unreadable: fault };
}

const isObject = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * Version 0, the unversioned `SavedSession`, listed its unsaved assets in `unsavedAssets`
 * before `load.saved_assets` held their saved text. That text was not stored: an empty one
 * keeps each unsaved, and opening the document replaces it with the file's. Its load named a
 * prefab document `"Prefab"`, whose asset is the session's `document`.
 */
function fromVersion0({ unsavedAssets, ...saved }: Record<string, unknown>): unknown {
  if (!isObject(saved.load)) return saved;
  const load = { ...saved.load };
  if (!("saved_assets" in load)) {
    const unsaved = Array.isArray(unsavedAssets) ? unsavedAssets : [];
    load.saved_assets = unsaved.map((id: unknown) => [id, ""]);
  }
  if (load.document === "Prefab" && isText(saved.document)) load.document = { Prefab: { id: saved.document } };
  return { ...saved, load };
}

const isText = (value: unknown) => typeof value === "string";
const isPairs = (value: unknown) => Array.isArray(value) && value.every((pair) => Array.isArray(pair) && pair.length === 2 && isText(pair[0]) && isText(pair[1]));

/**
 * The fields the editor reads, with what each must be. The game checks the rest when it loads
 * the session, and a session it refuses is set aside like one that does not decode.
 */
const FIELDS: [path: string, valid: (value: unknown) => boolean, what: string][] = [
  ["tick", (value) => Number.isSafeInteger(value) && (value as number) >= 0, "a tick"],
  ["document", (value) => value === undefined || value === null || isText(value), "an asset identifier"],
  ["load", isObject, "an object"],
  ["load.type", (value) => value === "load_session", '"load_session"'],
  ["load.scene", isText, "a text"],
  ["load.saved_scene", (value) => value === null || isText(value), "a text or null"],
  ["load.inputs", isText, "base64 bytes"],
  ["load.expected", (value) => Array.isArray(value) && value.every(isText), "a list of hashes"],
  ["load.assets", isPairs, "a list of [asset, text] pairs"],
  ["load.saved_assets", isPairs, "a list of [asset, text] pairs"],
  ["load.keeps_assets", (value) => typeof value === "boolean", "true or false"],
];

/** Why `saved` is not a `SavedSession` the editor can read, or null when it is. */
function faultOf(saved: unknown): string | null {
  for (const [path, valid, what] of FIELDS) {
    const value = path.split(".").reduce<unknown>((at, key) => (isObject(at) ? at[key] : undefined), saved);
    if (!valid(value)) return value === undefined ? `it has no ${path}` : `its ${path} is not ${what}`;
  }
  return null;
}
