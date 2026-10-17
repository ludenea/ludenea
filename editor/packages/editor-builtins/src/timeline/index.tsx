import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { createStore, type Store } from "../shared";
import { edit } from "./editing";
import { TimelinePanel } from "./TimelinePanel";

/** The speed last asked of the host, which no reply reports back; the host clamps it to [0.1, 8]. */
export type SpeedState = Store<{ speed: number }>;

const MIN_SPEED = 0.1;
const MAX_SPEED = 8;

function game(editor: Editor) {
  if (!editor.control) throw new Error("no game is running");
  return editor.control;
}

/** Moves the game to `tick` and shows what it holds there. */
export async function seek(editor: Editor, tick: number): Promise<void> {
  try {
    await game(editor).request({ type: "seek", tick });
  } finally {
    void editor.timeline.getState().refresh();
  }
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
    editor.commands.register({
      id: "edit.undo",
      title: "Undo",
      run: () => edit(editor, game(editor), { type: "undo" }).then(() => {}),
    });
    editor.commands.register({
      id: "edit.redo",
      title: "Redo",
      run: () => edit(editor, game(editor), { type: "redo" }).then(() => {}),
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
      component: ({ editor }) => <TimelinePanel editor={editor} speed={speed} />,
    });
  },
});
