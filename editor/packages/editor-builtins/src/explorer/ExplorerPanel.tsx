import type { ControlClient } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useState } from "react";
import { useGame, useStore, type Store } from "../shared";
import { EntityList, entityKey } from "./EntityList";
import { Introspection, type IntrospectionTab } from "./Introspection";

const TABS = ["Entities", "Schedule", "Events", "Commands"] as const;
export type Tab = (typeof TABS)[number];
/** The shown tab and the entity query as typed and as applied. */
export type ExplorerState = Store<{ tab: Tab; text: string; filter: string }>;

export function ExplorerPanel({ editor, state }: { editor: Editor; state: ExplorerState }) {
  const control = useGame(editor);
  const { tab } = useStore(state);
  const setTab = (tab: Tab) => state.set({ tab });
  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  return (
    <div className="ludenea-explorer">
      <div role="tablist" aria-label="Explorer">
        {TABS.map((name) => (
          <button key={name} role="tab" aria-selected={name === tab} onClick={() => setTab(name)}>
            {name}
          </button>
        ))}
      </div>
      <div role="tabpanel" aria-label={tab}>
        {tab === "Entities" ? <Entities editor={editor} control={control} state={state} /> : <Introspection editor={editor} control={control} tab={tab as IntrospectionTab} />}
      </div>
    </div>
  );
}

/** The entities matching a query written like the code, such as `With<Body>, Without<Dead>`. */
function Entities({ editor, control, state }: { editor: Editor; control: ControlClient; state: ExplorerState }) {
  const { text, filter } = useStore(state);
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="ludenea-entities">
      <form
        onSubmit={(event) => {
          event.preventDefault();
          state.set({ filter: text.trim() });
        }}
      >
        <input type="search" aria-label="Entity query" placeholder="With<Body>, Without<Dead>" value={text} onChange={(event) => state.set({ text: event.target.value })} />
      </form>
      {error && (
        <p role="alert" className="ludenea-error">
          {error}
        </p>
      )}
      <div className="ludenea-entities-list">
        <EntityList
          editor={editor}
          control={control}
          filter={filter}
          label="Matching entities"
          onError={setError}
          row={(row) => (
            <>
              <span className="ludenea-mono">{entityKey(row.entity)}</span> <code className="ludenea-muted">{row.hash}</code>
            </>
          )}
        />
      </div>
    </div>
  );
}
