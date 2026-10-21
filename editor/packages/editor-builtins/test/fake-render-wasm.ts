// Stands in for the generated `@ludenea/render-wasm` package in tests (see vitest.config.ts),
// so they run without the WebAssembly build. Frames pushed as bytes are their JSON in UTF-8.
import type * as Real from "@ludenea/render-wasm";
import type { Entity, RenderFrame, Vec2 } from "@ludenea/control-client";

type Shape = Omit<Real.WebRenderer, "backend" | "set_atlas" | typeof Symbol.dispose>;
type CameraView = { center: Vec2; view: Vec2 };
/** A texture `set_textures` refuses, as the real one reports it. */
type Refused = { id: string; path: string | null; message: string };

/** What the real renderer shows without a camera: 20 by 11.25 world units around the origin. */
const DEFAULT_CAMERA: CameraView = { center: { x: 0, y: 0 }, view: { x: 20, y: 11.25 } };

/** Every renderer created, most recent last. */
export const renderers: WebRenderer[] = [];

/** What the next renderers do: the textures `set_textures` refuses; what the next `init` and the next `create` throw. */
export const script: { refused: Refused[]; initError: Error | null; createError: Error | null } = { refused: [], initError: null, createError: null };

/** Every call of `init`. */
export const inits: unknown[] = [];

export default async function init(): Promise<void> {
  inits.push(null);
  const error = script.initError;
  script.initError = null;
  if (error) throw error;
}

/** A JavaScript `Error` named as the real renderer names its errors. */
export function named(name: string, message: string): Error {
  const error = new Error(message);
  error.name = name;
  return error;
}

export function textureFrames(): string {
  return JSON.stringify({ width: 1, height: 1, frames: [{ x: 0, y: 0, width: 1, height: 1, pivot: [0, 0] }] });
}

export function ghostFrame(frame: string): string {
  return frame;
}

let panicHandler: ((message: string) => void) | null = null;

export function setPanicHandler(handler: (message: string) => void): void {
  panicHandler = handler;
}

/** Calls the handler `setPanicHandler` set last with `message`, as the real module does when it panics. */
export function panic(message: string): void {
  panicHandler?.(message);
}

/** What the real renderer says with each name it gives its errors. */
const MESSAGES: { [name: string]: string } = {
  NoSurfaceTexture: "the canvas has no texture this frame",
  DeviceLost: "the GPU device was lost: Destroyed",
  Gpu: "drawing failed on the GPU: out of memory",
};

export class WebRenderer implements Shape {
  readonly calls: { method: string; args: unknown[] }[] = [];
  /** The frames pushed, oldest first. */
  readonly pushed: RenderFrame[] = [];
  /** The error the next draws throw, by name, and how many of them do. */
  private failing = { name: "", count: 0 };
  /** What `pick` answers, topmost first, wherever it is asked. */
  hits: Entity[] = [];
  /** What `bounds` answers for any entities: the box of the sprites and tilemaps drawn. */
  box: { min: Vec2; max: Vec2 } | undefined = undefined;
  /** What `pick` throws instead of answering, while set. */
  pickError: Error | null = null;
  private css = { width: 1, height: 1 };

  static async create(canvas: HTMLCanvasElement): Promise<WebRenderer> {
    const error = script.createError;
    script.createError = null;
    if (error) throw error;
    const renderer = new WebRenderer();
    renderer.calls.push({ method: "create", args: [canvas] });
    renderers.push(renderer);
    return renderer;
  }

  set_textures(sources: string, pngs: Uint8Array[]): string {
    this.calls.push({ method: "set_textures", args: [JSON.parse(sources), pngs] });
    return JSON.stringify(script.refused);
  }

  resize(width: number, height: number, ratio: number) {
    this.calls.push({ method: "resize", args: [width, height, ratio] });
    this.css = { width, height };
  }

  push_frame(json: string, overlay: string): string {
    return this.push("push_frame", JSON.parse(json) as RenderFrame, overlay);
  }

  push_frame_bytes(bytes: Uint8Array, overlay: string): string {
    return this.push("push_frame_bytes", JSON.parse(new TextDecoder().decode(bytes)) as RenderFrame, overlay);
  }

  draw(alpha: number, camera?: string | null) {
    this.calls.push({ method: "draw", args: [alpha, camera ?? null] });
    if (this.failing.count === 0) return;
    this.failing.count--;
    throw named(this.failing.name, MESSAGES[this.failing.name] ?? `${this.failing.name} on purpose`);
  }

  /** The next `count` draws throw the error the real renderer names `name`; `NoSurfaceTexture` is a hidden canvas. */
  fail(name: string, count = Infinity) {
    this.failing = { name, count };
  }

  pick(x: number, y: number, camera?: string | null): string {
    this.calls.push({ method: "pick", args: [x, y, camera ?? null] });
    if (this.pickError) throw this.pickError;
    return JSON.stringify(this.hits);
  }

  bounds(entities: string): string | undefined {
    this.calls.push({ method: "bounds", args: [JSON.parse(entities)] });
    return this.box && JSON.stringify(this.box);
  }

  /** As the real one: through `camera`, else the last frame's camera, fitted to the canvas. */
  screen_to_world(x: number, y: number, parallax: number, camera?: string | null): string {
    const { center, view } = camera ? (JSON.parse(camera) as CameraView) : (this.pushed.at(-1)?.camera ?? DEFAULT_CAMERA);
    const { width, height } = this.css;
    const aspect = width / height;
    const visible = view.x < view.y * aspect ? { x: view.y * aspect, y: view.y } : { x: view.x, y: view.x / aspect };
    return JSON.stringify({
      x: ((2 * x) / width - 1) * (visible.x / 2) + center.x * parallax,
      y: (1 - (2 * y) / height) * (visible.y / 2) + center.y * parallax,
    });
  }

  free() {
    this.calls.push({ method: "free", args: [] });
  }

  private push(method: string, frame: RenderFrame, overlay: string): string {
    this.calls.push({ method, args: [frame, JSON.parse(overlay)] });
    this.pushed.push(frame);
    return JSON.stringify({ tick: frame.tick, missing: [], debug: frame.debug });
  }
}
