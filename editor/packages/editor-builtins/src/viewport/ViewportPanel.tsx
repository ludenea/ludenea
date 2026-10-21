import { WebRenderer } from "@ludenea/render-wasm";
import type { Entity, Vec2 } from "@ludenea/control-client";
import { runReported, type Editor, type ViewportOverlay } from "@ludenea/editor-shell";
import { useEffect, useRef, useState } from "react";
import { captureInput } from "../input/capture";
import { errorText, loadRenderModule, reportedOnce, shared, useGame } from "../shared";
import { ResimulatingNotice } from "../timeline/editing";
import { fromCameraItem, panBy, screenToWorld, toCameraView, zoomAt, type Box, type Camera, type Size } from "./camera";
import { FrameFeed } from "./frames";
import { mergeHits, moveBy, moveCamera, same, visibleLayers, type Shown, type ViewportState } from "./state";
import { drawOverlay } from "./overlay";
import { loadTextures } from "./textures";

/** Pointer travel, in CSS pixels, past which a press is a drag rather than a click. */
const DRAG_THRESHOLD = 3;
const WHEEL_ZOOM = 0.0015;
const ARROWS: { [key: string]: string } = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" };

async function createRenderer(canvas: HTMLCanvasElement): Promise<WebRenderer> {
  await loadRenderModule();
  return WebRenderer.create(canvas);
}

/** A texture the renderer could not load: it draws as a placeholder. */
type Refused = { id: string; path: string | null; message: string };

/** The editor camera showing what `renderer` last drew through the game camera, in a viewport of `size`. */
function shownCamera(renderer: WebRenderer, size: Size): Camera {
  const at = (x: number, y: number) => JSON.parse(renderer.screen_to_world(x, y, 1, null)) as Vec2;
  // A side of 0 pixels counts as 1, as the renderer draws it.
  const [topLeft, bottomRight] = [at(0, 0), at(Math.max(size.width, 1), Math.max(size.height, 1))];
  const center = { x: (topLeft.x + bottomRight.x) / 2, y: (topLeft.y + bottomRight.y) / 2 };
  return fromCameraItem({ center, view: { x: bottomRight.x - topLeft.x, y: topLeft.y - bottomRight.y } }, size);
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

/**
 * `failedOverlays`: the plugin overlays whose failure is reported already, kept across mounts: once
 * per registration. `textureWarnings`: the texture warnings logged already, kept across mounts.
 * `shown`: where the panel tells the viewport's commands what the game camera shows.
 */
export function ViewportPanel({
  editor,
  state,
  failedOverlays,
  textureWarnings,
  shown,
}: {
  editor: Editor;
  state: ViewportState;
  failedOverlays: Set<ViewportOverlay>;
  textureWarnings: Set<string>;
  shown: Shown;
}) {
  const game = useGame(editor);
  const root = useRef<HTMLDivElement>(null);
  const gpu = useRef<HTMLCanvasElement>(null);
  const overlay = useRef<HTMLCanvasElement>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    const element = root.current!;
    const context = overlay.current!.getContext("2d");
    let renderer: WebRenderer | null = null;
    let rendered: Size & { ratio: number } = { width: 0, height: 0, ratio: 0 };
    let ratio = window.devicePixelRatio || 1;
    let disposed = false;
    // The current frame must be drawn again (paused; playing draws every animation frame).
    let repaint = true;
    let press: Press | null = null;
    let space = false;
    let seen = state.get();
    const report = (context: string) => (error: unknown) => editor.console.report(error, context);
    const drawing = reportedOnce(editor, "Drawing the viewport");
    const warnOnce = (text: string) => {
      if (textureWarnings.has(text)) return;
      textureWarnings.add(text);
      editor.console.log("warning", text);
    };
    const feed = new FrameFeed(editor, {
      layers: () => visibleLayers(editor.control, state),
      selection: () => editor.selection.getState().entities,
      pushed: ({ tick, debug }) => state.set({ frame: debug, tick }),
    });
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

    // Textures belong to the project: loaded for each renderer, which starts blank without them.
    const uploadTextures = (target: WebRenderer) =>
      loadTextures(editor.project, warnOnce)
        .then(({ sources, pngs }) => {
          if (disposed || renderer !== target) return;
          const refused = JSON.parse(target.set_textures(JSON.stringify(sources), pngs)) as Refused[];
          for (const { id, path, message } of refused) warnOnce(`${path ?? id}: ${message}`);
          // New textures blank the canvas until the next push.
          feed.again();
          repaint = true;
        })
        .catch((error) => {
          if (!disposed) editor.console.report(error, "Loading the textures for the viewport");
        });

    createRenderer(gpu.current!).then(
      (created) => {
        if (disposed) return created.free();
        renderer = created;
        feed.setTarget(created);
        repaint = true;
        void uploadTextures(created);
      },
      (error) => setFailure(`The viewport cannot render: ${errorText(error)}`),
    );

    // What the overlay, picking and tools see through: the game camera as drawn, or the editor's.
    const viewCamera = (): Camera => {
      const { camera, size, gameCamera } = state.get();
      return gameCamera && renderer ? shownCamera(renderer, size) : camera;
    };
    shown.camera = viewCamera;
    // What the last draw showed of `entities`' sprites and tilemaps.
    const drawnBounds = (entities: readonly Entity[]): Box | null => {
      const box = renderer?.bounds(JSON.stringify(entities));
      return box ? (JSON.parse(box) as Box) : null;
    };
    shown.bounds = drawnBounds;

    const paint = () => {
      repaint = false;
      const { camera, size, frame, tick, gameCamera } = state.get();
      const layers = visibleLayers(editor.control, state);
      const selection = editor.selection.getState().entities;
      if (renderer) {
        try {
          if (size.width !== rendered.width || size.height !== rendered.height || ratio !== rendered.ratio) {
            renderer.resize(size.width, size.height, ratio);
            rendered = { ...size, ratio };
          }
          renderer.draw(feed.alpha(performance.now()), gameCamera ? null : JSON.stringify(toCameraView(camera, size)));
          drawing.worked();
        } catch (error) {
          // The canvas has no texture this frame (hidden or occluded): draw again on the next one.
          if (error instanceof Error && error.name === "NoSurfaceTexture") repaint = true;
          else drawing.failed(error);
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
      drawOverlay(context, { camera: viewCamera(), size, frame, layers, tick, selection, overlays, drag }, ratio, overlayFailed);
    };

    // Every animation frame while playing, between the ticks the host pushes; on change otherwise.
    let animation = requestAnimationFrame(function loop() {
      // Moving the window to another screen or zooming the page changes it, with no event here.
      if ((window.devicePixelRatio || 1) !== ratio) {
        ratio = window.devicePixelRatio || 1;
        repaint = true;
      }
      if (editor.timeline.getState().playing || repaint) paint();
      animation = requestAnimationFrame(loop);
    });

    const stops = [
      state.subscribe(() => {
        const next = state.get();
        if (next.hidden !== seen.hidden) feed.layersChanged();
        if (next.revision !== seen.revision) feed.refresh();
        seen = next;
        repaint = true;
      }),
      editor.selection.subscribe(() => {
        feed.again();
        repaint = true;
      }),
      shared(editor).overlays.subscribe(() => (repaint = true)),
      subscribeGame(editor, () => (repaint = true)),
    ];

    const local = (event: MouseEvent): Point => {
      const rect = element.getBoundingClientRect();
      return { x: event.clientX - rect.left, y: event.clientY - rect.top };
    };
    const world = (point: Point) => screenToWorld(viewCamera(), state.get().size, point);
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
        // The renderer answers for what it drew last, so it is asked now, through the camera on screen.
        const drawn = new Promise<Entity[]>((resolve) =>
          resolve(renderer ? (JSON.parse(renderer.pick(at.x, at.y, JSON.stringify(toCameraView(viewCamera(), state.get().size)))) as Entity[]) : []),
        );
        const hosted = control.request({ type: "pick", point: world(at) }).then(({ entities }) => entities);
        const picked = Promise.all([hosted, drawn]).then(([host, sprites]) => mergeHits(host, sprites));
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
        const [dx, dy] = [at.x - press.last.x, at.y - press.last.y];
        moveCamera(state, shown, (camera) => panBy(camera, dx, dy));
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
      const at = local(event);
      moveCamera(state, shown, (camera, size) => zoomAt(camera, size, at, Math.exp(-event.deltaY * WHEEL_ZOOM)));
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
      feed.dispose();
      if (shown.camera === viewCamera) shown.camera = null;
      if (shown.bounds === drawnBounds) shown.bounds = null;
      for (const [type, listener] of listeners) element.removeEventListener(type, listener);
      element.removeEventListener("wheel", onWheel);
      renderer?.free();
    };
  }, [editor, state, failedOverlays, textureWarnings, shown]);

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
