import type { BranchId, BranchInfo } from "@ludenea/control-client";
import type { BranchState, Editor } from "@ludenea/editor-shell";
import { memo, useRef } from "react";
import { errorText, useStore, type Store } from "../shared";

/** The picker, rename form, or delete confirmation the timeline shows; `branch` null is "no compare branch". */
export type BranchAction = { kind: "switch" | "compare" | "rename" | "delete"; branch: BranchId | null };

export interface BranchForms {
  state: Store<{ action: BranchAction | null; error: string | null }>;
  /** Does what `action` asks (`name` is the new name of a rename) and closes the form. */
  perform(action: BranchAction, name?: string): Promise<void>;
}

/** Every branch forked from `id`, directly or not, depth first. */
export function descendants(list: BranchInfo[], id: BranchId): BranchInfo[] {
  return list.filter((branch) => branch.parent === id).flatMap((child) => [child, ...descendants(list, child.id)]);
}

const at = (tick: number, scale: number) => `${(tick / scale) * 100}%`;
// What the lanes show, as a string: a playing game sends a new timeline every tick, with the same branches.
const laneKey = ({ list, current, compare }: BranchState) => JSON.stringify([list, current, compare]);

/** One lane per branch, each under its parent, from its fork point to its last recorded tick, with its edits; clicking one switches to it. */
export const BranchLanes = memo(function BranchLanes({ editor, onError }: { editor: Editor; onError?: (error: string) => void }) {
  editor.branches(laneKey);
  const { list, current, compare } = editor.branches.getState();
  const scale = Math.max(1, ...list.map((branch) => branch.head));
  const ordered = list.filter((branch) => branch.parent === null).flatMap((root) => [root, ...descendants(list, root.id)]);
  const switchTo = (branch: BranchId) =>
    void editor.commands.run("timeline.switch-branch", branch).catch((failure) => onError?.(errorText(failure)));

  return (
    <ul aria-label="Branches" className="ludenea-lanes">
      {ordered.map((branch) => {
        return (
          <li key={branch.id}>
            <button aria-current={branch.id === current} onClick={() => switchTo(branch.id)}>
              <span className="ludenea-lane-name">{branch.name}</span>
              {branch.id === compare && <span className="ludenea-muted"> compare</span>}
              {branch.stale && <span className="ludenea-warning"> stale</span>}
              <span className="ludenea-lane-track">
                <span className="ludenea-lane-span" style={{ left: at(branch.fork_tick, scale), width: at(Math.max(0, branch.head - branch.fork_tick), scale) }} />
                {branch.parent !== null && (
                  <span
                    aria-label={`${branch.name} forks at tick ${branch.fork_tick}`}
                    title={`Forks at tick ${branch.fork_tick}`}
                    className="ludenea-mark-fork"
                    style={{ left: at(branch.fork_tick, scale) }}
                  />
                )}
                {branch.edit_ticks.map((tick) => (
                  <span key={tick} aria-label={`${branch.name} edit at tick ${tick}`} title={`Edit at tick ${tick}`} className="ludenea-mark-edit" style={{ left: at(tick, scale) }} />
                ))}
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
});

const VERB = { switch: "Switch", compare: "Compare", rename: "Rename", delete: "Delete" } as const;

/** The branch picker of the open action, with its rename field or delete confirmation. */
export function BranchForm({ editor, forms }: { editor: Editor; forms: BranchForms }) {
  const { action, error } = useStore(forms.state);
  const list = editor.branches((state) => state.list);
  const name = useRef<HTMLInputElement>(null);
  if (!action) return null;
  const nameOf = (id: BranchId) => list.find((branch) => branch.id === id)?.name ?? `branch ${id}`;
  const choices = action.kind === "delete" ? list.filter((branch) => branch.parent !== null) : list;
  const set = (patch: Partial<BranchAction>) => forms.state.set({ action: { ...action, ...patch }, error: null });
  const submit = () =>
    void forms.perform(action, name.current?.value.trim()).catch((failure) => forms.state.set({ error: errorText(failure) }));
  const chosen = action.branch;
  const removed = chosen === null ? [] : descendants(list, chosen).map((branch) => branch.name);

  return (
    <div className="ludenea-confirm" role={action.kind === "delete" ? "alertdialog" : "group"} aria-label={`${VERB[action.kind]} branch`}>
      {action.kind === "delete" && chosen !== null && (
        <span>
          Delete {nameOf(chosen)}
          {removed.length > 0 && ` and its descendants ${removed.join(", ")}`}?
        </span>
      )}
      <select aria-label="Branch" value={chosen ?? ""} onChange={(event) => set({ branch: event.target.value === "" ? null : Number(event.target.value) })}>
        {action.kind === "compare" && <option value="">None</option>}
        {choices.map((branch) => (
          <option key={branch.id} value={branch.id}>
            {branch.name}
          </option>
        ))}
      </select>
      {action.kind === "rename" && chosen !== null && (
        <input
          key={chosen}
          ref={name}
          aria-label="New name"
          defaultValue={nameOf(chosen)}
          onKeyDown={(event) => event.key === "Enter" && submit()}
        />
      )}
      <button aria-label={action.kind === "delete" && chosen !== null ? `Delete ${nameOf(chosen)}` : undefined} onClick={submit}>
        {VERB[action.kind]}
      </button>
      <button onClick={() => forms.state.set({ action: null, error: null })}>Cancel</button>
      {error && <p className="ludenea-error">{error}</p>}
    </div>
  );
}
