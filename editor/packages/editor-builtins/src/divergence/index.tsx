import { ghost_frame } from "@ludenea/debug-draw-wasm";
import type { ControlClient, DebugDrawFrame, Difference, DivergenceReport, Entity, Event, FileEntry, Topic, TraceComparison } from "@ludenea/control-client";
import { definePlugin, type Disposable, type Editor } from "@ludenea/editor-shell";
import { useEffect, useState } from "react";
import { createStore, errorText, shared, showPanel, useGame, useStore, type Store } from "../shared";
import { seek } from "../timeline";
import { css, outline } from "../viewport/overlay";
import { loadRenderer } from "../viewport/ViewportPanel";
import { CausalReport } from "./CausalReport";
import { SnapshotDiff } from "./SnapshotDiff";
import { TraceViewer } from "./TraceViewer";

/** The host's report range after the fork tick, and what "Extend" adds to it. */
const DIVERGENCE_TICKS = 600;
const LAGGING = "#ffb000";

/** A file compared with another file or, while `against` is null, with the current session. */
export type Comparison<R> = { path: string | null; against: string | null; result: R | null; error: string | null };

export interface Viewer {
  trace: Store<Comparison<TraceComparison>>;
  snapshot: Store<Comparison<Difference[]>>;
  compareTrace(patch?: Partial<Comparison<TraceComparison>>): Promise<void>;
  diffSnapshot(patch?: Partial<Comparison<Difference[]>>): Promise<void>;
  /** Seeks to the traces' first divergent tick, when the current branch is one side, and compares the snapshot with it. */
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
const report = (what: string) => (error: unknown) => console.error(`divergence: ${what} failed:`, error);

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
    control?.request({ type: "subscribe", topic }).catch(report(`subscribing to ${topic.type}`));
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
    const comparing =
      <R,>(store: Store<Comparison<R>>, compare: (control: ControlClient, a: string, b: string | null) => Promise<R>) =>
      async (patch: Partial<Comparison<R>> = {}) => {
        store.set({ ...patch, result: null, error: null });
        const { path, against } = store.get();
        if (!path) return;
        const current = () => store.get().path === path && store.get().against === against;
        try {
          const result = await compare(game(editor), await read(path), against === null ? null : await read(against));
          if (current()) store.set({ result });
        } catch (error) {
          if (current()) store.set({ error: errorText(error) });
        }
      };
    const viewer: Viewer = {
      trace: createStore(empty<TraceComparison>()),
      snapshot: createStore(empty<Difference[]>()),
      compareTrace: (patch) =>
        comparing(viewer.trace, async (control, a, b) => {
          const { type: _, ...comparison } = await control.request({ type: "compare_traces", a, b });
          return comparison;
        })(patch),
      diffSnapshot: (patch) => comparing(viewer.snapshot, async (control, a, b) => (await control.request({ type: "diff_snapshots", a, b })).differences)(patch),
      async seekDivergence() {
        const { against, result } = viewer.trace.get();
        if (against !== null || result?.first_divergent == null) return;
        await seek(editor, result.first_divergent);
        await viewer.diffSnapshot({ against: null });
      },
    };
    editor.commands.register({
      id: "divergence.open-file",
      title: "Open a trace or snapshot in the divergence viewer",
      async run(_, file) {
        const path = (file as { path?: unknown } | undefined)?.path;
        if (typeof path !== "string") throw new Error("choose a trace or snapshot from the asset browser");
        showPanel(editor, "ludenea.divergence");
        if (path.endsWith(".trace")) await viewer.compareTrace({ path, against: null });
        else if (path.endsWith(".snap")) await viewer.diffSnapshot({ path, against: null });
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
      try {
        const { type: _, ...divergence } = await control.request({ type: "get_divergence", up_to: reportState.get().range });
        if (editor.control === control) reportState.set({ report: divergence, error: null });
      } catch (error) {
        if (editor.control === control) reportState.set({ error: errorText(error) });
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
        };
      },
      extend() {
        const { report: current, range } = reportState.get();
        if (!current) return;
        reportState.set({ range: (range ?? current.fork_tick + DIVERGENCE_TICKS) + DIVERGENCE_TICKS });
        refreshReport();
      },
      async select(tick, entities) {
        await seek(editor, tick);
        editor.selection.getState().select(entities);
      },
    };

    // Registered again when the report changes, so the explorer shows the new values.
    const registerColumn = () =>
      editor.explorer.columns.register({
        id: "divergence.diverged-at",
        title: "Diverged at",
        width: 80,
        value(entity) {
          const found = reportState.get().report?.entities.find((row) => row.entity.index === entity.index && row.entity.generation === entity.generation);
          return found ? String(found.first_tick) : null;
        },
      });
    let column: Disposable = registerColumn();
    let shown = reportState.get().report;
    const stopColumn = reportState.subscribe(() => {
      if (reportState.get().report === shown) return;
      shown = reportState.get().report;
      column.dispose();
      column = registerColumn();
    });

    // The ghost: the compare branch drawn over the viewport while one is set.
    const ghost = createStore({ visible: true, frame: null as DebugDrawFrame | null, tick: 0, lagging: false });
    const showsGhost = () => ghost.get().visible && hasCompare();
    const fetchGhost = single(async () => {
      const control = editor.control;
      if (!control || !showsGhost()) return;
      try {
        const reply = await control.request({ type: "get_ghost_draw", tick: editor.timeline.getState().timeline?.current ?? 0, layers: control.game.debug_layers });
        // A host that answers something else (a test double) leaves the ghost as it is.
        if (reply.type !== "ghost_draw" || editor.control !== control) return;
        await loadRenderer();
        ghost.set({ frame: JSON.parse(ghost_frame(JSON.stringify(reply.frame))) as DebugDrawFrame, tick: reply.tick, lagging: reply.lagging });
      } catch (error) {
        if (editor.control === control) report("fetching the ghost")(error);
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
      stopColumn,
      stopRepaint,
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
  const snapshot = useStore(viewer.snapshot).path;
  const [files, setFiles] = useState<FileEntry[]>([]);
  useEffect(() => {
    editor.project.request({ type: "list_files" }).then(
      ({ entries }) => setFiles(entries),
      () => setFiles([]),
    );
  }, [editor, trace, snapshot]);
  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  if (!trace && !snapshot) return <p className="ludenea-muted">Open a trace or a snapshot from the asset browser.</p>;
  const of = (kind: string) => files.filter((file) => file.kind === kind).map((file) => file.path);
  return (
    <div className="ludenea-divergence">
      {trace && <TraceViewer viewer={viewer} traces={of("Trace")} />}
      {snapshot && <SnapshotDiff viewer={viewer} snapshots={of("Snapshot")} />}
    </div>
  );
}
