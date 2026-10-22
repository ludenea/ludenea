import type { ControlClient } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";

/** What a frame time measurement recorded. */
export interface FrameTimes {
  /** The time between consecutive animation frames while the game played, in milliseconds. */
  frames: number[];
  /** How many `render_frames` events the game pushed meanwhile. */
  pushed: number;
  /** The errors the editor console reported from the viewport's opening to the pause (a renderer that cannot draw reports there). */
  errors: string[];
}

type DocumentState = ReturnType<Editor["document"]["getState"]>;

/** What the measurement uses of the editor. */
type Measured = {
  control: ControlClient | null;
  document: { getState(): Pick<DocumentState, "loaded" | "error"> };
  commands: Pick<Editor["commands"], "run">;
  console: Pick<Editor["console"], "list">;
};

/** How often the measurement checks whether the game and its scene are ready. */
const POLL_MS = 100;

/** The game once it has loaded the open scene; rejects when the scene cannot load. */
async function loadedGame(editor: Measured): Promise<ControlClient> {
  for (;;) {
    const { loaded, error } = editor.document.getState();
    if (error) throw new Error(`the scene cannot load: ${error}`);
    if (editor.control && loaded) return editor.control;
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }
}

/** How long without an animation frame ends a measurement: the window is not on screen. */
const STALL_MS = 5000;

/** Records the time between animation frames for `playMs`; rejects after `STALL_MS` without one. */
function recordFrames(playMs: number): Promise<number[]> {
  return new Promise((resolve, reject) => {
    const frames: number[] = [];
    let start: number | undefined;
    let last = 0;
    let stalled: ReturnType<typeof setTimeout>;
    const watch = () => {
      clearTimeout(stalled);
      stalled = setTimeout(
        () => reject(new Error(`no animation frame for ${STALL_MS / 1000} seconds: the editor window must stay on screen (not minimized, hidden, or behind a locked screen)`)),
        STALL_MS,
      );
    };
    const frame = (now: number) => {
      watch();
      if (start === undefined) start = now;
      else frames.push(now - last);
      last = now;
      if (now - start < playMs) return void requestAnimationFrame(frame);
      clearTimeout(stalled);
      resolve(frames);
    };
    watch();
    requestAnimationFrame(frame);
  });
}

/**
 * Measures how smoothly a freshly started editor plays its open scene: once the game has loaded
 * it, shows the viewport through the game camera, plays for `playMs` while recording the time
 * between animation frames and the render frames the game pushes, then pauses. The errors the
 * editor console reports meanwhile are in the report: the viewport reports there a frame its
 * renderer cannot read and a renderer that cannot start.
 */
export async function measureFrameTimes(editor: Measured, playMs: number): Promise<FrameTimes> {
  const control = await loadedGame(editor);
  const before = editor.console.list().at(-1)?.id ?? 0;
  const errors = () => editor.console.list().filter((entry) => entry.id > before && entry.level === "error").map((entry) => entry.text);
  await editor.commands.run("panels.open.ludenea.viewport");
  await editor.commands.run("viewport.toggle-game-camera");
  let pushed = 0;
  const stop = control.on("render_frames", () => pushed++);
  await editor.commands.run("timeline.play-pause");
  try {
    const frames = await recordFrames(playMs);
    return { frames, pushed, errors: errors() };
  } finally {
    stop();
    // A failing pause must not replace the error that ended the measurement.
    await editor.commands.run("timeline.play-pause").catch((error: unknown) => console.warn(`cannot pause after the frame time measurement: ${error instanceof Error ? error.message : String(error)}`));
  }
}

/** The 95th percentile of the frame times a measurement must stay at or under, in milliseconds. */
export const FRAME_TIME_BUDGET_MS = 20;

/** The value at `fraction` of `sorted`, as the end-to-end performance tests compute it. */
const percentile = (sorted: number[], fraction: number) => sorted[Math.min(sorted.length - 1, Math.floor(fraction * sorted.length))]!;

/**
 * The verdict on a measurement's report, a `FrameTimes` or the `{ error }` that ended it: the
 * line to print, starting with `subject`, and whether it fails. It fails on an error, without
 * frames drawn or render frames pushed, when the editor reported errors (the viewport did not
 * draw what the game pushed), and when the 95th percentile is over the budget.
 */
export function frameTimesVerdict(report: FrameTimes | { error: string }, subject: string): { line: string; failed: boolean } {
  if ("error" in report) return { line: `${subject}: the measurement failed: ${report.error}`, failed: true };
  const sorted = [...report.frames].sort((a, b) => a - b);
  if (sorted.length === 0) return { line: `${subject}: no frame was drawn while the game played`, failed: true };
  const [p50, p95, p99] = [0.5, 0.95, 0.99].map((fraction) => percentile(sorted, fraction));
  const line = `${subject}: ${sorted.length} frames, p50 ${p50!.toFixed(2)} ms, p95 ${p95!.toFixed(2)} ms, p99 ${p99!.toFixed(2)} ms; ${report.pushed} render frame events pushed`;
  if (report.pushed === 0) return { line: `${line}; the game pushed no render frames`, failed: true };
  if (report.errors.length > 0) return { line: `${line}; the editor reported errors: ${report.errors.join("; ")}`, failed: true };
  if (p95! > FRAME_TIME_BUDGET_MS) return { line: `${line}; the 95th percentile is over ${FRAME_TIME_BUDGET_MS} ms`, failed: true };
  return { line, failed: false };
}
