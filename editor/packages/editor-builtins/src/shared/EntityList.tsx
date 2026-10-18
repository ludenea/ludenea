import type { ControlClient, Entity, EntityRow } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { errorText } from ".";

const PAGE = 200;
const ROW_HEIGHT = 22;

export const entityKey = (entity: Entity) => `${entity.index}v${entity.generation}`;

/**
 * The entities matching `filter` at the current tick, paged in by `QueryEntities` as the
 * visible rows need them and refreshed when the `Entities` topic reports a change. A filter
 * the host rejects (`InvalidFilter`, naming the candidates) goes to `onError`.
 */
export function EntityList(props: {
  editor: Editor;
  control: ControlClient;
  filter: string;
  label: string;
  row(row: EntityRow): ReactNode;
  onError(error: string | null): void;
}) {
  const { editor, control, filter, label, row: content, onError } = props;
  const [total, setTotal] = useState(0);
  const [version, setVersion] = useState(0);
  const [rows, setRows] = useState(() => new Map<number, EntityRow>());
  const scroller = useRef<HTMLDivElement>(null);
  const report = useRef(onError);
  report.current = onError;
  const selection = editor.selection((state) => state.entities);
  const virtualizer = useVirtualizer({ count: total, getScrollElement: () => scroller.current, estimateSize: () => ROW_HEIGHT, overscan: 10 });
  const items = virtualizer.getVirtualItems();
  const firstPage = Math.floor((items[0]?.index ?? 0) / PAGE);
  const lastPage = Math.floor((items.at(-1)?.index ?? 0) / PAGE);

  useEffect(() => {
    setTotal(0);
    setRows(new Map());
    const topic = { type: "entities", filter } as const;
    control.request({ type: "subscribe", topic }).catch((error) => report.current(errorText(error)));
    const stop = control.on("entities_changed", (event) => {
      if (event.filter === filter) setVersion((version) => version + 1);
    });
    return () => {
      stop();
      control.request({ type: "unsubscribe", topic }).catch(() => {});
    };
  }, [control, filter]);

  // ponytail: refetches the visible pages on every change (each tick while playing); coalesce
  // to one request in flight if that shows up in profiles.
  useEffect(() => {
    let current = true;
    for (let page = firstPage; page <= lastPage; page++) {
      control.request({ type: "query_entities", filter, offset: page * PAGE, limit: PAGE }).then(
        (reply) => {
          if (!current) return;
          report.current(null);
          setTotal(reply.total);
          setRows((rows) => {
            const next = new Map(rows);
            reply.rows.forEach((row, index) => next.set(page * PAGE + index, row));
            return next;
          });
        },
        (error) => current && report.current(errorText(error)),
      );
    }
    return () => {
      current = false;
    };
  }, [control, filter, version, firstPage, lastPage]);

  const selected = new Set(selection.map(entityKey));
  // Click or Enter selects; with Shift, Ctrl, or Cmd it adds to the selection.
  const choose = (entity: Entity, event: MouseEvent | KeyboardEvent) => {
    const store = editor.selection.getState();
    if (event.shiftKey || event.ctrlKey || event.metaKey) store.add(entity);
    else store.select([entity]);
  };

  return (
    <div ref={scroller} className="ludenea-entity-list" style={{ height: "100%", overflow: "auto" }}>
      <ul role="listbox" aria-label={label} aria-multiselectable style={{ height: virtualizer.getTotalSize(), position: "relative", margin: 0, padding: 0 }}>
        {items.map((item) => {
          const row = rows.get(item.index);
          return (
            <li
              key={item.key}
              role="option"
              aria-selected={row ? selected.has(entityKey(row.entity)) : false}
              tabIndex={row ? 0 : -1}
              onClick={(event) => row && choose(row.entity, event)}
              onKeyDown={(event) => {
                if (row && event.key === "Enter") choose(row.entity, event);
              }}
              style={{ position: "absolute", top: 0, left: 0, right: 0, height: item.size, transform: `translateY(${item.start}px)`, listStyle: "none" }}
            >
              {row ? content(row) : "..."}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
