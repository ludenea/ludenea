import type { ControlClient } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useState } from "react";
import { useGame } from "../shared";
import { EntityList, entityKey } from "./EntityList";
import { Introspection, type IntrospectionTab } from "./Introspection";

const TABS = ["Entities", "Schedule", "Events", "Commands"] as const;
type Tab = (typeof TABS)[number];

export function ExplorerPanel({ editor }: { editor: Editor }) {
  const control = useGame(editor);
  const [tab, setTab] = useState<Tab>("Entities");
  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  return (
    <div className="ludenea-explorer" style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div role="tablist" aria-label="Explorer">
        {TABS.map((name) => (
          <button key={name} role="tab" aria-selected={name === tab} onClick={() => setTab(name)}>
            {name}
          </button>
        ))}
      </div>
      <div role="tabpanel" aria-label={tab} style={{ flex: 1, minHeight: 0 }}>
        {tab === "Entities" ? <Entities editor={editor} control={control} /> : <Introspection editor={editor} control={control} tab={tab as IntrospectionTab} />}
      </div>
    </div>
  );
}

/** The entities matching a query written like the code, such as `With<Body>, Without<Dead>`. */
function Entities({ editor, control }: { editor: Editor; control: ControlClient }) {
  const [text, setText] = useState("");
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);
  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          setFilter(text.trim());
        }}
      >
        <input type="search" aria-label="Entity query" placeholder="With<Body>, Without<Dead>" value={text} onChange={(event) => setText(event.target.value)} />
      </form>
      {error && (
        <p role="alert" className="ludenea-error">
          {error}
        </p>
      )}
      <div style={{ flex: 1, minHeight: 0 }}>
        <EntityList
          editor={editor}
          control={control}
          filter={filter}
          label="Matching entities"
          onError={setError}
          row={(row) => (
            <>
              <span>{entityKey(row.entity)}</span> <code className="ludenea-muted">{row.hash}</code>
            </>
          )}
        />
      </div>
    </div>
  );
}
