import { describe, expect, it } from "vitest";
import type { SavedSession } from "../src/app";
import { decodeStoredSession, encodeStoredSession } from "../src/stores/stored-session";
import { storedBytes } from "./support";

const saved = (scene: string): SavedSession => ({
  load: { type: "load_session", scene, saved_scene: null, inputs: "", edits: "", undo: "", expected: [], assets: [], saved_assets: [], branches: "", document: "Scene", keeps_assets: false },
  tick: 0,
  document: null,
});

describe("stored sessions", () => {
  it("a_large_stored_session_round_trips_with_any_text", () => {
    // Several encoding chunks, with characters of two to four UTF-8 bytes across their bounds.
    const scene = "é漢🎮".repeat(30_000);

    expect(decodeStoredSession(encodeStoredSession(saved(scene)))).toEqual({ saved: saved(scene) });
  });

  it("an_unreadable_stored_session_says_what_is_wrong", () => {
    const valid = saved("(main)");
    const read = (value: unknown) => decodeStoredSession(storedBytes(value));

    expect(decodeStoredSession(btoa("\xff\xfe"))).toEqual({ unreadable: "it is not UTF-8 text" });
    expect(read([valid])).toEqual({ unreadable: "it is not a JSON object" });
    expect(read({ version: "1", saved: valid })).toEqual({ unreadable: 'it has version "1", and this editor reads version 1 and the unversioned form before it' });
    expect(read({ version: 1, saved: { ...valid, tick: -1 } })).toEqual({ unreadable: "its tick is not a tick" });
    expect(read({ version: 1, saved: { ...valid, load: { ...valid.load, type: "quit" } } })).toEqual({ unreadable: 'its load.type is not "load_session"' });
    expect(read({ version: 1, saved: { ...valid, load: { ...valid.load, saved_assets: [["a"]] } } })).toEqual({ unreadable: "its load.saved_assets is not a list of [asset, text] pairs" });
  });
});
