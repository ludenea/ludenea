import type { Cause, Entity, EntityDivergence } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useEffect } from "react";
import { useGame, useStore } from "../shared";
import { entityKey } from "../shared/EntityList";
import type { Report } from ".";
import { fieldPath } from "./SnapshotDiff";

const KINDS: { [kind in EntityDivergence["kind"]]: string } = {
  changed: "changed",
  only_in_current: "only in the current branch",
  only_in_compare: "only in the compare branch",
  replaced: "replaced (other components)",
};

/** How the current branch diverges from the compare branch: the root cause, then each entity by tick with its cause candidates. */
export function CausalReport({ editor, report }: { editor: Editor; report: Report }) {
  const control = useGame(editor);
  const compare = editor.branches((state) => state.compare);
  const { report: divergence, error } = useStore(report.state);
  useEffect(() => report.watch(), [report]);
  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  if (compare === null) return <p className="ludenea-muted">No compare branch: choose one in the timeline to see how the branches diverge.</p>;
  const select = (tick: number, ...entities: Entity[]) => report.select(tick, entities).catch((error) => editor.console.report(error, `Seeking to tick ${tick}`));
  const cause = (owner: EntityDivergence, cause: Cause) =>
    cause.type === "event" ? (
      <button onClick={() => select(cause.tick, owner.entity, cause.other)}>
        {cause.event} ({cause.path}) with {entityKey(cause.other)} at tick {cause.tick}
      </button>
    ) : (
      <button onClick={() => select(cause.tick, owner.entity)}>
        {cause.kind} by {cause.system} at tick {cause.tick}
      </button>
    );
  return (
    <div className="ludenea-divergence-report">
      {error && (
        <p role="alert" className="ludenea-error">
          {error}
        </p>
      )}
      {divergence && (
        <>
          <p className="ludenea-muted">
            Fork at tick {divergence.fork_tick}, compared up to tick {divergence.up_to}
            {divergence.complete ? "." : "…"} {divergence.first_divergent === null ? "No divergence yet." : `First divergent tick ${divergence.first_divergent}.`}
            {divergence.complete && <button onClick={() => report.extend()}>Extend</button>}
          </p>
          <ul role="tree" aria-label="Divergence report">
            {divergence.root_cause && (
              <li role="treeitem" aria-level={1} aria-label={`Root cause at tick ${divergence.root_cause.tick}`}>
                <button onClick={() => select(divergence.root_cause!.tick, ...divergence.root_cause!.entities)}>Root cause at tick {divergence.root_cause.tick}</button>{" "}
                {divergence.root_cause.kind.type === "edit" ? divergence.root_cause.kind.description : "take over"}
                <ul role="group">
                  {divergence.root_cause.entities.length === 0 && <li className="ludenea-muted">No entity diverged within the compared range yet.</li>}
                  {divergence.root_cause.entities.map((entity) => (
                    <li key={entityKey(entity)} role="treeitem" aria-level={2}>
                      <button onClick={() => select(divergence.root_cause!.tick, entity)}>{entityKey(entity)}</button>
                    </li>
                  ))}
                </ul>
              </li>
            )}
            {divergence.entities.map((row) => {
              const label = `${entityKey(row.entity)} at tick ${row.first_tick}`;
              const difference = row.first_difference;
              return (
                <li key={entityKey(row.entity)} role="treeitem" aria-level={1} aria-label={label}>
                  <button onClick={() => select(row.first_tick, row.entity)}>{label}</button> <span className="ludenea-muted">{KINDS[row.kind]}</span>
                  {difference && (
                    <div className="ludenea-mono">
                      {difference.component} {fieldPath(difference.path)}: {difference.left_text} → {difference.right_text}
                    </div>
                  )}
                  <ul role="group">
                    {row.causes.length === 0 && <li className="ludenea-muted">no recorded interaction</li>}
                    {row.causes.map((candidate, index) => (
                      <li key={index} role="treeitem" aria-level={2}>
                        {cause(row, candidate)}
                      </li>
                    ))}
                  </ul>
                </li>
              );
            })}
          </ul>
        </>
      )}
    </div>
  );
}
