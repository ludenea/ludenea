import type { Entity } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";

export interface SelectionState {
  entities: Entity[];
  select(entities: Entity[]): void;
  add(entity: Entity): void;
  clear(): void;
}

/** A Zustand store: call it as a React hook with a selector, or use `getState`/`subscribe`. */
export type SelectionStore = UseBoundStore<StoreApi<SelectionState>>;

const same = (a: Entity, b: Entity) => a.index === b.index && a.generation === b.generation;

export function createSelectionStore(): SelectionStore {
  return create<SelectionState>()((set) => ({
    entities: [],
    select: (entities) => set({ entities }),
    add: (entity) =>
      set(({ entities }) => (entities.some((selected) => same(selected, entity)) ? {} : { entities: [...entities, entity] })),
    clear: () => set({ entities: [] }),
  }));
}
