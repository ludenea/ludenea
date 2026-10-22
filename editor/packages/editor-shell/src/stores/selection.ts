import type { AssetId, Entity } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";
import type { AssetKindName } from "../plugin";

export interface SelectionState {
  entities: Entity[];
  /** The asset the inspector shows (a data asset or a texture) and its kind; selecting entities clears it, and selecting it clears them. */
  asset: { id: AssetId; kind: AssetKindName } | null;
  select(entities: Entity[]): void;
  add(entity: Entity): void;
  selectAsset(id: AssetId, kind: AssetKindName): void;
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
    selectAsset: (id, kind) => set({ entities: [], asset: { id, kind } }),
    clear: () => set({ entities: [], asset: null }),
  }));
}
