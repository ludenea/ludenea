import type { ControlClient } from "@ludenea/control-client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { frameTimesVerdict, measureFrameTimes } from "../src/frame-times";

/** An editor whose game and scene arrive when the test says, recording the commands run. */
function fakeEditor() {
  const listeners = new Set<() => void>();
  const game = { on: (_type: string, listener: () => void) => (listeners.add(listener), () => listeners.delete(listener)) } as unknown as ControlClient;
  const state = { loaded: null as { type: "loaded"; warnings: string[]; instances: []; load_addresses: [] } | null, error: null as string | null };
  const commands: string[] = [];
  const entries: { id: number; level: "info" | "warning" | "error"; text: string }[] = [];
  const editor = {
    control: null as ControlClient | null,
    document: { getState: () => state },
    commands: {
      run: async (id: string) => {
        commands.push(id);
      },
    },
    console: { list: () => entries },
  };
  return {
    editor,
    commands,
    state,
    log: (level: "info" | "warning" | "error", text: string) => entries.push({ id: entries.length + 1, level, text }),
    connect: () => (editor.control = game),
    push: () => listeners.forEach((listener) => listener()),
    listening: () => listeners.size,
  };
}

/** Animation frames at the timestamps of `times`, then every 1000 ms. */
function animationFrames(times: number[], onFrame: () => void = () => {}) {
  const queue = [...times];
  let last = 0;
  vi.stubGlobal("requestAnimationFrame", (callback: (now: number) => void) =>
    setTimeout(() => {
      last = queue.shift() ?? last + 1000;
      onFrame();
      callback(last);
    }, 0),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("frame times", () => {
  it("measuring_waits_for_the_scene_then_plays_through_the_game_camera", async () => {
    const fake = fakeEditor();
    // Two frames pushed during play.
    let frame = 0;
    animationFrames([1000, 1008, 1033, 1041, 1100], () => {
      if (++frame === 3 || frame === 4) fake.push();
    });
    const measuring = measureFrameTimes(fake.editor, 100);
    await new Promise((resolve) => setTimeout(resolve, 150));
    expect(fake.commands).toEqual([]);
    fake.connect();
    fake.state.loaded = { type: "loaded", warnings: [], instances: [], load_addresses: [] };
    const times = await measuring;
    expect(fake.commands).toEqual(["panels.open.ludenea.viewport", "viewport.toggle-game-camera", "timeline.play-pause", "timeline.play-pause"]);
    // From the first frame: each frame's time since the one before, until 100 ms have passed.
    expect(times).toEqual({ frames: [8, 25, 8, 59], pushed: 2, errors: [] });
    expect(fake.listening()).toBe(0);
  });

  it("a_window_that_draws_no_frame_fails_the_measurement_and_pauses", async () => {
    vi.useFakeTimers();
    try {
      const fake = fakeEditor();
      fake.connect();
      fake.state.loaded = { type: "loaded", warnings: [], instances: [], load_addresses: [] };
      // A hidden window, or one behind a locked screen: no animation frame ever comes.
      vi.stubGlobal("requestAnimationFrame", () => 0);
      const measuring = expect(measureFrameTimes(fake.editor, 100)).rejects.toThrow(
        "no animation frame for 5 seconds: the editor window must stay on screen (not minimized, hidden, or behind a locked screen)",
      );
      await vi.advanceTimersByTimeAsync(5000);
      await measuring;
      expect(fake.commands.slice(-2)).toEqual(["timeline.play-pause", "timeline.play-pause"]);
      expect(fake.listening()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("errors_the_editor_reports_once_the_viewport_opens_are_in_the_report", async () => {
    const fake = fakeEditor();
    fake.connect();
    fake.state.loaded = { type: "loaded", warnings: [], instances: [], load_addresses: [] };
    fake.log("error", "Building the game failed: an earlier build");
    let frame = 0;
    animationFrames([1000, 1010, 1100], () => {
      if (++frame === 2) {
        fake.push();
        // As the viewport reports a frame its renderer cannot read (a stale renderer build).
        fake.log("error", "Showing the render frame failed: unexpected end of input");
        fake.log("warning", "1 sprites or tiles have no texture");
      }
    });
    const times = await measureFrameTimes(fake.editor, 100);
    expect(times.errors).toEqual(["Showing the render frame failed: unexpected end of input"]);
    expect(frameTimesVerdict(times, "swarm")).toEqual({
      line: "swarm: 2 frames, p50 90.00 ms, p95 90.00 ms, p99 90.00 ms; 1 render frame events pushed; the editor reported errors: Showing the render frame failed: unexpected end of input",
      failed: true,
    });
  });

  it("a_scene_that_cannot_load_fails_the_measurement", async () => {
    const fake = fakeEditor();
    fake.connect();
    fake.state.error = "the scene does not parse";
    await expect(measureFrameTimes(fake.editor, 100)).rejects.toThrow("the scene cannot load: the scene does not parse");
    expect(fake.commands).toEqual([]);
  });

  it("the_verdict_fails_over_twenty_milliseconds_at_the_95th_percentile", () => {
    // 100 frames: 95 at 10 ms, then the slowest five.
    const frames = [...Array<number>(95).fill(10), 20, 21, 30, 40, 50];
    expect(frameTimesVerdict({ frames, pushed: 600, errors: [] }, "swarm")).toEqual({ line: "swarm: 100 frames, p50 10.00 ms, p95 20.00 ms, p99 50.00 ms; 600 render frame events pushed", failed: false });
    frames[95] = 20.5;
    expect(frameTimesVerdict({ frames, pushed: 600, errors: [] }, "swarm")).toEqual({
      line: "swarm: 100 frames, p50 10.00 ms, p95 20.50 ms, p99 50.00 ms; 600 render frame events pushed; the 95th percentile is over 20 ms",
      failed: true,
    });
  });

  it("the_verdict_fails_without_frames_pushed_or_drawn_and_on_an_error", () => {
    expect(frameTimesVerdict({ frames: [8], pushed: 0, errors: [] }, "swarm")).toEqual({ line: "swarm: 1 frames, p50 8.00 ms, p95 8.00 ms, p99 8.00 ms; 0 render frame events pushed; the game pushed no render frames", failed: true });
    expect(frameTimesVerdict({ frames: [], pushed: 3, errors: [] }, "swarm")).toEqual({ line: "swarm: no frame was drawn while the game played", failed: true });
    expect(frameTimesVerdict({ error: "the scene cannot load: boom" }, "swarm")).toEqual({ line: "swarm: the measurement failed: the scene cannot load: boom", failed: true });
  });
});
