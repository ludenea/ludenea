import type { Command, ControlClient, ReplyFor } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";

/** Throws while the host replays the recorded future: it refuses edits until then. */
export function assertEditable(editor: Editor): void {
  if (editor.timeline.getState().timeline?.resimulating) throw new Error("the game is resimulating; edit once it finishes");
}

const edited = new WeakMap<Editor, Set<() => void>>();

/** Calls `listener` after each editing command `edit` sends succeeds; returns the unsubscribe. */
export function onEdited(editor: Editor, listener: () => void): () => void {
  let listeners = edited.get(editor);
  if (!listeners) edited.set(editor, (listeners = new Set()));
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Sends an editing command (refused while resimulating), then refreshes the timeline and hash it changed. */
export async function edit<C extends Command>(editor: Editor, control: ControlClient, command: C): Promise<ReplyFor<C>> {
  assertEditable(editor);
  try {
    const reply = await control.request(command);
    for (const listener of [...(edited.get(editor) ?? [])]) listener();
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
