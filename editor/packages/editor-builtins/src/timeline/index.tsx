import type { BranchId, FileEntry } from "@ludenea/control-client";
import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { createStore, errorText, shared, showPanel, type Store } from "../shared";
import type { BranchAction, BranchForms } from "./BranchLanes";
import { TimelinePanel } from "./TimelinePanel";

/** The speed last asked of the host, which no reply reports back; the host clamps it to [0.1, 8]. */
export type SpeedState = Store<{ speed: number }>;

// Powers of two from 1, so slowing down and speeding up retrace the same steps.
const MIN_SPEED = 0.125;
const MAX_SPEED = 8;

function game(editor: Editor) {
  if (!editor.control) throw new Error("no game is running");
  return editor.control;
}

/** Moves the game to `tick` and shows what it holds there, once the timeline reports it. */
export async function seek(editor: Editor, tick: number): Promise<void> {
  try {
    await game(editor).request({ type: "seek", tick });
  } finally {
    await editor.timeline.getState().refresh();
  }
}

const REPLAY_FOLDER = "assets/replays";

/** A replay path named after `branch` that no project file takes. */
function freeReplayPath(files: FileEntry[], branch: string): string {
  const taken = new Set(files.map((file) => file.path));
  const base = `${REPLAY_FOLDER}/${branch.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "branch"}`;
  let path = `${base}.replay`;
  for (let number = 2; taken.has(path); number++) path = `${base}-${number}.replay`;
  return path;
}

/** Saves `branch` (the current one when null) as a replay asset; resolves with its path. */
async function saveReplay(editor: Editor, branch: BranchId | null): Promise<string> {
  const { replay } = await game(editor).request({ type: "export_replay", branch });
  const { list, current } = editor.branches.getState();
  const name = list.find((info) => info.id === (branch ?? current))?.name ?? "branch";
  const { entries } = await editor.project.request({ type: "list_files" });
  const path = freeReplayPath(entries, name);
  // The service creates an asset from text: the file and its meta first, then the bytes.
  await editor.project.request({ type: "create_asset", path, kind: "Replay", text: "" });
  try {
    await editor.project.request({ type: "write_binary", path, bytes: replay });
  } catch (error) {
    await editor.project.request({ type: "delete", path }).catch((failure: unknown) =>
      editor.console.log("warning", `Cannot remove ${path}, left empty by the failed save: ${errorText(failure)}`),
    );
    throw error;
  }
  return path;
}

export const timelinePlugin = definePlugin({
  name: "ludenea.timeline",
  activate(editor) {
    const speed: SpeedState = createStore({ speed: 1 });
    // A new host starts at normal speed.
    editor.onGameChanged(() => speed.set({ speed: 1 }));
    const setSpeed = async (factor: number) => {
      const next = Math.min(MAX_SPEED, Math.max(MIN_SPEED, speed.get().speed * factor));
      await game(editor).request({ type: "set_speed", speed: next });
      speed.set({ speed: next });
    };

    editor.commands.register({
      id: "timeline.play-pause",
      title: "Play or pause",
      async run() {
        const playing = editor.timeline.getState().playing;
        await game(editor).request({ type: playing ? "pause" : "play" });
        // `Ticked` reports playing on the next tick; `Paused` reports pausing.
        if (!playing) editor.timeline.setState({ playing: true });
      },
    });
    editor.commands.register({
      id: "timeline.step",
      title: "Step one tick",
      async run() {
        try {
          await game(editor).request({ type: "step", ticks: 1, record_hashes: false });
        } finally {
          void editor.timeline.getState().refresh();
        }
      },
    });
    editor.commands.register({
      id: "timeline.step-back",
      title: "Step one tick back",
      async run() {
        const current = editor.timeline.getState().timeline?.current ?? 0;
        if (current > 0) await seek(editor, current - 1);
      },
    });
    editor.commands.register({ id: "timeline.speed-up", title: "Play faster", run: () => setSpeed(2) });
    editor.commands.register({ id: "timeline.speed-down", title: "Play slower", run: () => setSpeed(0.5) });
    // The palette reads the titles when it shows them: they name what undo and redo change.
    editor.commands.register({
      id: "edit.undo",
      get title() {
        return editor.timeline.getState().timeline?.undo_label ?? "Undo";
      },
      async run() {
        await editor.edit({ type: "undo" });
      },
    });
    editor.commands.register({
      id: "edit.redo",
      get title() {
        return editor.timeline.getState().timeline?.redo_label ?? "Redo";
      },
      async run() {
        await editor.edit({ type: "redo" });
      },
    });

    const forms: BranchForms = {
      state: createStore<{ action: BranchAction | null; error: string | null }>({ action: null, error: null }),
      async perform({ kind, branch }, name) {
        const branches = editor.branches.getState();
        if (kind === "compare") await branches.setCompare(branch);
        else if (branch === null) throw new Error("choose a branch first");
        else if (kind === "switch") await branches.switch(branch);
        else if (kind === "delete") await branches.remove(branch);
        else if (!name) throw new Error("enter the branch's new name");
        else await branches.rename(branch, name);
        forms.state.set({ action: null, error: null });
        await editor.timeline.getState().refresh();
      },
    };
    // Without arguments (the palette, the buttons) a command opens its form in the timeline.
    const ask = (kind: BranchAction["kind"], chosen?: BranchId) => {
      game(editor);
      const { list, current, compare } = editor.branches.getState();
      let branch: BranchId | null = chosen ?? (kind === "compare" ? compare : current);
      if (kind === "delete" && list.find((info) => info.id === branch)?.parent === null) {
        // The picker lists no root: a root asked for by name is refused here.
        if (chosen !== undefined) throw new Error("the root branch cannot be deleted");
        branch = list.find((info) => info.parent !== null)?.id ?? null;
        if (branch === null) throw new Error("the root branch cannot be deleted, and there is no other");
      }
      forms.state.set({ action: { kind, branch }, error: null });
      showPanel(editor, "ludenea.timeline");
    };
    editor.commands.register({
      id: "timeline.switch-branch",
      title: "Switch branch",
      run: (_, branch) => (typeof branch === "number" ? forms.perform({ kind: "switch", branch }) : ask("switch")),
    });
    editor.commands.register({
      id: "timeline.rename-branch",
      title: "Rename branch",
      run: (_, branch, name) =>
        typeof branch === "number" && typeof name === "string" ? forms.perform({ kind: "rename", branch }, name) : ask("rename"),
    });
    editor.commands.register({
      id: "timeline.delete-branch",
      title: "Delete branch",
      // Always confirmed: the confirmation names the descendants deleted with it.
      run: (_, branch) => ask("delete", typeof branch === "number" ? branch : undefined),
    });
    editor.commands.register({
      id: "timeline.set-compare-branch",
      title: "Set compare branch",
      // `null` turns the ghost off.
      run: (...args) => (args.length > 1 ? forms.perform({ kind: "compare", branch: args[1] as BranchId | null }) : ask("compare")),
    });
    editor.commands.register({
      id: "timeline.save-replay",
      title: "Save branch as replay",
      async run(_, branch) {
        const path = await saveReplay(editor, typeof branch === "number" ? branch : null);
        shared(editor).reveal.set({ path });
      },
    });

    editor.shortcuts.register({ key: "F5", command: "timeline.play-pause" });
    editor.shortcuts.register({ key: ".", command: "timeline.step" });
    editor.shortcuts.register({ key: ",", command: "timeline.step-back" });
    editor.shortcuts.register({ key: "]", command: "timeline.speed-up" });
    editor.shortcuts.register({ key: "[", command: "timeline.speed-down" });
    editor.shortcuts.register({ key: "Mod+Z", command: "edit.undo" });
    editor.shortcuts.register({ key: "Mod+Shift+Z", command: "edit.redo" });

    editor.panels.register({
      id: "ludenea.timeline",
      title: "Timeline",
      defaultPosition: "bottom",
      component: ({ editor }) => <TimelinePanel editor={editor} speed={speed} forms={forms} />,
    });
  },
});
