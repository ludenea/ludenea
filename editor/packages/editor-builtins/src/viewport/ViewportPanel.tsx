import init, { ViewportRenderer } from "@ludenea/debug-draw-wasm";
import type { Entity, Vec2 } from "@ludenea/control-client";
import { runReported, type Editor, type ViewportOverlay } from "@ludenea/editor-shell";
import { useEffect, useRef, useState } from "react";
import { captureInput } from "../input/capture";
import { errorText, reportedOnce, shared, useGame } from "../shared";
import { ResimulatingNotice } from "../timeline/editing";
import { panBy, screenToWorld, zoomAt, type Size } from "./camera";
import { moveBy, visibleLayers, type ViewportState } from "./state";
import { drawOverlay } from "./overlay";

/** Pointer travel, in CSS pixels, past which a press is a drag rather than a click. */
const DRAG_THRESHOLD = 3;
const WHEEL_ZOOM = 0.0015;
const ARROWS: { [key: string]: string } = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" };

let loaded: Promise<unknown> | undefined;
/** Loads the renderer's WebAssembly module once, for every use of it. */
export const loadRenderer = () => (loaded ??= init());

async function createRenderer(canvas: HTMLCanvasElement): Promise<ViewportRenderer> {
  await loadRenderer();
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
  /** The selected entity under the press, once the pick answers: a drag moves the selection holding it. */
  target?: Entity | null;
};

const same = (a: Entity, b: Entity) => a.index === b.index && a.generation === b.generation;

/** `failedOverlays`: the plugin overlays whose failure is reported already, kept across mounts: once per registration. */
export function ViewportPanel({ editor, state, failedOverlays }: { editor: Editor; state: ViewportState; failedOverlays: Set<ViewportOverlay> }) {
  const game = useGame(editor);
  const root = useRef<HTMLDivElement>(null);
  const gpu = useRef<HTMLCanvasElement>(null);
  const overlay = useRef<HTMLCanvasElement>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    const element = root.current!;
    const context = overlay.current!.getContext("2d");
    let renderer: ViewportRenderer | null = null;
    let rendered: Size & { ratio: number } = { width: 0, height: 0, ratio: 0 };
    let ratio = window.devicePixelRatio || 1;
    let disposed = false;
    // `dirty`: a new frame is needed; `repaint`: the current one must be drawn again.
    let dirty = true;
    let repaint = true;
    let inFlight = false;
    let press: Press | null = null;
    let space = false;
    let seen = state.get();
    const report = (context: string) => (error: unknown) => editor.console.report(error, context);
    const fetching = reportedOnce(editor, "Fetching the debug draw");
    const drawing = reportedOnce(editor, "Drawing the viewport");
    const overlayFailed = (overlay: ViewportOverlay, error: unknown) => {
      if (failedOverlays.has(overlay)) return;
      failedOverlays.add(overlay);
      editor.console.report(error, `Drawing the viewport overlay "${overlay.id}"`);
    };

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
        .then(
          ({ tick, frame }) => {
            fetching.worked();
            if (!disposed && editor.control === control) state.set({ frame, tick });
          },
          // A game being replaced refuses its last request: only the current game's failures count.
          (error) => {
            if (!disposed && editor.control === control) fetching.failed(error);
          },
        )
        .finally(() => (inFlight = false));
    };

    const paint = () => {
      repaint = false;
      const { camera, size, frame, tick } = state.get();
      const layers = visibleLayers(editor.control, state);
      const selection = editor.selection.getState().entities;
      if (renderer) {
        try {
          if (size.width !== rendered.width || size.height !== rendered.height || ratio !== rendered.ratio) {
            renderer.resize(size.width, size.height, ratio);
            rendered = { ...size, ratio };
          }
          renderer.set_camera(camera.center.x, camera.center.y, camera.pixelsPerMeter);
          // No surface texture this time: draw again on the next animation frame.
          if (!renderer.draw(JSON.stringify(frame ?? { layers: {} }), JSON.stringify(layers), JSON.stringify(selection))) repaint = true;
          drawing.worked();
        } catch (error) {
          drawing.failed(error);
        }
      }
      if (!context) return;
      const canvas = context.canvas;
      if (canvas.width !== Math.round(size.width * ratio)) canvas.width = Math.round(size.width * ratio);
      if (canvas.height !== Math.round(size.height * ratio)) canvas.height = Math.round(size.height * ratio);
      const drag = press?.mode === "select" && press.moved && press.target ? { entities: dragged(press.target), offset: dragOffset(press) } : null;
      const overlays = editor.overlays.list();
      // One unregistered then registered again is a new registration: its failure is news.
      for (const overlay of failedOverlays) if (!overlays.includes(overlay)) failedOverlays.delete(overlay);
      drawOverlay(context, { camera, size, frame, layers, tick, selection, overlays, drag }, ratio, overlayFailed);
    };

    // One request in flight at most: every animation frame while playing, on change otherwise.
    let animation = requestAnimationFrame(function loop() {
      // Moving the window to another screen or zooming the page changes it, with no event here.
      if ((window.devicePixelRatio || 1) !== ratio) {
        ratio = window.devicePixelRatio || 1;
        repaint = true;
      }
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
      shared(editor).overlays.subscribe(() => (repaint = true)),
      subscribeGame(editor, () => {
        // Another game: its failures are news.
        fetching.worked();
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
    // What a drag starting on `target` moves: the selection holding it, or it alone.
    const dragged = (target: Entity): readonly Entity[] => {
      const { entities } = editor.selection.getState();
      return entities.some((entity) => same(entity, target)) ? entities : [target];
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
          const selection = editor.selection.getState();
          const target = entities.find((entity) => selection.entities.some((selected) => same(selected, entity)));
          // Pressing an unselected entity selects it, so a drag can start right away.
          if (!target && entities[0] && !current.shift) selection.select([entities[0]]);
          current.target = target ?? (current.shift ? null : (entities[0] ?? null));
          repaint = true;
        }, report("Picking an entity in the viewport"));
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
      // A failed pick was reported on press: the release reports only its own failure.
      const reportedOnPress = () => {};
      if (!released.moved) {
        released.picked.then((entities) => {
          const hit = entities[0];
          if (!released.shift) selection.getState().select(hit ? [hit] : []);
          else if (hit) selection.getState().add(hit);
        }, reportedOnPress);
        return;
      }
      // A drag moves the selection it started on by the same offset, as one edit on release.
      const offset = dragOffset(released);
      released.picked
        .then(async () => {
          if (released.target && control) await moveBy(editor, control, dragged(released.target), offset, state);
        }, reportedOnPress)
        .catch(report("Moving the dragged entities in the viewport"));
    };

    // The browser took the pointer (a gesture, a lost touch): drop the press, move nothing.
    const onPointerCancel = () => {
      press = null;
      repaint = true;
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
      // While playing, plain keys are the game's; without a game there is nothing to change.
      const playing = editor.timeline.getState().playing;
      if (!editor.control) return;
      if (event.key === "Delete" && !playing) {
        event.preventDefault();
        void runReported(editor, "scene.despawn-selection");
        return;
      }
      const direction = ARROWS[event.key];
      if (!direction || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
      if (playing) return;
      event.preventDefault();
      void runReported(editor, `viewport.nudge-${direction}`);
    };
    const onKeyUp = (event: KeyboardEvent) => {
      if (event.key === " ") space = false;
    };
    const onBlur = () => (space = false);

    const listeners: [string, EventListener][] = [
      ["pointerdown", onPointerDown as EventListener],
      ["pointermove", onPointerMove as EventListener],
      ["pointerup", onPointerUp as EventListener],
      ["pointercancel", onPointerCancel],
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
