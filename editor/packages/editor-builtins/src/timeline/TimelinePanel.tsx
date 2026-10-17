import type { Editor } from "@ludenea/editor-shell";
import { useEffect, useMemo, useState } from "react";
import { errorText, useGame, useStore } from "../shared";
import { seek, type SpeedState } from "./index";

/** Seeks to the latest position asked for, with one `Seek` in flight at most. */
function createScrubber(editor: Editor, onError: (error: unknown) => void) {
  let inFlight = false;
  let queued: number | null = null;
  const run = (tick: number) => {
    inFlight = true;
    seek(editor, tick)
      .catch(onError)
      .finally(() => {
        inFlight = false;
        const next = queued;
        queued = null;
        if (next !== null) run(next);
      });
  };
  return (tick: number) => {
    if (inFlight) queued = tick;
    else run(tick);
  };
}

const at = (tick: number, head: number) => `${head > 0 ? (tick / head) * 100 : 0}%`;
const mark = (left: string, color: string, height: string) =>
  ({ position: "absolute", left, top: 0, width: 2, height, background: color, pointerEvents: "none" }) as const;

export function TimelinePanel({ editor, speed }: { editor: Editor; speed: SpeedState }) {
  const control = useGame(editor);
  const { timeline, hash, pending, playing, effectiveRate, divergence } = editor.timeline();
  const { speed: current } = useStore(speed);
  const [error, setError] = useState<string | null>(null);
  // The dragged position shows until the seek it asked for lands.
  const [dragged, setDragged] = useState<number | null>(null);
  const scrub = useMemo(() => createScrubber(editor, (failure) => setError(errorText(failure))), [editor]);
  useEffect(() => setDragged(null), [timeline?.current]);

  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  if (!timeline) return <p className="ludenea-muted">Loading the timeline...</p>;
  const run = (command: string) => () =>
    void editor.commands.run(command).then(
      () => setError(null),
      (failure) => setError(errorText(failure)),
    );
  const { head } = timeline;
  const target = control.game.tick_rate * current;

  return (
    <div className="ludenea-timeline">
      <div className="ludenea-toolbar">
        <button onClick={run("timeline.play-pause")}>{playing ? "Pause" : "Play"}</button>
        <button onClick={run("timeline.step-back")} disabled={playing}>
          Step back
        </button>
        <button onClick={run("timeline.step")} disabled={playing}>
          Step
        </button>
        <button onClick={run("timeline.speed-down")}>Slower</button>
        <span>{current}x</span>
        <button onClick={run("timeline.speed-up")}>Faster</button>
        <button onClick={run("edit.undo")} disabled={!timeline.can_undo}>
          Undo
        </button>
        <button onClick={run("edit.redo")} disabled={!timeline.can_redo}>
          Redo
        </button>
      </div>
      <p>
        Tick {timeline.current} of {head}, hash <code aria-label="Hash">{hash}{pending && " (pending)"}</code>
        {/* The measured rate wavers around the target: only a clear shortfall is shown. */}
        {playing && effectiveRate < target * 0.95 && (
          <span className="ludenea-warning">
            {" "}
            running at {effectiveRate.toFixed(1)} ticks per second, below {target}
          </span>
        )}
      </p>
      {timeline.resimulating && (
        <progress aria-label="Resimulation" max={head} value={timeline.simulated}>
          {timeline.simulated} of {head}
        </progress>
      )}
      <div style={{ position: "relative" }}>
        <div style={{ position: "relative", height: 12 }}>
          {timeline.checkpoints.map((tick) => (
            <span key={`checkpoint ${tick}`} title={`Checkpoint at tick ${tick}`} style={{ ...mark(at(tick, head), "currentColor", "50%"), opacity: 0.25 }} />
          ))}
          {timeline.edit_ticks.map((tick) => (
            <span key={`edit ${tick}`} aria-label={`Edit at tick ${tick}`} title={`Edit at tick ${tick}`} style={mark(at(tick, head), "#e0a030", "100%")} />
          ))}
          {divergence && (
            <span aria-label={`Divergence at tick ${divergence.tick}`} title={`Divergence at tick ${divergence.tick}`} style={mark(at(divergence.tick, head), "#e04040", "100%")} />
          )}
        </div>
        <input
          type="range"
          aria-label="Tick"
          min={0}
          max={head}
          value={dragged ?? timeline.current}
          disabled={timeline.resimulating}
          style={{ width: "100%", margin: 0 }}
          onChange={(event) => {
            const tick = Number(event.target.value);
            setDragged(tick);
            scrub(tick);
          }}
        />
      </div>
      {error && <p className="ludenea-error">{error}</p>}
    </div>
  );
}
