// `@ludenea/editor-shell/app`: what the applications use to create, hold, and show the editor.
// Plugins import `@ludenea/editor-shell`, which leaves all of this out.
export { createEditor, type EditorHost } from "./editor";
export { Shell } from "./layout/Shell";
export { mountShell } from "./layout/mount";
export { PluginLoader } from "./loader";
export { CommandPalette } from "./palette/CommandPalette";
export { type Registry } from "./registry";
export { exportSession, restoreSession, type DocumentSessions, type SavedSession } from "./stores/document";
export { installShortcuts } from "./shortcuts";
