import type { CameraItem, DebugDrawFrame, Entity, Vec2 } from "@ludenea/control-client";

/** What the viewport looks at: `center` in meters, the scale in CSS pixels per meter. */
export type Camera = { center: Vec2; pixelsPerMeter: number };
/** The viewport's size in CSS pixels. */
export type Size = { width: number; height: number };
/** A world box, in meters. */
export type Box = { min: Vec2; max: Vec2 };
/** A camera as the renderer takes it: `view` is the minimum visible width and height, in meters. */
export type CameraView = { center: Vec2; view: Vec2 };
type Point = { x: number; y: number };

/** A side of 0 pixels counts as 1, as the renderer draws it. */
const sides = ({ width, height }: Size) => ({ width: Math.max(width, 1), height: Math.max(height, 1) });

/** The renderer's camera showing what `camera` shows in a viewport of `size`. */
export function toCameraView({ center, pixelsPerMeter }: Camera, size: Size): CameraView {
  const { width, height } = sides(size);
  return { center, view: { x: width / pixelsPerMeter, y: height / pixelsPerMeter } };
}

/**
 * The editor camera showing what the game camera `item` shows in a viewport of `size`: its
 * minimum view fits, widened on the longer side, at the same scale on both axes.
 */
export function fromCameraItem({ center, view }: Pick<CameraItem, "center" | "view">, size: Size): Camera {
  const { width, height } = sides(size);
  return { center, pixelsPerMeter: Math.min(width / view.x, height / view.y) };
}

export const MIN_PIXELS_PER_METER = 2;
export const MAX_PIXELS_PER_METER = 2000;

/** World y points up, screen y points down. */
export const worldToScreen = ({ center, pixelsPerMeter }: Camera, size: Size, point: Vec2): Point => ({
  x: size.width / 2 + (point.x - center.x) * pixelsPerMeter,
  y: size.height / 2 - (point.y - center.y) * pixelsPerMeter,
});

/** How a layer of `parallax` sees `camera`: at `center * parallax`, as the renderer draws it. */
export const throughParallax = ({ center, pixelsPerMeter }: Camera, parallax: number): Camera => ({ center: { x: center.x * parallax, y: center.y * parallax }, pixelsPerMeter });

export const screenToWorld = ({ center, pixelsPerMeter }: Camera, size: Size, point: Point): Vec2 => ({
  x: center.x + (point.x - size.width / 2) / pixelsPerMeter,
  y: center.y - (point.y - size.height / 2) / pixelsPerMeter,
});

/** Scales by `factor` (clamped) keeping the world point under `anchor` in place. */
export function zoomAt(camera: Camera, size: Size, anchor: Point, factor: number): Camera {
  const fixed = screenToWorld(camera, size, anchor);
  const pixelsPerMeter = Math.min(MAX_PIXELS_PER_METER, Math.max(MIN_PIXELS_PER_METER, camera.pixelsPerMeter * factor));
  return {
    pixelsPerMeter,
    center: { x: fixed.x - (anchor.x - size.width / 2) / pixelsPerMeter, y: fixed.y + (anchor.y - size.height / 2) / pixelsPerMeter },
  };
}

/** Moves the view so the content follows a drag of `dx`, `dy` screen pixels. */
export const panBy = (camera: Camera, dx: number, dy: number): Camera => ({
  ...camera,
  center: { x: camera.center.x - dx / camera.pixelsPerMeter, y: camera.center.y + dy / camera.pixelsPerMeter },
});

export const same = (a: Entity, b: Entity) => a.index === b.index && a.generation === b.generation;
const sameEntity = (a: Entity | null, b: Entity) => a !== null && same(a, b);

/** The smallest box holding both, either of which may be missing. */
export function union(a: Box | null, b: Box | null): Box | null {
  if (!a || !b) return a ?? b;
  return { min: { x: Math.min(a.min.x, b.min.x), y: Math.min(a.min.y, b.min.y) }, max: { x: Math.max(a.max.x, b.max.x), y: Math.max(a.max.y, b.max.y) } };
}

/** The bounds of what `entities` draw in `layers`, or null when they draw nothing there. */
export function bounds(frame: DebugDrawFrame, layers: readonly string[], entities: readonly Entity[]): Box | null {
  let box: Box | null = null;
  const grow = (point: Vec2, margin = 0) => {
    box = union(box, { min: { x: point.x - margin, y: point.y - margin }, max: { x: point.x + margin, y: point.y + margin } });
  };
  for (const layer of layers) {
    for (const { entity, primitive } of frame.layers[layer] ?? []) {
      if (!entities.some((selected) => sameEntity(entity, selected))) continue;
      switch (primitive.type) {
        case "line":
          [primitive.a, primitive.b].forEach((point) => grow(point));
          break;
        case "capsule":
          [primitive.a, primitive.b].forEach((point) => grow(point, primitive.radius));
          break;
        case "circle":
          grow(primitive.center, primitive.radius);
          break;
        case "polygon":
          primitive.points.forEach((point) => grow(point));
          break;
        case "label":
          grow(primitive.at);
      }
    }
  }
  return box;
}
