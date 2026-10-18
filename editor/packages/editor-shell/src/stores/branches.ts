import type { BranchId, BranchInfo, ControlClient } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";
import type { TimelineStore } from "./timeline";

export interface BranchState {
  list: BranchInfo[];
  current: BranchId;
  /** The branch the ghost shows, null once turned off. */
  compare: BranchId | null;
  switch(id: BranchId): Promise<void>;
  rename(id: BranchId, name: string): Promise<void>;
  /** Deletes the branch and its descendants; resolves with every branch deleted. */
  remove(id: BranchId): Promise<BranchId[]>;
  /** Shows `id` as the ghost; null turns the ghost off. */
  setCompare(id: BranchId | null): Promise<void>;
}

/** A Zustand store following the timeline the game reports (`Timeline`, `TimelineChanged`). */
export type BranchStore = UseBoundStore<StoreApi<BranchState>>;

const NONE: BranchInfo[] = [];

export function createBranchStore(control: () => ControlClient | null, timeline: TimelineStore): BranchStore {
  const game = () => {
    const current = control();
    if (!current) throw new Error("no game is connected");
    return current;
  };
  const store = create<BranchState>()(() => ({
    list: NONE,
    current: 0,
    compare: null,
    async switch(branch) {
      await game().request({ type: "switch_branch", branch });
    },
    async rename(branch, name) {
      await game().request({ type: "rename_branch", branch, name });
    },
    async remove(branch) {
      return (await game().request({ type: "delete_branch", branch })).branches;
    },
    async setCompare(branch) {
      await game().request({ type: "set_compare_branch", branch });
    },
  }));
  timeline.subscribe(({ timeline }) => {
    const { list, current, compare } = store.getState();
    const next = timeline ? { list: timeline.branches, current: timeline.branch, compare: timeline.compare } : { list: NONE, current: 0, compare: null };
    // Every tick changes the timeline; the branches seldom.
    if (next.list !== list || next.current !== current || next.compare !== compare) store.setState(next);
  });
  return store;
}
