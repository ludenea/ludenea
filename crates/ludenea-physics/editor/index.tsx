import type { ControlClient, Entity } from "@ludenea/control-client";
import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { useEffect, useState } from "react";

const BODY = "ludenea::Body";
const KINDS = ["Static", "Kinematic", "Dynamic"];
const COLLISION = "ludenea::Collision";
const PAGE = 1000;
const INTROSPECTION = { type: "introspection" } as const;

type Contact = { kind: string; a: Entity; b: Entity };
type Physics = { counts: [string, number][]; contacts: Contact[]; note: string | null };

const key = (entity: Entity) => `${entity.index}v${entity.generation}`;
const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * The body's kind. Read untyped: `editor.entities` is typed by a game's generated
 * `ComponentTypes`, which an engine plugin must neither assume nor augment.
 */
async function kindOf(control: ControlClient, entity: Entity): Promise<string | undefined> {
  const body = (await control.request({ type: "get_entity", entity })).components[BODY];
  return body?.type === "json" ? (body.value as { kind: string }).kind : undefined;
}

/** Body counts by kind at the current tick, and the contacts that began or ended at `tick`. */
async function read(control: ControlClient, tick: number | null): Promise<Physics> {
  const counts = new Map(KINDS.map((kind) => [kind, 0]));
  // ponytail: reads every body on each refresh (each tick while playing); count on the host
  // if large scenes make this slow.
  for (let offset = 0, total = 1; offset < total; offset += PAGE) {
    const page = await control.request({ type: "query_entities", filter: `With<${BODY}>`, offset, limit: PAGE });
    if (page.rows.length === 0) break;
    total = page.total;
    const bodies = await Promise.all(page.rows.map((row) => kindOf(control, row.entity)));
    for (const kind of bodies) if (kind) counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  if (tick === null) return { counts: [...counts], contacts: [], note: null };
  try {
    const { events } = await control.request({ type: "get_tick_events", tick });
    const contacts = events.filter((event) => event.event_type === COLLISION).map((event) => event.value as Contact);
    return { counts: [...counts], contacts, note: null };
  } catch (error) {
    // `InvalidTick`: the tick ran before the recording started.
    return { counts: [...counts], contacts: [], note: message(error) };
  }
}

function PhysicsPanel({ editor }: { editor: Editor }) {
  const [control, setControl] = useState(editor.control);
  const [physics, setPhysics] = useState<Physics | null>(null);
  const [error, setError] = useState<string | null>(null);
  const hash = editor.timeline((state) => state.hash);
  const tick = editor.timeline((state) => state.timeline?.current ?? null);

  useEffect(() => {
    const subscription = editor.onGameChanged(setControl);
    return () => subscription.dispose();
  }, [editor]);
  // The host records tick events only while `Introspection` is subscribed.
  useEffect(() => {
    if (!control) return;
    control.request({ type: "subscribe", topic: INTROSPECTION }).catch((failure) => setError(message(failure)));
    return () => void control.request({ type: "unsubscribe", topic: INTROSPECTION }).catch(() => {});
  }, [control]);
  useEffect(() => {
    if (!control) return;
    let current = true;
    read(control, tick).then(
      (next) => {
        if (!current) return;
        setPhysics(next);
        setError(null);
      },
      (failure) => current && setError(message(failure)),
    );
    return () => {
      current = false;
    };
  }, [editor, control, tick, hash]);

  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  return (
    <div className="ludenea-physics">
      {error && <p className="ludenea-error">{error}</p>}
      <table aria-label="Bodies">
        <tbody>
          {physics?.counts.map(([kind, count]) => (
            <tr key={kind}>
              <th scope="row">{kind}</th>
              <td>{count}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <h3>Contacts{tick === null ? "" : ` at tick ${tick}`}</h3>
      {physics?.note && <p className="ludenea-muted">{physics.note}</p>}
      <ul aria-label="Contacts">
        {physics?.contacts.map((contact, index) => (
          <li key={index} tabIndex={0}>
            {contact.kind} {key(contact.a)} and {key(contact.b)}
          </li>
        ))}
      </ul>
    </div>
  );
}

export default definePlugin({
  name: "ludenea_physics",
  activate(editor) {
    editor.panels.register({ id: "ludenea_physics.panel", title: "Physics", defaultPosition: "right", component: PhysicsPanel });
  },
});
