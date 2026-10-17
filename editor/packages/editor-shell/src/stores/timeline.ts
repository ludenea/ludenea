import type { ControlClient, Timeline } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";

export interface TimelineState {
  timeline: Timeline | null;
  hash: string | null;
  /** The hash shown includes edits not yet stepped into a recorded tick. */
  pending: boolean;
  playing: boolean;
  effectiveRate: number;
  divergence: { tick: number } | null;
}

/** A Zustand store fed by the shell from the game's events. */
export type TimelineStore = UseBoundStore<StoreApi<TimelineState>>;

export function createTimelineStore(): TimelineStore {
  return create<TimelineState>()(() => ({
    timeline: null,
    hash: null,
    pending: false,
    playing: false,
    effectiveRate: 0,
    divergence: null,
  }));
}

/** Feeds `store` from `control`'s events until the returned function is called. */
export function feedTimeline(control: ControlClient, store: TimelineStore): () => void {
  const stops = [
    control.on("ticked", (event) =>
      store.setState({ hash: event.hash, pending: false, playing: event.playing, effectiveRate: event.effective_rate }),
    ),
    control.on("timeline_changed", (event) => store.setState({ timeline: event.timeline })),
    control.on("paused", () => store.setState({ playing: false })),
    control.on("diverged", (event) => store.setState({ divergence: { tick: event.tick } })),
  ];
  return () => stops.forEach((stop) => stop());
}
