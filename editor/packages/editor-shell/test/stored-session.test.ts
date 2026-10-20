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

  it("an_unversioned_stored_session_of_a_prefab_names_its_asset", () => {
    const prefab = "000000000000000000000000000000b0";
    // Before versions, the load named a prefab document "Prefab" and the stored session its asset.
    const { saved_assets: _, ...load } = saved("(prefab)").load;
    const unversioned = { load: { ...load, document: "Prefab" }, tick: 3, document: prefab, unsavedAssets: [] };

    expect(decodeStoredSession(storedBytes(unversioned))).toEqual({
      saved: { load: { ...load, saved_assets: [], document: { Prefab: { id: prefab } } }, tick: 3, document: prefab },
    });
  });

  it("an_unversioned_stored_session_with_unsaved_assets_that_are_not_identifiers_names_them", () => {
    const { saved_assets: _, ...load } = saved("(main)").load;
    const read = (unsavedAssets: unknown) => decodeStoredSession(storedBytes({ load, tick: 0, document: null, unsavedAssets }));

    expect(read([1])).toEqual({ unreadable: "its unsavedAssets is not a list of asset identifiers" });
    expect(read("a")).toEqual({ unreadable: "its unsavedAssets is not a list of asset identifiers" });
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
