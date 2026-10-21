// Stands in for the generated `@ludenea/render-wasm` package in tests (see vitest.config.ts),
// so they run without the WebAssembly build. Frames pushed as bytes are their JSON in UTF-8.
import type * as Real from "@ludenea/render-wasm";
import type { RenderFrame, Vec2 } from "@ludenea/control-client";

type Shape = Omit<Real.WebRenderer, "backend" | "set_atlas" | typeof Symbol.dispose>;
type CameraView = { center: Vec2; view: Vec2 };
/** A texture `set_textures` refuses, as the real one reports it. */
type Refused = { id: string; path: string | null; message: string };

/** What the real renderer shows without a camera: 20 by 11.25 world units around the origin. */
const DEFAULT_CAMERA: CameraView = { center: { x: 0, y: 0 }, view: { x: 20, y: 11.25 } };

/** Every renderer created, most recent last. */
export const renderers: WebRenderer[] = [];

/** What the next renderers do: the textures `set_textures` refuses. */
export const script: { refused: Refused[] } = { refused: [] };

export default async function init(): Promise<void> {}

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

export function setPanicHandler(): void {}

export class WebRenderer implements Shape {
  readonly calls: { method: string; args: unknown[] }[] = [];
  /** The frames pushed, oldest first. */
  readonly pushed: RenderFrame[] = [];
  /** How many of the next draws find no surface texture. */
  skips = 0;
  private css = { width: 1, height: 1 };

  static async create(canvas: HTMLCanvasElement): Promise<WebRenderer> {
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
    if (this.skips === 0) return;
    this.skips--;
    throw named("NoSurfaceTexture", "the canvas has no texture this frame");
  }

  pick(): string {
    return "[]";
  }

  bounds(): string | undefined {
    return undefined;
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
