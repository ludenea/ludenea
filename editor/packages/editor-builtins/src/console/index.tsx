import type { ConsoleLevel, ControlClient, Diagnostic } from "@ludenea/control-client";
import { definePlugin } from "@ludenea/editor-shell";
import { ASSET_FOLDERS, createStore, shared, showPanel } from "../shared";
import { ConsolePanel, where, type Entry, type Levels, type Location } from "./ConsolePanel";

// ponytail: keeps the last lines only; page older ones to disk if long sessions need them.
const LIMIT = 5000;

const levelOf = (text: string): ConsoleLevel =>
  /^error\b/.test(text) ? "error" : /^warning\b/.test(text) ? "warning" : "info";

/**
 * Cargo reports each compiler message twice: a `line` with the rendered text, then a
 * `diagnostic` with its location. The console shows it once: the diagnostic turns the latest
 * matching rendered line (first line starting with its level and holding its message) into a
 * located entry, and stands alone when no such line exists.
 */
function located(entries: Entry[], diagnostic: Diagnostic, next: () => number): Entry[] {
  const location = diagnostic.file ? { file: diagnostic.file, line: diagnostic.line, column: diagnostic.column } : undefined;
  const level = levelOf(diagnostic.level);
  const matches = (entry: Entry) => {
    const first = entry.text.split("\n", 1)[0]!;
    return entry.source === "cargo" && first.startsWith(diagnostic.level) && first.includes(diagnostic.message);
  };
  for (let index = entries.length - 1; index >= 0; index--) {
    const entry = entries[index]!;
    if (!matches(entry)) continue;
    return [...entries.slice(0, index), { ...entry, level, source: "build", location }, ...entries.slice(index + 1)];
  }
  return [...entries, { id: next(), level, source: "build", text: `${diagnostic.level}: ${diagnostic.message}`, location }];
}

/** `file` relative to the project `root` (as the asset browser names it), when it lies inside. */
function projectPath(file: string, root: string | undefined): string {
  let path = file.replaceAll("\\", "/");
  const prefix = root && `${root.replaceAll("\\", "/").replace(/\/+$/, "")}/`;
  if (prefix && path.startsWith(prefix)) path = path.slice(prefix.length);
  return path.replace(/^(\.\/)+/, "");
}

export const consolePlugin = definePlugin({
  name: "ludenea.console",
  activate(editor) {
    let lastId = 0;
    const next = () => ++lastId;
    const state = createStore({ entries: [] as Entry[], levels: { error: true, warning: true, info: true } as Levels, notice: null as string | null });
    const update = (change: (entries: Entry[]) => Entry[]) => state.set({ entries: change(state.get().entries).slice(-LIMIT) });
    const add = (level: ConsoleLevel, text: string, source: Entry["source"] = "editor", location?: Location) =>
      update((entries) => [...entries, { id: next(), level, source, text, location }]);

    // What the editor console holds, from its first entry: those logged before this plugin
    // activated waited there.
    let seen = 0;
    const take = () => {
      const fresh = editor.console.list().filter((entry) => entry.id > seen);
      if (fresh.length === 0) return;
      seen = fresh.at(-1)!.id;
      update((entries) => [...entries, ...fresh.map(({ level, text, location }) => ({ id: next(), level, source: "editor" as const, text, location }))]);
    };
    take();
    const stopEditor = editor.console.subscribe(take);
    // Kept here, not in the panel: the shell unmounts a panel while its tab is hidden.
    const scroll = { top: 0 };
    const project = editor.project;
    const stops = [
      project.on("build", ({ event }) => {
        if (event.type === "line") add(levelOf(event.text), event.text, "cargo");
        else if (event.type === "diagnostic") update((entries) => located(entries, event, next));
        else if (event.type === "finished") add(event.ok ? "info" : "error", event.ok ? "Build finished." : "Build failed.");
      }),
      project.on("game_output", ({ stream, line }) => add(stream === "Stderr" ? "warning" : "info", line, "game")),
      project.on("warning", ({ message }) => add("warning", message)),
      project.on("game_ready", () => add("info", "Game started.")),
      project.on("game_exited", ({ code }) => add(code === 0 ? "info" : "warning", `Game exited${code === null ? "" : ` with code ${code}`}.`)),
      project.on("plugin_rebuilt", ({ name }) => add("info", `Plugin ${name} rebuilt.`)),
      project.on("plugin_failed", ({ name, diagnostics }) => {
        add("error", `Plugin ${name} failed to build.`);
        for (const diagnostic of diagnostics) update((entries) => located(entries, diagnostic, next));
      }),
    ];
    // Panics, dropped edits, and divergences all reach the host console.
    let stopHost: (() => void) | undefined;
    const listen = (control: ControlClient | null) => {
      stopHost?.();
      stopHost = control?.on("console", ({ level, text }) => add(level, text, "game"));
    };
    listen(editor.control);
    editor.onGameChanged(listen);

    // Scenes and assets open in the asset browser. It lists nothing else, and the editor has no
    // source view: a source location goes to the clipboard, for the user's own editor.
    const open = (location: Location) => {
      const path = projectPath(location.file, shared(editor).project.get().info?.root);
      if (ASSET_FOLDERS.some((folder) => path.startsWith(`${folder}/`))) {
        shared(editor).reveal.set({ path });
        showPanel(editor, "ludenea.assets");
        return;
      }
      const text = where(location);
      const copied = navigator.clipboard?.writeText(text) ?? Promise.reject(new Error("no clipboard"));
      copied.then(
        () => state.set({ notice: `Copied ${text}` }),
        () => state.set({ notice: text }),
      );
    };
    editor.panels.register({
      id: "ludenea.console",
      title: "Console",
      defaultPosition: "bottom",
      component: () => <ConsolePanel state={state} scroll={scroll} onLocation={open} />,
    });
    return () => {
      stopEditor();
      stops.forEach((stop) => stop());
      stopHost?.();
    };
  },
});
