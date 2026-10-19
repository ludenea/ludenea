import type { ControlClient, Timeline } from "@ludenea/control-client";
import { create, type StoreApi, type UseBoundStore } from "zustand";
import type { EditorConsole } from "../console";

export interface TimelineState {
  timeline: Timeline | null;
  hash: string | null;
  /** The hash shown includes edits not yet stepped into a recorded tick. */
  pending: boolean;
  playing: boolean;
  effectiveRate: number;
  /** The last `Diverged` report; kept until another replaces it or the game changes. */
  divergence: { tick: number } | null;
  /**
   * Fetches the timeline and the hash with its pending flag: run after an edit, a seek, a
   * step, or an undo. Failures go to the editor console, never thrown.
   */
  refresh(): Promise<void>;
}

/** A Zustand store fed by the shell from the game's events. */
export type TimelineStore = UseBoundStore<StoreApi<TimelineState>>;

const EMPTY = { timeline: null, hash: null, pending: false, playing: false, effectiveRate: 0, divergence: null };

export function createTimelineStore(control: () => ControlClient | null, report: EditorConsole["report"]): TimelineStore {
  return create<TimelineState>()((set) => ({
    ...EMPTY,
    async refresh() {
      const game = control();
      if (!game) return;
      try {
        const [timeline, hash] = await Promise.all([game.request({ type: "get_timeline" }), game.request({ type: "get_hash" })]);
        if (control() !== game) return;
        // A host that answers something else (a test double) leaves the store as it is.
        if (timeline.type === "timeline") {
          const { type: _, ...rest } = timeline;
          set({ timeline: rest });
        }
        if (hash.type === "hash") set({ hash: hash.hash, pending: hash.pending });
      } catch (error) {
        report(error, "Refreshing the timeline");
      }
    },
  }));
}

/** Empties `store`, then feeds it from `control`'s events until the returned function is called. */
export function feedTimeline(control: ControlClient | null, store: TimelineStore): () => void {
  store.setState(EMPTY);
  if (!control) return () => {};
  const stops = [
    control.on("ticked", (event) =>
      store.setState(({ timeline }) => ({
        timeline: timeline && { ...timeline, current: event.tick },
        hash: event.hash,
        pending: event.pending,
        playing: event.playing,
        effectiveRate: event.effective_rate,
      })),
    ),
    control.on("timeline_changed", (event) => store.setState({ timeline: event.timeline })),
    control.on("paused", () => {
      store.setState({ playing: false });
      void store.getState().refresh();
    }),
    control.on("diverged", (event) => store.setState({ divergence: { tick: event.tick } })),
  ];
  void store.getState().refresh();
  return () => stops.forEach((stop) => stop());
}
