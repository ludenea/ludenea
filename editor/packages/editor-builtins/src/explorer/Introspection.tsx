import type { ControlClient } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useEffect, useState } from "react";
import { errorText } from "../shared";

export type IntrospectionTab = "Schedule" | "Events" | "Commands";

const TOPIC = { type: "introspection" } as const;

/**
 * The tabs reading what the host records per tick. The host records only while the
 * `Introspection` topic is subscribed, so this subscribes for as long as one of them shows.
 */
export function Introspection({ editor, control, tab }: { editor: Editor; control: ControlClient; tab: IntrospectionTab }) {
  useEffect(() => {
    control.request({ type: "subscribe", topic: TOPIC }).catch((error) => console.warn(`subscribe to introspection: ${String(error)}`));
    return () => void control.request({ type: "unsubscribe", topic: TOPIC }).catch(() => {});
  }, [control]);
  // Every tick and every seek changes the hash: refetch then.
  const hash = editor.timeline((state) => state.hash);
  const tick = editor.timeline((state) => state.timeline?.current ?? null);
  if (tab === "Schedule") return <Schedule control={control} hash={hash} />;
  if (tick === null) return <p className="ludenea-muted">No tick is selected.</p>;
  return tab === "Events" ? <Events control={control} tick={tick} hash={hash} /> : <Commands control={control} tick={tick} hash={hash} />;
}

/** The latest reply to `request`, sent again whenever `key` changes. */
function useReply<T>(request: () => Promise<T>, key: unknown[]): { reply: T | null; error: string | null } {
  const [state, setState] = useState<{ reply: T | null; error: string | null }>({ reply: null, error: null });
  useEffect(() => {
    let current = true;
    request().then(
      (reply) => current && setState({ reply, error: null }),
      (error) => current && setState({ reply: null, error: errorText(error) }),
    );
    return () => {
      current = false;
    };
    // `request` is rebuilt each render; `key` says when it changed.
  }, key);
  return state;
}

function Schedule({ control, hash }: { control: ControlClient; hash: string | null }) {
  const { reply, error } = useReply(() => control.request({ type: "get_schedule" }), [control, hash]);
  if (error) return <p className="ludenea-error">{error}</p>;
  return (
    <table aria-label="Schedule">
      <thead>
        <tr>
          <th>System</th>
          <th>Reads</th>
          <th>Writes</th>
          <th>Microseconds per tick</th>
        </tr>
      </thead>
      <tbody>
        {reply?.systems.map((system) => (
          <tr key={system.name} tabIndex={0}>
            <td>{system.name}</td>
            <td>{system.reads.join(", ")}</td>
            <td>{system.writes.join(", ")}</td>
            <td>{system.micros === null ? "not measured in the browser" : system.micros.toFixed(1)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Events({ control, tick, hash }: { control: ControlClient; tick: number; hash: string | null }) {
  const { reply, error } = useReply(() => control.request({ type: "get_tick_events", tick }), [control, tick, hash]);
  if (error) return <p className="ludenea-error">{error}</p>;
  if (reply?.events.length === 0) return <p className="ludenea-muted">No events at tick {tick}.</p>;
  return (
    <ul aria-label={`Events at tick ${tick}`}>
      {reply?.events.map((event, index) => (
        <li key={index} tabIndex={0}>
          <span>{event.event_type}</span> <code>{JSON.stringify(event.value)}</code>
        </li>
      ))}
    </ul>
  );
}

function Commands({ control, tick, hash }: { control: ControlClient; tick: number; hash: string | null }) {
  const { reply, error } = useReply(() => control.request({ type: "get_tick_commands", tick }), [control, tick, hash]);
  if (error) return <p className="ludenea-error">{error}</p>;
  if (reply?.sync_points.length === 0) return <p className="ludenea-muted">No commands at tick {tick}.</p>;
  return (
    <ul aria-label={`Commands at tick ${tick}`}>
      {reply?.sync_points.map((point, index) => (
        <li key={index} tabIndex={0}>
          After <strong>{point.after_system}</strong>
          <ul>
            {point.commands.map((command, position) => (
              <li key={position}>{command}</li>
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );
}
