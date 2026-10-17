import type { Command, ControlClient, ReplyFor } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";

/** Throws while the host replays the recorded future: it refuses edits until then. */
export function assertEditable(editor: Editor): void {
  if (editor.timeline.getState().timeline?.resimulating) throw new Error("the game is resimulating; edit once it finishes");
}

/** Sends an editing command (refused while resimulating), then refreshes the timeline and hash it changed. */
export async function edit<C extends Command>(editor: Editor, control: ControlClient, command: C): Promise<ReplyFor<C>> {
  assertEditable(editor);
  try {
    return await control.request(command);
  } finally {
    void editor.timeline.getState().refresh();
  }
}

export const useResimulating = (editor: Editor): boolean => editor.timeline((state) => state.timeline?.resimulating ?? false);

/** Shown by editing panels while edits are refused. */
export function ResimulatingNotice({ editor }: { editor: Editor }) {
  const timeline = editor.timeline((state) => state.timeline);
  if (!timeline?.resimulating) return null;
  return (
    <p role="status" className="ludenea-muted">
      Resimulating (tick {timeline.simulated} of {timeline.head}): editing resumes when it finishes.
    </p>
  );
}
