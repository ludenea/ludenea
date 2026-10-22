import type { Color, DebugDrawFrame, Entity, Primitive, Vec2 } from "@ludenea/control-client";
import type { ViewportOverlay } from "@ludenea/editor-shell";
import { same } from "../shared";
import { worldToScreen, type Camera, type Size } from "./camera";

/** The color the renderer highlights selected entities with. */
const HIGHLIGHT = "#ffd400";

export type OverlayScene = {
  camera: Camera;
  size: Size;
  frame: DebugDrawFrame | null;
  layers: readonly string[];
  tick: number;
  selection: readonly Entity[];
  overlays: readonly ViewportOverlay[];
  /** The entities being dragged: their outlines are drawn moved by `offset` meters. */
  drag: { entities: readonly Entity[]; offset: Vec2 } | null;
  /** The renderer's mapping of a world point on a layer of `parallax` to the screen, which overlays get. */
  mapToScreen(point: Vec2, parallax: number): { x: number; y: number };
};

export const css = (color: Color) => `rgba(${color >>> 24}, ${(color >>> 16) & 255}, ${(color >>> 8) & 255}, ${(color & 255) / 255})`;

/** Draws what the GPU renderer does not: labels, the drag outline, and plugin overlays; `failed` hears each overlay that throws. */
export function drawOverlay(context: CanvasRenderingContext2D, scene: OverlayScene, pixelRatio: number, failed: (overlay: ViewportOverlay, error: unknown) => void): void {
  const { camera, size, frame, layers, drag } = scene;
  context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
  context.clearRect(0, 0, size.width, size.height);
  const toScreen = (point: Vec2) => worldToScreen(camera, size, point);
  const items = layers.flatMap((layer) => frame?.layers[layer] ?? []);

  context.font = "12px sans-serif";
  for (const { primitive } of items) {
    if (primitive.type !== "label") continue;
    const at = toScreen(primitive.at);
    context.fillStyle = css(primitive.color);
    context.fillText(primitive.text, at.x, at.y);
  }

  if (drag) {
    const moved = (point: Vec2) => toScreen({ x: point.x + drag.offset.x, y: point.y + drag.offset.y });
    context.strokeStyle = HIGHLIGHT;
    context.lineWidth = 1.5;
    context.setLineDash([4, 3]);
    for (const { entity, primitive } of items) {
      if (entity === null || !drag.entities.some((dragged) => same(entity, dragged))) continue;
      context.beginPath();
      outline(context, primitive, moved, camera.pixelsPerMeter);
      context.stroke();
    }
    context.setLineDash([]);
  }

  for (const overlay of scene.overlays) {
    try {
      overlay.draw({ canvas: context, worldToScreen: (point, parallax = 1) => scene.mapToScreen(point, parallax), tick: scene.tick, selection: scene.selection });
    } catch (error) {
      failed(overlay, error);
    }
  }
}

/** Adds `primitive`'s outline to the current path; `scale` in pixels per meter. */
export function outline(context: CanvasRenderingContext2D, primitive: Primitive, toScreen: (point: Vec2) => { x: number; y: number }, scale: number) {
  const circle = (center: Vec2, radius: number) => {
    const at = toScreen(center);
    context.moveTo(at.x + radius * scale, at.y);
    context.arc(at.x, at.y, radius * scale, 0, 2 * Math.PI);
  };
  const path = (points: Vec2[], closed: boolean) => {
    points.forEach((point, index) => {
      const at = toScreen(point);
      if (index === 0) context.moveTo(at.x, at.y);
      else context.lineTo(at.x, at.y);
    });
    if (closed) context.closePath();
  };
  switch (primitive.type) {
    case "line":
      return path([primitive.a, primitive.b], false);
    case "circle":
      return circle(primitive.center, primitive.radius);
    case "polygon":
      return path(primitive.points, true);
    case "capsule": {
      // Both end circles and both sides: enough to show where the shape goes.
      const { a, b, radius } = primitive;
      const length = Math.hypot(b.x - a.x, b.y - a.y) || 1;
      const side = { x: (-(b.y - a.y) / length) * radius, y: ((b.x - a.x) / length) * radius };
      circle(a, radius);
      circle(b, radius);
      path([{ x: a.x + side.x, y: a.y + side.y }, { x: b.x + side.x, y: b.y + side.y }], false);
      return path([{ x: a.x - side.x, y: a.y - side.y }, { x: b.x - side.x, y: b.y - side.y }], false);
    }
    case "label":
      return;
  }
}
