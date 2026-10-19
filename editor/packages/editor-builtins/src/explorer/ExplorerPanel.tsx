import type { ControlClient, Entity } from "@ludenea/control-client";
import { PluginBoundary, type Editor, type ExplorerColumn } from "@ludenea/editor-shell";
import { useEffect, useState, useSyncExternalStore } from "react";
import { useGame, useStore, type Store } from "../shared";
import { EntityList, entityKey } from "../shared/EntityList";
import { failureMark } from "../shared/FailureMark";
import { Introspection, type IntrospectionTab } from "./Introspection";

const TABS = ["Entities", "Schedule", "Events", "Commands"] as const;
export type Tab = (typeof TABS)[number];
/** The shown tab, the entity query as typed and as applied, and the list's scroll position. */
export type ExplorerState = Store<{ tab: Tab; text: string; filter: string; scroll: { top: number } }>;

/**
 * `failedColumns`: the columns whose subscription failed and is reported already, kept across
 * mounts: once per registration.
 */
export function ExplorerPanel({ editor, state, failedColumns }: { editor: Editor; state: ExplorerState; failedColumns: WeakSet<ExplorerColumn> }) {
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
        {tab === "Entities" ? <Entities editor={editor} control={control} state={state} failedColumns={failedColumns} /> : <Introspection editor={editor} control={control} tab={tab as IntrospectionTab} />}
      </div>
    </div>
  );
}

/** The entities matching a query written like the code, such as `With<Body>, Without<Dead>`. */
function Entities({ editor, control, state, failedColumns }: { editor: Editor; control: ControlClient; state: ExplorerState; failedColumns: WeakSet<ExplorerColumn> }) {
  const { text, filter, scroll } = useStore(state);
  const [error, setError] = useState<string | null>(null);
  const columns = useSyncExternalStore(editor.explorer.columns.subscribe, editor.explorer.columns.list);
  // A column whose values changed: the rows read them again.
  const [values, setValues] = useState(0);
  // A subscribe or unsubscribe that throws costs only that column's subscription, reported once.
  useEffect(() => {
    const stops = columns.map((column) => {
      const plugin = editor.pluginOf(column);
      const slot = `the "${column.title}" explorer column${plugin === undefined ? "" : ` of plugin "${plugin}"`}`;
      const failed = (doing: string, error: unknown) => {
        if (failedColumns.has(column)) return;
        failedColumns.add(column);
        editor.console.report(error, `${doing} ${slot}`);
      };
      try {
        const stop = column.subscribe?.(() => setValues((version) => version + 1));
        return () => {
          try {
            stop?.();
          } catch (error) {
            failed("Unsubscribing from", error);
          }
        };
      } catch (error) {
        failed("Subscribing to", error);
        return () => {};
      }
    });
    return () => stops.forEach((stop) => stop());
  }, [editor, columns, failedColumns]);
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
      {columns.length > 0 && (
        <div role="row" className="ludenea-columns">
          <span role="columnheader">Entity</span>
          {columns.map((column) => (
            <span key={column.id} role="columnheader" style={{ width: column.width }}>
              {column.title}
            </span>
          ))}
        </div>
      )}
      <div className="ludenea-entities-list">
        <EntityList
          editor={editor}
          control={control}
          filter={filter}
          label="Matching entities"
          onError={setError}
          scroll={scroll}
          row={(row) => (
            <>
              <span>
                <span className="ludenea-mono">{entityKey(row.entity)}</span> <code className="ludenea-muted">{row.hash}</code>
              </span>
              {columns.map((column) => (
                <span key={column.id} className="ludenea-column" style={{ width: column.width }}>
                  {/* A cell that threw tries again for another entity in its row, or new values. */}
                  <PluginBoundary key={`${entityKey(row.entity)} ${values}`} editor={editor} registration={column} name={`The "${column.title}" explorer column`} fallback={failureMark}>
                    <ColumnValue column={column} entity={row.entity} />
                  </PluginBoundary>
                </span>
              ))}
            </>
          )}
        />
      </div>
    </div>
  );
}

/** `value` read while rendering, inside the cell's boundary. */
function ColumnValue({ column, entity }: { column: ExplorerColumn; entity: Entity }) {
  return column.value(entity);
}
