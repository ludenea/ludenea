import { describe, expect, it } from "vitest";
import * as plugins from "../src";
import * as app from "../src/app";

const APP_ONLY = ["CommandPalette", "PluginLoader", "STORED_SESSION_VERSION", "Shell", "createEditor", "exportSession", "installShortcuts", "mountShell", "reportCaughtErrors", "restoreSession"];

describe("entry points", () => {
  it("the_plugin_entry_leaves_out_what_only_the_applications_use", () => {
    expect(Object.keys(plugins).filter((name) => APP_ONLY.includes(name))).toEqual([]);
    expect(Object.keys(app).sort()).toEqual(APP_ONLY);
  });
});
