import type { ComponentValue, ControlClient, Entity, JsonValue, PathSegment } from "@ludenea/control-client";
import type { EditorConsole } from "./console";
import type { Disposable } from "./plugin";

/**
 * Stable component name to value type. Empty here; a project's generated `game.d.ts`
 * augments it from the game's schemas (64-bit integers are `number | string`).
 */
export interface ComponentTypes {}

/** Typed access to components, over `GetEntity`, `SetField`, and the `Entity` topic. */
export interface EntityAccess {
  /** The component's value, or undefined when absent or without an exact JSON form (read RON through `control`). */
  get<N extends keyof ComponentTypes>(entity: Entity, component: N): Promise<ComponentTypes[N] | undefined>;
  set<N extends keyof ComponentTypes>(entity: Entity, component: N, path: PathSegment[], value: unknown): Promise<void>;
  /**
   * Changed components (JSON values only) and removed component names, across game restarts;
   * `despawned`, when given, hears that the entity is no longer alive at the tick shown (an
   * undo of its spawn, a seek before it, another branch). Alive again (a redo), it reports
   * all its components as changed.
   */
  watch(entity: Entity, listener: (components: Partial<ComponentTypes>, removed: string[]) => void, despawned?: () => void): Disposable;
}

type Watch = { entity: Entity; listener: (components: Partial<ComponentTypes>, removed: string[]) => void; despawned?: () => void };

function jsonValues(components: { [name: string]: ComponentValue }): { [name: string]: JsonValue } {
  const values: { [name: string]: JsonValue } = {};
  for (const [name, value] of Object.entries(components)) if (value.type === "json") values[name] = value.value;
  return values;
}

const key = (entity: Entity) => `${entity.index}v${entity.generation}`;

export function createEntityAccess(
  control: () => ControlClient | null,
  onGameChanged: (listener: (control: ControlClient | null) => void) => void,
  own: (dispose: () => void) => Disposable,
  report: EditorConsole["report"],
): EntityAccess {
  const watches = new Set<Watch>();
  let stopEvents: (() => void)[] = [];

  const game = () => {
    const current = control();
    if (!current) throw new Error("no game is connected");
    return current;
  };
  // Each watch subscribes; the client counts them, so the host hears the first and the last.
  const subscription = (type: "subscribe" | "unsubscribe", entity: Entity) =>
    void control()
      ?.request({ type, topic: { type: "entity", entity } })
      .catch((error) => report(error, `${type === "subscribe" ? "Watching" : "Unwatching"} entity ${key(entity)}`));
  const watched = (entity: Entity) => [...watches].filter((watch) => key(watch.entity) === key(entity));
  const listen = (current: ControlClient | null) => {
    stopEvents.forEach((stop) => stop());
    stopEvents = current
      ? [
          current.on("entity_changed", (event) => {
            const matching = watched(event.entity);
            if (matching.length === 0) return;
            const components = jsonValues(event.components) as Partial<ComponentTypes>;
            for (const watch of matching) watch.listener(components, event.removed);
          }),
          current.on("entity_despawned", (event) => {
            for (const watch of watched(event.entity)) watch.despawned?.();
          }),
        ]
      : [];
  };

  listen(control());
  onGameChanged((current) => {
    listen(current);
    for (const watch of watches) subscription("subscribe", watch.entity);
  });

  return {
    async get(entity, component) {
      const reply = await game().request({ type: "get_entity", entity });
      const value = reply.components[component as string];
      return value?.type === "json" ? (value.value as ComponentTypes[typeof component]) : undefined;
    },
    async set(entity, component, path, value) {
      const target = { type: "component" as const, entity, component: component as string };
      await game().request({ type: "set_field", target, path, value: value as JsonValue });
    },
    watch(entity, listener, despawned) {
      subscription("subscribe", entity);
      const watch = { entity, listener, despawned };
      watches.add(watch);
      return own(() => {
        watches.delete(watch);
        subscription("unsubscribe", entity);
      });
    },
  };
}
