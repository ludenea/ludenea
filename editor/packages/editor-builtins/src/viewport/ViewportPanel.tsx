import type { WebRenderer } from "@ludenea/render-wasm";
import type { Entity, FileChange, Vec2 } from "@ludenea/control-client";
import { runReported, type Editor, type ToolContext, type ToolPointer, type ViewportOverlay, type ViewportTool } from "@ludenea/editor-shell";
import { useEffect, useRef, useState } from "react";
import { captureInput } from "../input/capture";
import { errorText, loadRenderModule, onRenderPanic, reportedOnce, shared, useGame } from "../shared";
import { ResimulatingNotice } from "../timeline/editing";
import { fromCameraItem, panBy, same, screenToWorld, toCameraView, zoomAt, type Box, type Camera, type Size } from "./camera";
import { FrameFeed } from "./frames";
import { mergeHits, moveBy, moveCamera, visibleLayers, type Shown, type ViewportState } from "./state";
import { drawOverlay } from "./overlay";
import { loadTextures, reloadChanged, type LoadedTextures } from "./textures";

/** Pointer travel, in CSS pixels, past which a press is a drag rather than a click. */
const DRAG_THRESHOLD = 3;
const WHEEL_ZOOM = 0.0015;
const ARROWS: { [key: string]: string } = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" };
/** Failing draws in a row (a hidden canvas aside) after which the viewport stops and offers a retry. */
const FAILED_DRAWS_TO_STOP = 3;
/** A lost GPU device is recovered from at most once in this time; a second loss stops the viewport. */
const RECREATION_INTERVAL_MS = 60_000;
const FILL = { position: "absolute", inset: 0, width: "100%", height: "100%" } as const;

async function createRenderer(canvas: HTMLCanvasElement): Promise<WebRenderer> {
  const { WebRenderer } = await loadRenderModule();
  return WebRenderer.create(canvas);
}

/** A canvas for one renderer: a lost WebGPU context cannot be configured again, so a new renderer gets a new canvas. */
function gpuCanvas(): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  Object.assign(canvas.style, FILL);
  return canvas;
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

/** The overlay drawing each viewport tool's `overlay`: the same one every frame, so its failure is reported once. */
const toolOverlays = new WeakMap<ViewportTool, ViewportOverlay>();
function toolOverlay(tool: ViewportTool): ViewportOverlay | null {
  const draw = tool.overlay;
  if (!draw) return null;
  let overlay = toolOverlays.get(tool);
  if (!overlay) toolOverlays.set(tool, (overlay = { id: tool.id, draw: (context) => draw.call(tool, context) }));
  return overlay;
}

type Point = { x: number; y: number };
type Press = {
  mode: "pan" | "select" | "tool";
  start: Point;
  last: Point;
  moved: boolean;
  shift: boolean;
  picked?: Promise<Entity[]>;
  /** The selected entity under the press, once the pick answers: a drag moves the selection holding it. */
  target?: Entity | null;
  /** The tool the press is for: it hears the moves and the release while it stays active. */
  tool?: ViewportTool;
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
  const overlay = useRef<HTMLCanvasElement>(null);
  const retry = useRef(() => {});
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    const element = root.current!;
    const context = overlay.current!.getContext("2d");
    let renderer: WebRenderer | null = null;
    let rendered: Size & { ratio: number } = { width: 0, height: 0, ratio: 0 };
    let ratio = window.devicePixelRatio || 1;
    let disposed = false;
    // The GPU canvas is the panel's own, not React's: it is replaced with each renderer.
    let gpu: HTMLCanvasElement | null = null;
    // Stopped after a failure the viewport does not recover from alone, until a retry.
    let stopped = false;
    let failedDraws = 0;
    // When the renderer was last created again after a lost device (`performance.now()`).
    let recreated = -Infinity;
    // Counts the creations started: one finishing after a newer one or a stop is dropped.
    let creations = 0;
    // Counts the panics of the renderer's module: a renderer created across one belongs to an aborted instance.
    let panics = 0;
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

    // The project's textures as the viewport last knew them: a renderer created again starts from
    // them, not from the disk. Reloads queue behind the initial load; one that fails is dropped.
    let known: Promise<LoadedTextures> | null = null;
    const upload = (target: WebRenderer, { sources, pngs }: LoadedTextures) => {
      if (disposed || renderer !== target) return;
      const refused = JSON.parse(target.set_textures(JSON.stringify(sources), pngs)) as Refused[];
      for (const { id, path, message } of refused) warnOnce(`${path ?? id}: ${message}`);
      // New textures blank the canvas until the next push.
      feed.again();
      repaint = true;
    };
    const uploadTextures = (target: WebRenderer) => {
      known ??= loadTextures(editor, warnOnce);
      return known.then((textures) => upload(target, textures)).catch((error) => {
        // Only the initial load starts over: a reload that fails leaves the set as it was.
        known = null;
        if (!disposed) editor.console.report(error, "Loading the textures for the viewport");
      });
    };
    // A changed texture is drawn at once, with no command to the game: the session does not change.
    const reload = (changes: FileChange[]) => {
      if (!known) return;
      let before: LoadedTextures | null = null;
      const next = known.then((current) => {
        before = current;
        // A fresh warning for each break: a version that works clears its path's earlier ones.
        const loaded = (path: string) => textureWarnings.forEach((text) => text.startsWith(`${path}: `) && textureWarnings.delete(text));
        return reloadChanged(current, changes, editor, warnOnce, loaded).catch((error) => {
          if (!disposed) editor.console.report(error, "Reloading the textures for the viewport");
          return current;
        });
      });
      known = next;
      next.then((textures) => renderer && textures !== before && upload(renderer, textures)).catch((error) => editor.console.report(error, "Drawing the reloaded textures in the viewport"));
    };

    // Drops the renderer, freed unless `free` is false: the frames wait for the next one, which starts with them.
    const release = (free = true) => {
      const dropped = renderer;
      renderer = null;
      feed.setTarget(null);
      rendered = { width: 0, height: 0, ratio: 0 };
      if (free) dropped?.free();
    };

    // Stops drawing and shows `message` with a retry; the session goes on.
    const stop = (message: string, free = true) => {
      if (stopped || disposed) return;
      stopped = true;
      creations++;
      release(free);
      setFailure(message);
    };
    // A panic aborts the module in the middle of a call: the renderer it was using is dropped, not
    // freed, and the next one is created in a new instance of the module.
    const stopPanics = onRenderPanic((message) => {
      panics++;
      stop(message, false);
    });

    const start = () => {
      const creation = ++creations;
      const panicsBefore = panics;
      stopped = false;
      failedDraws = 0;
      setFailure(null);
      const fresh = gpuCanvas();
      if (gpu) gpu.replaceWith(fresh);
      else overlay.current!.before(fresh);
      gpu = fresh;
      createRenderer(fresh).then(
        (created) => {
          if (disposed || creation !== creations) {
            // Nothing is called on an aborted instance, not even `free`.
            if (panics === panicsBefore) created.free();
            return;
          }
          renderer = created;
          feed.setTarget(created);
          repaint = true;
          void uploadTextures(created);
        },
        (error) => {
          if (creation === creations) stop(errorText(error));
        },
      );
    };
    retry.current = () => {
      if (stopped && !disposed) start();
    };
    start();

    // Recovers from a lost GPU device by creating the renderer again, once in a while at most.
    const deviceLost = (error: Error) => {
      const now = performance.now();
      if (now - recreated < RECREATION_INTERVAL_MS) return stop(error.message);
      recreated = now;
      editor.console.log("warning", `The GPU device of the viewport was lost (${error.message}): the viewport created its renderer again`);
      release();
      start();
    };

    // What the overlay, picking and tools see through: the game camera as drawn, or the editor's.
    const viewCamera = (): Camera => {
      const { camera, size, gameCamera } = state.get();
      return gameCamera && renderer ? shownCamera(renderer, size) : camera;
    };
    shown.camera = viewCamera;
    // The camera the renderer draws and picks through: the frame's own (null) through the game camera, else the editor's.
    const drawnCamera = (): string | null => {
      const { camera, size, gameCamera } = state.get();
      return gameCamera ? null : JSON.stringify(toCameraView(camera, size));
    };
    // What the last draw showed of `entities`' sprites and tilemaps.
    const drawnBounds = (entities: readonly Entity[]): Box | null => {
      const box = renderer?.bounds(JSON.stringify(entities));
      return box ? (JSON.parse(box) as Box) : null;
    };
    shown.bounds = drawnBounds;

    const paint = () => {
      repaint = false;
      const { size, frame, tick } = state.get();
      const layers = visibleLayers(editor.control, state);
      const selection = editor.selection.getState().entities;
      if (renderer) {
        try {
          if (size.width !== rendered.width || size.height !== rendered.height || ratio !== rendered.ratio) {
            renderer.resize(size.width, size.height, ratio);
            rendered = { ...size, ratio };
          }
          renderer.draw(feed.alpha(performance.now()), drawnCamera());
          drawing.worked();
          failedDraws = 0;
        } catch (error) {
          const named = error instanceof Error ? error : null;
          // A panic stopped the viewport during the draw: the trap ending the call is that panic, shown already.
          if (stopped) return;
          // The canvas has no texture this frame (hidden or occluded): draw again on the next one.
          if (named?.name === "NoSurfaceTexture") repaint = true;
          else if (named?.name === "DeviceLost") deviceLost(named);
          else {
            drawing.failed(error);
            // Drawn again on the next frame, until it works or fails too often.
            repaint = true;
            if (++failedDraws >= FAILED_DRAWS_TO_STOP) stop(errorText(error));
          }
        }
      }
      if (!context) return;
      const canvas = context.canvas;
      if (canvas.width !== Math.round(size.width * ratio)) canvas.width = Math.round(size.width * ratio);
      if (canvas.height !== Math.round(size.height * ratio)) canvas.height = Math.round(size.height * ratio);
      const drag = press?.mode === "select" && press.moved && press.target ? { entities: dragged(press.target), offset: dragOffset(press) } : null;
      const tool = activeTool();
      const drawnByTool = tool && toolOverlay(tool);
      const overlays = drawnByTool ? [...editor.overlays.list(), drawnByTool] : editor.overlays.list();
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
      if (!stopped && (editor.timeline.getState().playing || repaint)) paint();
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
      editor.viewport.activeTool.subscribe(() => {
        // A press for the tool that left ends without it, and that tool hears so; a selection
        // or drag ends too, moving nothing. A pan goes on: it is the viewport's whatever the tool.
        if (press?.mode === "tool") abandonToolPress();
        else if (press?.mode === "select") press = null;
        showCursor();
        repaint = true;
      }),
      subscribeGame(editor, () => (repaint = true)),
      editor.project.on("files_changed", ({ changes }) => reload(changes)),
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

    // The active viewport tool, null for select and move.
    const activeTool = (): ViewportTool | null => {
      const id = editor.viewport.activeTool.getState();
      return id === null ? null : (editor.viewport.tools.list().find((tool) => tool.id === id) ?? null);
    };
    const showCursor = () => {
      element.style.cursor = activeTool()?.cursor ?? "";
    };
    showCursor();
    const toolContext: ToolContext = {
      editor,
      screenToWorld(screen, parallax) {
        if (!renderer) throw new Error("the viewport has no renderer to map the point with: it is starting or stopped");
        return JSON.parse(renderer.screen_to_world(screen.x, screen.y, parallax, drawnCamera())) as Vec2;
      },
      redraw: () => void (repaint = true),
    };
    // Ends the tool's press without a release: the tool hears it through `pointerCancel`.
    const abandonToolPress = () => {
      const tool = press?.tool;
      press = null;
      repaint = true;
      if (!tool) return;
      try {
        tool.pointerCancel?.(toolContext);
      } catch (error) {
        editor.console.report(error, `The viewport tool "${tool.id}" cancelling a press`);
      }
    };
    // Gives `event` to `tool`'s `handler`; a tool that throws has its failure reported.
    const toTool = (tool: ViewportTool, handler: "pointerDown" | "pointerMove" | "pointerUp", event: PointerEvent) => {
      const at = local(event);
      const pointer: ToolPointer = { screen: at, world: world(at), button: event.button, shift: event.shiftKey, alt: event.altKey };
      try {
        tool[handler]?.(pointer, toolContext);
      } catch (error) {
        editor.console.report(error, `The viewport tool "${tool.id}" handling a pointer event`);
      }
    };

    // The sprites and tilemaps drawn under `at`, topmost first; none when the renderer cannot say.
    const spritesAt = (at: Point): Entity[] => {
      if (!renderer) return [];
      try {
        return JSON.parse(renderer.pick(at.x, at.y, drawnCamera())) as Entity[];
      } catch (error) {
        editor.console.report(error, "Picking the sprites and tilemaps in the viewport");
        return [];
      }
    };

    // Presses and keys on the viewport's controls (the alert and its Retry) are theirs: capturing
    // the pointer would take their click. Text over the viewport is part of its surface.
    const onControl = (event: Event) => event.target instanceof Element && event.target.closest("button, [role=alert]") !== null;

    const onPointerDown = (event: PointerEvent) => {
      if (onControl(event)) return;
      element.focus();
      const at = local(event);
      const control = editor.control;
      const tool = activeTool();
      if (event.button === 1 || (event.button === 0 && space)) {
        press = { mode: "pan", start: at, last: at, moved: false, shift: false };
      } else if (tool) {
        press = { mode: "tool", start: at, last: at, moved: false, shift: event.shiftKey, tool };
        toTool(tool, "pointerDown", event);
      } else if (event.button === 0 && control) {
        // The renderer answers for what it drew last, so it is asked now; its failure leaves the host's hits.
        const sprites = spritesAt(at);
        const picked = control.request({ type: "pick", point: world(at) }).then(({ entities }) => mergeHits(entities, sprites));
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
      // An active tool hears every move but a pan's, pressed or not.
      const tool = activeTool();
      if (tool && press?.mode !== "pan") toTool(tool, "pointerMove", event);
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
      if (released?.mode === "tool" && released.tool) return toTool(released.tool, "pointerUp", event);
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
      if (press?.mode === "tool") return abandonToolPress();
      press = null;
      repaint = true;
    };

    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const at = local(event);
      moveCamera(state, shown, (camera, size) => zoomAt(camera, size, at, Math.exp(-event.deltaY * WHEEL_ZOOM)));
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (onControl(event)) return;
      if (event.key === " ") {
        space = true;
        event.preventDefault();
        return;
      }
      if (event.key === "Escape" && editor.viewport.activeTool.getState() !== null) {
        event.preventDefault();
        editor.viewport.setTool(null);
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
      gpu?.remove();
      stopPanics();
      retry.current = () => {};
    };
  }, [editor, state, failedOverlays, textureWarnings, shown]);

  return (
    <div ref={root} role="application" aria-label="Viewport" tabIndex={0} style={{ position: "relative", width: "100%", height: "100%", overflow: "hidden", outline: "none" }}>
      <canvas ref={overlay} style={{ ...FILL, pointerEvents: "none" }} />
      {failure && (
        <div role="alert" className="ludenea-banner" style={{ position: "absolute", margin: 8 }}>
          The viewport stopped rendering: {failure}
          <button onClick={() => retry.current()}>Retry</button>
        </div>
      )}
      {!failure && !game && (
        <p className="ludenea-muted" style={{ position: "absolute", margin: 8 }}>No game is running.</p>
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
