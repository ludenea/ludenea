import type { PluginFailure } from "@ludenea/editor-shell";

/** What a small plugin slot that failed (a decoration, an explorer cell) shows: a mark naming the failure. */
export const failureMark = ({ text }: PluginFailure) => (
  <span role="img" aria-label={text} title={text} className="ludenea-error">
    ⚠
  </span>
);
