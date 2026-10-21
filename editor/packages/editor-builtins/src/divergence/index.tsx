import init, { ghost_frame } from "@ludenea/debug-draw-wasm";
import type { ControlClient, DebugDrawFrame, Difference, DivergenceReport, Entity, Event, FileEntry, Topic, TraceComparison } from "@ludenea/control-client";
import { definePlugin, type Disposable, type Editor } from "@ludenea/editor-shell";
import { useEffect, useState } from "react";
import { createStore, errorText, reportedOnce, shared, showPanel, useGame, useStore, type Store } from "../shared";
import { seek } from "../timeline";
import { css, outline } from "../viewport/overlay";
import { CausalReport } from "./CausalReport";
import { SnapshotDiff } from "./SnapshotDiff";
import { TraceViewer } from "./TraceViewer";

/** What "Extend" adds to the compared range. */
const DIVERGENCE_TICKS = 600;
/** How often a comparison with the session follows it while playing. */
const FOLLOW_PLAYING_MS = 1000;
const LAGGING = "#ffb000";

let loaded: Promise<unknown> | undefined;
/** Loads the module that styles the ghost once, for every use of it. */
const loadGhostStyle = () => (loaded ??= init());

/** A file compared with another file or, while `against` is null, with the current session. */
export type Comparison<R> = { path: string | null; against: string | null; result: R | null; error: string | null };

/** `tick`: the tick a seek from the trace strip reached, which a snapshot still has to be chosen for. */
export type SnapshotComparison = Comparison<Difference[]> & { tick: number | null };

export interface Viewer {
  trace: Store<Comparison<TraceComparison>>;
  snapshot: Store<SnapshotComparison>;
  /** `refresh`: the same comparison again, the previous result shown until the new one. */
  compareTrace(patch?: Partial<Comparison<TraceComparison>>, refresh?: boolean): Promise<void>;
  diffSnapshot(patch?: Partial<SnapshotComparison>, refresh?: boolean): Promise<void>;
  /** Seeks to the traces' first divergent tick, when the current branch is one side, and asks for a snapshot to compare with it. */
  seekDivergence(): Promise<void>;
}

export interface Report {
  state: Store<{ report: DivergenceReport | null; error: string | null; range: number | null }>;
  /** Computes the report until the returned function is called: what an open panel does. */
  watch(): () => void;
  extend(): void;
  /** Seeks to `tick` and selects `entities`. */
  select(tick: number, entities: Entity[]): Promise<void>;
}

const empty = <R,>(): Comparison<R> => ({ path: null, against: null, result: null, error: null });

function game(editor: Editor): ControlClient {
  if (!editor.control) throw new Error("no game is running");
  return editor.control;
}

/** Keeps `topic` subscribed on the current game while `wanted()`, calling `listener` on its event; call the result after any change. */
function follower(editor: Editor, topic: Topic, event: Event["type"], wanted: () => boolean, listener: () => void) {
  let following: { control: ControlClient; stop: () => void } | null = null;
  return () => {
    const control = wanted() ? editor.control : null;
    if (following?.control === control) return;
    if (following) {
      following.stop();
      following.control.request({ type: "unsubscribe", topic }).catch(() => {});
    }
    following = control && { control, stop: control.on(event, listener) };
    control?.request({ type: "subscribe", topic }).catch((error) => editor.console.report(error, `Following the ${topic.type} of the compare branch`));
  };
}

/** Runs `task` at most once at a time; a call while it runs runs it once more after. */
function single(task: () => Promise<void>): () => void {
  let running = false;
  let again = false;
  const run = () => {
    if (running) return void (again = true);
    running = true;
    task().finally(() => {
      running = false;
      if (again) {
        again = false;
        run();
      }
    });
  };
  return run;
}

export const divergencePlugin = definePlugin({
  name: "ludenea.divergence",
  activate(editor) {
    const read = async (path: string) => (await editor.project.request({ type: "read_binary", path })).bytes;
    const comparing = <R, C extends Comparison<R>>(store: Store<C>, compare: (control: ControlClient, a: string, b: string | null) => Promise<R>) => {
      // Only the latest comparison's answer is shown.
      let latest = 0;
      return async (patch: Partial<C> = {}, refresh = false) => {
        if (!refresh) store.set({ ...patch, result: null, error: null } as Partial<C>);
        const { path, against } = store.get();
        const request = ++latest;
        if (!path) return;
        const current = () => request === latest;
        try {
          const result = await compare(game(editor), await read(path), against === null ? null : await read(against));
          if (current()) store.set({ result } as Partial<C>);
        } catch (error) {
          if (current()) store.set({ error: errorText(error) } as Partial<C>);
        }
      };
    };
    const trace = createStore(empty<TraceComparison>());
    const snapshot = createStore<SnapshotComparison>({ ...empty<Difference[]>(), tick: null });
    const viewer: Viewer = {
      trace,
      snapshot,
      compareTrace: comparing(trace, async (control, a, b) => {
        const { type: _, ...comparison } = await control.request({ type: "compare_traces", a, b });
        return comparison;
      }),
      diffSnapshot: comparing(snapshot, async (control, a, b) => (await control.request({ type: "diff_snapshots", a, b })).differences),
      async seekDivergence() {
        const { against, result } = trace.get();
        if (against !== null || result?.first_divergent == null) return;
        await seek(editor, result.first_divergent);
        // The trace holds hashes only: the user picks the snapshot taken at that tick.
        await viewer.diffSnapshot({ path: null, against: null, tick: result.first_divergent });
      },
    };
    // Comparisons with the session follow it: a trace at once while paused and once a second
    // while playing, a snapshot (a whole diff of the world) only while paused, once play stops.
    const live = (store: Store<Comparison<unknown>>) => store.get().path !== null && store.get().against === null;
    const stale = { trace: false, snapshot: false };
    let following: ReturnType<typeof setTimeout> | undefined;
    const followSession = () => {
      if (following !== undefined) return;
      following = setTimeout(
        () => {
          following = undefined;
          if (stale.trace && live(trace)) void viewer.compareTrace({}, true);
          if (stale.snapshot && live(snapshot)) void viewer.diffSnapshot({}, true);
          stale.trace = stale.snapshot = false;
        },
        editor.timeline.getState().playing ? FOLLOW_PLAYING_MS : 0,
      );
    };
    const stopFollowing = editor.timeline.subscribe((next, previous) => {
      stale.trace ||= live(trace) && next.timeline?.simulated !== previous.timeline?.simulated;
      stale.snapshot ||= live(snapshot) && !next.playing && (previous.playing || next.timeline?.current !== previous.timeline?.current);
      if (stale.trace || stale.snapshot) followSession();
    });

    editor.commands.register({
      id: "divergence.open-file",
      title: "Open a trace or snapshot in the divergence viewer",
      async run(_, file) {
        const path = (file as { path?: unknown } | undefined)?.path;
        if (typeof path !== "string") throw new Error("choose a trace or snapshot from the asset browser");
        showPanel(editor, "ludenea.divergence");
        if (path.endsWith(".trace")) await viewer.compareTrace({ path, against: null });
        else if (path.endsWith(".snap")) await viewer.diffSnapshot({ path, against: null, tick: null });
        else throw new Error(`${path} is not a trace or a snapshot`);
      },
    });

    // The causal report: computed only while a panel watches it.
    let watchers = 0;
    const reportState: Report["state"] = createStore({ report: null as DivergenceReport | null, error: null as string | null, range: null as number | null });
    const hasCompare = () => editor.branches.getState().compare !== null;
    const refreshReport = single(async () => {
      const control = editor.control;
      if (!control || watchers === 0) return;
      // The host refuses the report without a compare branch: the panel says so instead.
      if (!hasCompare()) return reportState.set({ report: null, error: null });
      // An answer for another game, or for no watcher left (the column stays empty), is dropped.
      const current = () => editor.control === control && watchers > 0;
      try {
        const { type: _, ...divergence } = await control.request({ type: "get_divergence", up_to: reportState.get().range });
        if (current()) reportState.set({ report: divergence, error: null });
      } catch (error) {
        if (current()) reportState.set({ error: errorText(error) });
      }
    });
    const followReport = follower(editor, { type: "divergence" }, "divergence_progress", () => watchers > 0 && hasCompare(), refreshReport);
    const divergence: Report = {
      state: reportState,
      watch() {
        watchers++;
        followReport();
        refreshReport();
        return () => {
          watchers--;
          followReport();
          // Nothing keeps it current any more: the explorer column empties.
          if (watchers === 0) reportState.set({ report: null, error: null });
        };
      },
      extend() {
        const current = reportState.get().report;
        if (!current) return;
        reportState.set({ range: current.up_to + DIVERGENCE_TICKS });
        refreshReport();
      },
      async select(tick, entities) {
        await seek(editor, tick);
        editor.selection.getState().select(entities);
      },
    };

    // The explorer reads the values again when the report changes.
    const column: Disposable = editor.explorer.columns.register({
      id: "divergence.diverged-at",
      title: "Diverged at",
      width: 80,
      value(entity) {
        const found = reportState.get().report?.entities.find((row) => row.entity.index === entity.index && row.entity.generation === entity.generation);
        return found ? String(found.first_tick) : null;
      },
      subscribe(listener) {
        let shown = reportState.get().report;
        return reportState.subscribe(() => {
          if (reportState.get().report === shown) return;
          shown = reportState.get().report;
          listener();
        });
      },
    });

    // The ghost: the compare branch drawn over the viewport while one is set.
    const ghost = createStore({ visible: true, frame: null as DebugDrawFrame | null, tick: 0, lagging: false });
    const showsGhost = () => ghost.get().visible && hasCompare();
    // Fetched on every tick the ghost advances: a failure is reported once, until a fetch works.
    const fetching = reportedOnce(editor, "Fetching the ghost of the compare branch");
    const fetchGhost = single(async () => {
      const control = editor.control;
      if (!control || !showsGhost()) return;
      try {
        const hidden = shared(editor).layers.get().hidden;
        const layers = control.game.debug_layers.filter((layer) => !hidden.includes(layer));
        const reply = await control.request({ type: "get_ghost_draw", tick: editor.timeline.getState().timeline?.current ?? 0, layers });
        // A host that answers something else (a test double) leaves the ghost as it is.
        if (reply.type !== "ghost_draw" || editor.control !== control) return;
        await loadGhostStyle();
        ghost.set({ frame: JSON.parse(ghost_frame(JSON.stringify(reply.frame))) as DebugDrawFrame, tick: reply.tick, lagging: reply.lagging });
        fetching.worked();
      } catch (error) {
        if (editor.control === control) fetching.failed(error);
      }
    });
    const followGhost = follower(editor, { type: "ghost" }, "ghost_advanced", () => showsGhost(), fetchGhost);
    const updateGhost = () => {
      followGhost();
      if (showsGhost()) fetchGhost();
      else if (ghost.get().frame) ghost.set({ frame: null, lagging: false });
    };
    const stopRepaint = ghost.subscribe(() => shared(editor).overlays.set({ version: shared(editor).overlays.get().version + 1 }));
    editor.commands.register({
      id: "divergence.toggle-ghost",
      title: "Toggle the ghost of the compare branch",
      run() {
        ghost.set({ visible: !ghost.get().visible });
        updateGhost();
      },
    });
    editor.overlays.register({
      id: "divergence.ghost",
      draw({ canvas, worldToScreen }) {
        const { frame, tick, lagging } = ghost.get();
        if (!frame || !showsGhost()) return;
        const scale = Math.abs(worldToScreen({ x: 1, y: 0 }).x - worldToScreen({ x: 0, y: 0 }).x);
        canvas.save();
        canvas.lineWidth = 1.5;
        for (const { primitive } of Object.values(frame.layers).flat()) {
          if (primitive.type === "label") {
            const at = worldToScreen(primitive.at);
            canvas.fillStyle = css(primitive.color);
            canvas.fillText(primitive.text, at.x, at.y);
            continue;
          }
          canvas.strokeStyle = css(primitive.color);
          canvas.beginPath();
          outline(canvas, primitive, worldToScreen, scale);
          canvas.stroke();
        }
        if (lagging) {
          canvas.fillStyle = LAGGING;
          canvas.fillText(`Ghost lagging at tick ${tick}`, 8, 16);
        }
        canvas.restore();
      },
    });

    const stops = [
      editor.branches.subscribe((next, previous) => {
        if (next.compare === previous.compare && next.current === previous.current) return;
        reportState.set({ report: null, error: null, range: null });
        followReport();
        refreshReport();
        updateGhost();
      }),
      editor.timeline.subscribe((next, previous) => {
        if (next.timeline?.current !== previous.timeline?.current) fetchGhost();
      }),
      stopRepaint,
      stopFollowing,
      () => clearTimeout(following),
      shared(editor).layers.subscribe(fetchGhost),
    ];
    const changed = editor.onGameChanged(() => {
      reportState.set({ report: null, error: null, range: null });
      ghost.set({ frame: null, lagging: false });
      followReport();
      refreshReport();
      updateGhost();
    });

    editor.panels.register({
      id: "ludenea.divergence",
      title: "Divergence",
      defaultPosition: "bottom",
      component: ({ editor }) => <DivergencePanel editor={editor} viewer={viewer} />,
    });
    editor.panels.register({
      id: "ludenea.divergence-report",
      title: "Divergence report",
      defaultPosition: "right",
      component: ({ editor }) => <CausalReport editor={editor} report={divergence} />,
    });
    return () => {
      stops.forEach((stop) => stop());
      changed.dispose();
      column.dispose();
      watchers = 0;
      ghost.set({ visible: false });
      followReport();
      followGhost();
    };
  },
});

function DivergencePanel({ editor, viewer }: { editor: Editor; viewer: Viewer }) {
  const control = useGame(editor);
  const trace = useStore(viewer.trace).path;
  const { path: snapshot, tick } = useStore(viewer.snapshot);
  const [files, setFiles] = useState<FileEntry[]>([]);
  useEffect(() => {
    editor.project.request({ type: "list_files" }).then(
      ({ entries }) => setFiles(entries),
      () => setFiles([]),
    );
  }, [editor, trace, snapshot, tick]);
  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  if (!trace && !snapshot && tick === null) return <p className="ludenea-muted">Open a trace or a snapshot from the asset browser.</p>;
  const of = (kind: string) => files.filter((file) => file.kind === kind).map((file) => file.path);
  return (
    <div className="ludenea-divergence">
      {trace && <TraceViewer editor={editor} viewer={viewer} traces={of("Trace")} />}
      {(snapshot || tick !== null) && <SnapshotDiff viewer={viewer} snapshots={of("Snapshot")} />}
    </div>
  );
}
