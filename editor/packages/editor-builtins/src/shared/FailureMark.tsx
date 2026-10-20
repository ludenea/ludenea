import type { PluginFailure } from "@ludenea/editor-shell";

/** What a small plugin slot that failed (an explorer cell) shows: a mark naming the failure. */
export const failureMark = ({ text }: PluginFailure) => (
  <span role="img" aria-label={text} title={text} className="ludenea-error">
    ⚠
  </span>
);

/**
 * What a failed decoration shows: a mark naming the failure that renders it again when clicked,
 * for a decoration that fails until state it reads loads while its field stays the same.
 */
export const retryMark = ({ text, retry }: PluginFailure) => (
  <button type="button" aria-label={text} title={`${text} (click to render it again)`} className="ludenea-error" onClick={retry}>
    ⚠
  </button>
);
