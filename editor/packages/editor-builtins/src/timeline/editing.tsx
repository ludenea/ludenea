import type { Command, ControlClient, ReplyFor } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";

/** Throws while the host replays the recorded future: it refuses edits until then. */
export function assertEditable(editor: Editor): void {
  if (editor.timeline.getState().timeline?.resimulating) throw new Error("the game is resimulating; edit once it finishes");
}

const edited = new WeakMap<Editor, Set<(command: Command) => void>>();

/** Calls `listener` with each editing command `edit` sends, once it succeeds; returns the unsubscribe. */
export function onEdited(editor: Editor, listener: (command: Command) => void): () => void {
  let listeners = edited.get(editor);
  if (!listeners) edited.set(editor, (listeners = new Set()));
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * One editing command for `commands`: the command itself when there is one, otherwise a batch,
 * which the game applies as one edit (one undo step) or, when one of them fails, not at all.
 */
export function batched(commands: Command[]): Command {
  const [only] = commands;
  return commands.length === 1 && only ? only : { type: "batch", commands };
}

/** Sends an editing command (refused while resimulating), then refreshes the timeline and hash it changed. */
export async function edit<C extends Command>(editor: Editor, control: ControlClient, command: C): Promise<ReplyFor<C>> {
  assertEditable(editor);
  try {
    const reply = await control.request(command);
    for (const listener of [...(edited.get(editor) ?? [])]) listener(command);
    return reply;
  } finally {
    void editor.timeline.getState().refresh();
  }
}

export const useResimulating = (editor: Editor): boolean => editor.timeline((state) => state.timeline?.resimulating ?? false);

/**
 * Shown by editing panels while edits are refused. It selects plain numbers, null unless
 * resimulating, so the ticks of a playing game do not render it again.
 */
export function ResimulatingNotice({ editor }: { editor: Editor }) {
  const simulated = editor.timeline((state) => (state.timeline?.resimulating ? state.timeline.simulated : null));
  const head = editor.timeline((state) => (state.timeline?.resimulating ? state.timeline.head : null));
  if (simulated === null || head === null) return null;
  return (
    <p role="status" className="ludenea-muted">
      Resimulating (tick {simulated} of {head}): editing resumes when it finishes.
    </p>
  );
}
