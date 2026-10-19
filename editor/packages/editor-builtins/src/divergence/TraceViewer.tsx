import type { Editor } from "@ludenea/editor-shell";
import { useStore } from "../shared";
import type { Viewer } from ".";

const percent = (tick: number, total: number) => `${(100 * tick) / Math.max(total, 1)}%`;

/** A trace against another trace or the current branch: a strip of ticks with the first divergent one marked. */
export function TraceViewer({ editor, viewer, traces }: { editor: Editor; viewer: Viewer; traces: string[] }) {
  const { path, against, result, error } = useStore(viewer.trace);
  const total = result ? result.common + result.only_in_longer : 0;
  const divergent = result?.first_divergent ?? null;
  return (
    <section aria-label="Trace comparison">
      <h3>{path}</h3>
      <label>
        Compare the trace with{" "}
        <select value={against ?? ""} onChange={(event) => void viewer.compareTrace({ against: event.target.value || null })}>
          <option value="">Current branch</option>
          {traces
            .filter((trace) => trace !== path)
            .map((trace) => (
              <option key={trace} value={trace}>
                {trace}
              </option>
            ))}
        </select>
      </label>
      {error && (
        <p role="alert" className="ludenea-error">
          {error}
        </p>
      )}
      {result && (
        <>
          <div className="ludenea-trace-strip" style={{ position: "relative", height: 16, margin: "8px 0", background: "var(--color-border)" }}>
            {divergent !== null && (
              <span style={{ position: "absolute", top: 0, bottom: 0, left: percent(divergent, total), width: percent(result.common - divergent, total), background: "var(--color-danger)" }} />
            )}
            {result.only_in_longer > 0 && <span style={{ position: "absolute", top: 0, bottom: 0, right: 0, width: percent(result.only_in_longer, total), background: "var(--color-muted)" }} />}
            {divergent !== null && (
              <button
                aria-label={`First divergent tick ${divergent}`}
                title={against === null ? `Seek to tick ${divergent}` : "Open one side as the document to seek there"}
                disabled={against !== null}
                onClick={() => viewer.seekDivergence().catch((error) => editor.console.report(error, `Seeking to the first divergent tick ${divergent}`))}
                style={{ position: "absolute", top: -2, bottom: -2, left: percent(divergent, total), width: 4, padding: 0, border: 0, background: "var(--color-warning)" }}
              />
            )}
          </div>
          <p className="ludenea-muted">
            {divergent === null ? `The traces match over ${result.common} common ticks.` : `First divergent tick ${divergent} of ${result.common} common ticks.`}
            {result.only_in_longer > 0 && ` The longer trace has ${result.only_in_longer} more.`}
          </p>
        </>
      )}
    </section>
  );
}
