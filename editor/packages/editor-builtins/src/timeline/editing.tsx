import type { Editor } from "@ludenea/editor-shell";

/** Throws while the host replays the recorded future, as `editor.edit` does: for a command to refuse before it asks anything. */
export function assertEditable(editor: Editor): void {
  if (editor.timeline.getState().timeline?.resimulating) throw new Error("the game is resimulating; edit once it finishes");
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
