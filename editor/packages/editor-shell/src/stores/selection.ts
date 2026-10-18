import type { AssetId, Entity } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";

export interface SelectionState {
  entities: Entity[];
  /** The data asset the inspector edits; selecting entities clears it, and selecting it clears them. */
  asset: AssetId | null;
  select(entities: Entity[]): void;
  add(entity: Entity): void;
  selectAsset(id: AssetId): void;
  clear(): void;
}

/** A Zustand store: call it as a React hook with a selector, or use `getState`/`subscribe`. */
export type SelectionStore = UseBoundStore<StoreApi<SelectionState>>;

const same = (a: Entity, b: Entity) => a.index === b.index && a.generation === b.generation;

export function createSelectionStore(): SelectionStore {
  return create<SelectionState>()((set) => ({
    entities: [],
    asset: null,
    select: (entities) => set({ entities, asset: null }),
    add: (entity) =>
      set(({ entities }) => (entities.some((selected) => same(selected, entity)) ? {} : { entities: [...entities, entity], asset: null })),
    selectAsset: (id) => set({ entities: [], asset: id }),
    clear: () => set({ entities: [], asset: null }),
  }));
}
