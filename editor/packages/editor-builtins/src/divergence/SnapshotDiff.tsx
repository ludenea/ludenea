import type { Difference } from "@ludenea/control-client";
import { useStore } from "../shared";
import type { Viewer } from ".";

/** A field path such as `position.x` or `items[2]`; `value` for the whole value. */
export const fieldPath = (path: string[]) => path.map((segment, index) => (index === 0 || segment.startsWith("[") ? segment : `.${segment}`)).join("") || "value";

/** Differences grouped by subject, then component, in the host's order. */
function grouped(differences: Difference[]): Map<string, Map<string, Difference[]>> {
  const subjects = new Map<string, Map<string, Difference[]>>();
  for (const difference of differences) {
    const components = subjects.get(difference.subject) ?? new Map<string, Difference[]>();
    subjects.set(difference.subject, components);
    components.set(difference.component, [...(components.get(difference.component) ?? []), difference]);
  }
  return subjects;
}

/** A snapshot against another snapshot or the current tick: entity, component, field, with both values as the game's schemas write them. */
export function SnapshotDiff({ viewer, snapshots }: { viewer: Viewer; snapshots: string[] }) {
  const { path, against, result, error } = useStore(viewer.snapshot);
  return (
    <section aria-label="Snapshot comparison">
      <h3>{path}</h3>
      <label>
        Compare the snapshot with{" "}
        <select value={against ?? ""} onChange={(event) => void viewer.diffSnapshot({ against: event.target.value || null })}>
          <option value="">Current tick</option>
          {snapshots
            .filter((snapshot) => snapshot !== path)
            .map((snapshot) => (
              <option key={snapshot} value={snapshot}>
                {snapshot}
              </option>
            ))}
        </select>
      </label>
      {error && (
        <p role="alert" className="ludenea-error">
          {error}
        </p>
      )}
      {result?.length === 0 && <p className="ludenea-muted">The snapshots are identical.</p>}
      {result && result.length > 0 && (
        <ul role="tree" aria-label="Snapshot differences" className="ludenea-differences">
          {[...grouped(result)].map(([subject, components]) => (
            <li key={subject} role="treeitem" aria-label={subject} aria-expanded>
              <span className="ludenea-mono">{subject}</span>
              <ul role="group">
                {[...components].map(([component, differences]) => (
                  <li key={component} role="treeitem" aria-label={component} aria-expanded>
                    <span className="ludenea-mono">{component}</span>
                    <ul role="group">
                      {differences.map((difference) => (
                        <li key={difference.path.join("/")} role="treeitem" className="ludenea-difference">
                          <span>{fieldPath(difference.path)}</span>
                          <code title="left">{difference.left_text}</code>
                          <code title="right">{difference.right_text}</code>
                        </li>
                      ))}
                    </ul>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
