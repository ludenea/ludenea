import type { ControlClient, Entity, EntityRow } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import { shared, useGame, useStore } from "../shared";

const PAGE = 200;
const ROW_HEIGHT = 22;
const LABEL_COMPONENTS = 3;
const ALL = { type: "entities", filter: "" } as const;

const key = (entity: Entity) => `${entity.index}v${entity.generation}`;
const shortName = (name: string) => name.split("::").at(-1)!;
const warn = (what: string) => (error: unknown) => console.warn(`${what}: ${String(error)}`);

export function SceneTree({ editor }: { editor: Editor }) {
  const control = useGame(editor);
  const { build } = useStore(shared(editor).project);
  if (!control) return <p className="ludenea-muted">{build.status === "building" ? "Building the game..." : "No game is running."}</p>;
  return <EntityList editor={editor} control={control} />;
}

/** Every entity of the current tick, paged in as the visible rows need them. */
function EntityList({ editor, control }: { editor: Editor; control: ControlClient }) {
  const [total, setTotal] = useState(0);
  const [version, setVersion] = useState(0);
  const [rows, setRows] = useState(() => new Map<number, EntityRow>());
  // ponytail: labels are cached per entity for one game, so adding or removing a component keeps
  // the old label; refetch on `entity_changed` if that matters.
  const [labels, setLabels] = useState(() => new Map<string, string>());
  const requested = useRef(new Set<string>());
  const scroller = useRef<HTMLDivElement>(null);
  const selection = editor.selection((state) => state.entities);
  const virtualizer = useVirtualizer({ count: total, getScrollElement: () => scroller.current, estimateSize: () => ROW_HEIGHT, overscan: 10 });
  const items = virtualizer.getVirtualItems();
  const firstPage = Math.floor((items[0]?.index ?? 0) / PAGE);
  const lastPage = Math.floor((items.at(-1)?.index ?? 0) / PAGE);

  useEffect(() => {
    setRows(new Map());
    setLabels(new Map());
    requested.current.clear();
    control.request({ type: "subscribe", topic: ALL }).catch(warn("subscribe to entities"));
    const stop = control.on("entities_changed", (event) => {
      if (event.filter === ALL.filter) setVersion((version) => version + 1);
    });
    return () => {
      stop();
      control.request({ type: "unsubscribe", topic: ALL }).catch(() => {});
    };
  }, [control]);

  // ponytail: refetches the visible pages on every change (each tick while playing); coalesce
  // to one request in flight if that shows up in profiles.
  useEffect(() => {
    let current = true;
    for (let page = firstPage; page <= lastPage; page++) {
      control
        .request({ type: "query_entities", filter: ALL.filter, offset: page * PAGE, limit: PAGE })
        .then((reply) => {
          if (!current) return;
          setTotal(reply.total);
          setRows((rows) => {
            const next = new Map(rows);
            reply.rows.forEach((row, index) => next.set(page * PAGE + index, row));
            return next;
          });
        })
        .catch(warn("query entities"));
    }
    return () => {
      current = false;
    };
  }, [control, version, firstPage, lastPage]);

  useEffect(() => {
    for (const item of items) {
      const entity = rows.get(item.index)?.entity;
      if (!entity || requested.current.has(key(entity))) continue;
      requested.current.add(key(entity));
      control
        .request({ type: "get_entity", entity })
        .then((reply) => {
          const names = Object.keys(reply.components).slice(0, LABEL_COMPONENTS).map(shortName);
          setLabels((labels) => new Map(labels).set(key(entity), names.join(", ") || "(no components)"));
        })
        .catch(warn(`entity ${key(entity)}`));
    }
  });

  const selected = new Set(selection.map(key));
  // Click or Enter selects; with Shift, Ctrl, or Cmd it adds to the selection.
  const choose = (entity: Entity, event: MouseEvent | KeyboardEvent) => {
    const store = editor.selection.getState();
    if (event.shiftKey || event.ctrlKey || event.metaKey) store.add(entity);
    else store.select([entity]);
  };

  return (
    <div ref={scroller} className="ludenea-scene-tree" style={{ height: "100%", overflow: "auto" }}>
      <ul role="listbox" aria-label="Entities" aria-multiselectable style={{ height: virtualizer.getTotalSize(), position: "relative", margin: 0, padding: 0 }}>
        {items.map((item) => {
          const row = rows.get(item.index);
          return (
            <li
              key={item.key}
              role="option"
              aria-selected={row ? selected.has(key(row.entity)) : false}
              tabIndex={row ? 0 : -1}
              onClick={(event) => row && choose(row.entity, event)}
              onKeyDown={(event) => {
                if (row && event.key === "Enter") choose(row.entity, event);
              }}
              style={{ position: "absolute", top: 0, left: 0, right: 0, height: item.size, transform: `translateY(${item.start}px)`, listStyle: "none" }}
            >
              {row ? (
                <>
                  <span>{labels.get(key(row.entity)) ?? ""}</span> <span className="ludenea-muted">{key(row.entity)}</span>
                </>
              ) : (
                "..."
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
