import init, { ViewportRenderer } from "@ludenea/debug-draw-wasm";
import type { Entity, Vec2 } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useEffect, useRef, useState } from "react";
import { captureInput } from "../input/capture";
import { errorText, useGame } from "../shared";
import { ResimulatingNotice } from "../timeline/editing";
import { panBy, screenToWorld, zoomAt, type Size } from "./camera";
import { moveBy, visibleLayers, type ViewportState } from "./state";
import { drawOverlay } from "./overlay";

/** Pointer travel, in CSS pixels, past which a press is a drag rather than a click. */
const DRAG_THRESHOLD = 3;
const WHEEL_ZOOM = 0.0015;
const ARROWS: { [key: string]: string } = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" };

let loaded: Promise<unknown> | undefined;
async function createRenderer(canvas: HTMLCanvasElement): Promise<ViewportRenderer> {
  await (loaded ??= init());
  return ViewportRenderer.create(canvas);
}

type Point = { x: number; y: number };
type Press = {
  mode: "pan" | "select";
  start: Point;
  last: Point;
  moved: boolean;
  shift: boolean;
  picked?: Promise<Entity[]>;
  /** The selected entity under the press, once the pick answers: what a drag moves. */
  target?: Entity | null;
};

const same = (a: Entity, b: Entity) => a.index === b.index && a.generation === b.generation;
const report = (what: string) => (error: unknown) => console.error(`viewport: ${what} failed:`, error);

export function ViewportPanel({ editor, state }: { editor: Editor; state: ViewportState }) {
  const game = useGame(editor);
  const root = useRef<HTMLDivElement>(null);
  const gpu = useRef<HTMLCanvasElement>(null);
  const overlay = useRef<HTMLCanvasElement>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    const element = root.current!;
    const context = overlay.current!.getContext("2d");
    let renderer: ViewportRenderer | null = null;
    let rendered: Size = { width: 0, height: 0 };
    let disposed = false;
    // `dirty`: a new frame is needed; `repaint`: the current one must be drawn again.
    let dirty = true;
    let repaint = true;
    let inFlight = false;
    let press: Press | null = null;
    let space = false;
    let seen = state.get();

    const measure = () => {
      const size = { width: element.clientWidth, height: element.clientHeight };
      const { width, height } = state.get().size;
      if (size.width !== width || size.height !== height) state.set({ size });
    };
    measure();
    const resizes = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    resizes?.observe(element);

    createRenderer(gpu.current!).then(
      (created) => {
        if (disposed) return created.free();
        renderer = created;
        repaint = true;
      },
      (error) => setFailure(`The viewport cannot render: ${errorText(error)}`),
    );

    const fetchFrame = () => {
      const control = editor.control;
      if (!control || inFlight) return;
      inFlight = true;
      dirty = false;
      control
        .request({ type: "get_debug_draw", layers: visibleLayers(control, state) })
        .then(({ tick, frame }) => {
          if (!disposed && editor.control === control) state.set({ frame, tick });
        }, report("fetching the debug draw"))
        .finally(() => (inFlight = false));
    };

    const paint = () => {
      repaint = false;
      const { camera, size, frame, tick } = state.get();
      const ratio = window.devicePixelRatio || 1;
      const layers = visibleLayers(editor.control, state);
      const selection = editor.selection.getState().entities;
      if (renderer) {
        try {
          if (size.width !== rendered.width || size.height !== rendered.height) {
            renderer.resize(size.width, size.height, ratio);
            rendered = size;
          }
          renderer.set_camera(camera.center.x, camera.center.y, camera.pixelsPerMeter);
          renderer.draw(JSON.stringify(frame ?? { layers: {} }), JSON.stringify(layers), JSON.stringify(selection));
        } catch (error) {
          report("drawing")(error);
        }
      }
      if (!context) return;
      const canvas = context.canvas;
      if (canvas.width !== Math.round(size.width * ratio)) canvas.width = Math.round(size.width * ratio);
      if (canvas.height !== Math.round(size.height * ratio)) canvas.height = Math.round(size.height * ratio);
      const drag = press?.mode === "select" && press.moved && press.target ? { entity: press.target, offset: dragOffset(press) } : null;
      drawOverlay(context, { camera, size, frame, layers, tick, selection, overlays: editor.overlays.list(), drag }, ratio);
    };

    // One request in flight at most: every animation frame while playing, on change otherwise.
    let animation = requestAnimationFrame(function loop() {
      if (editor.timeline.getState().playing || dirty) fetchFrame();
      if (repaint) paint();
      animation = requestAnimationFrame(loop);
    });

    const stops = [
      state.subscribe(() => {
        const next = state.get();
        if (next.hidden !== seen.hidden || next.revision !== seen.revision) dirty = true;
        seen = next;
        repaint = true;
      }),
      editor.timeline.subscribe(() => (dirty = true)),
      editor.selection.subscribe(() => (repaint = true)),
      subscribeGame(editor, () => {
        state.set({ frame: null, tick: 0 });
        dirty = true;
      }),
    ];

    const local = (event: MouseEvent): Point => {
      const rect = element.getBoundingClientRect();
      return { x: event.clientX - rect.left, y: event.clientY - rect.top };
    };
    const world = (point: Point) => screenToWorld(state.get().camera, state.get().size, point);
    const dragOffset = ({ start, last }: Press): Vec2 => {
      const [from, to] = [world(start), world(last)];
      return { x: to.x - from.x, y: to.y - from.y };
    };

    const onPointerDown = (event: PointerEvent) => {
      element.focus();
      const at = local(event);
      const control = editor.control;
      if (event.button === 1 || (event.button === 0 && space)) {
        press = { mode: "pan", start: at, last: at, moved: false, shift: false };
      } else if (event.button === 0 && control) {
        const picked = control.request({ type: "pick", point: world(at) }).then(({ entities }) => entities);
        const current: Press = { mode: "select", start: at, last: at, moved: false, shift: event.shiftKey, picked };
        picked.then((entities) => {
          const selection = editor.selection.getState().entities;
          current.target = entities.find((entity) => selection.some((selected) => same(selected, entity))) ?? null;
          repaint = true;
        }, report("picking"));
        press = current;
      } else return;
      event.preventDefault();
      element.setPointerCapture?.(event.pointerId);
    };

    const onPointerMove = (event: PointerEvent) => {
      if (!press) return;
      const at = local(event);
      if (Math.hypot(at.x - press.start.x, at.y - press.start.y) > DRAG_THRESHOLD) press.moved = true;
      if (press.mode === "pan") {
        const { camera } = state.get();
        state.set({ camera: panBy(camera, at.x - press.last.x, at.y - press.last.y) });
      }
      press.last = at;
      repaint = true;
    };

    const onPointerUp = (event: PointerEvent) => {
      const released = press;
      press = null;
      repaint = true;
      if (released?.mode !== "select" || !released.picked) return;
      released.last = local(event);
      const control = editor.control;
      const { selection } = editor;
      if (!released.moved) {
        released.picked.then((entities) => {
          const hit = entities[0];
          if (!released.shift) selection.getState().select(hit ? [hit] : []);
          else if (hit) selection.getState().add(hit);
        }, report("picking"));
        return;
      }
      // A drag moves the selected entity it started on, as one edit on release.
      const offset = dragOffset(released);
      released.picked
        .then(() => {
          if (released.target && control) return moveBy(editor, control, released.target, offset, state);
        })
        .catch(report("moving"));
    };

    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const { camera, size } = state.get();
      state.set({ camera: zoomAt(camera, size, local(event), Math.exp(-event.deltaY * WHEEL_ZOOM)) });
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === " ") {
        space = true;
        event.preventDefault();
        return;
      }
      // While playing, plain keys are the game's.
      const playing = editor.timeline.getState().playing;
      if (event.key === "Delete" && !playing) {
        event.preventDefault();
        editor.commands.run("scene.despawn-selection").catch(report("despawning"));
        return;
      }
      const direction = ARROWS[event.key];
      if (!direction || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
      if (playing) return;
      event.preventDefault();
      editor.commands.run(`viewport.nudge-${direction}`).catch(report("nudging"));
    };
    const onKeyUp = (event: KeyboardEvent) => {
      if (event.key === " ") space = false;
    };
    const onBlur = () => (space = false);

    const listeners: [string, EventListener][] = [
      ["pointerdown", onPointerDown as EventListener],
      ["pointermove", onPointerMove as EventListener],
      ["pointerup", onPointerUp as EventListener],
      ["keydown", onKeyDown as EventListener],
      ["keyup", onKeyUp as EventListener],
      ["blur", onBlur],
    ];
    for (const [type, listener] of listeners) element.addEventListener(type, listener);
    element.addEventListener("wheel", onWheel, { passive: false });
    const stopCapture = captureInput(editor, element);

    return () => {
      disposed = true;
      cancelAnimationFrame(animation);
      stopCapture();
      resizes?.disconnect();
      stops.forEach((stop) => stop());
      for (const [type, listener] of listeners) element.removeEventListener(type, listener);
      element.removeEventListener("wheel", onWheel);
      renderer?.free();
    };
  }, [editor, state]);

  const fill = { position: "absolute", inset: 0, width: "100%", height: "100%" } as const;
  return (
    <div ref={root} role="application" aria-label="Viewport" tabIndex={0} style={{ position: "relative", width: "100%", height: "100%", overflow: "hidden", outline: "none" }}>
      <canvas ref={gpu} style={fill} />
      <canvas ref={overlay} style={{ ...fill, pointerEvents: "none" }} />
      {(failure || !game) && (
        <p className="ludenea-muted" style={{ position: "absolute", margin: 8 }}>{failure ?? "No game is running."}</p>
      )}
      <div style={{ position: "absolute", bottom: 0, margin: 8 }}>
        <ResimulatingNotice editor={editor} />
      </div>
    </div>
  );
}

function subscribeGame(editor: Editor, listener: () => void): () => void {
  const subscription = editor.onGameChanged(listener);
  return () => subscription.dispose();
}
