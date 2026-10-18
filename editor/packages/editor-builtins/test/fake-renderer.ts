// Stands in for the generated `@ludenea/debug-draw-wasm` package in tests (see vitest.config.ts),
// so they run without the WebAssembly build.
import type * as Real from "@ludenea/debug-draw-wasm";

type Shape = Pick<Real.ViewportRenderer, "resize" | "set_camera" | "draw" | "free">;

/** Every renderer created, most recent last. */
export const renderers: ViewportRenderer[] = [];

export default async function init(): Promise<void> {}

export class ViewportRenderer implements Shape {
  readonly calls: { method: string; args: unknown[] }[] = [];

  static async create(canvas: HTMLCanvasElement): Promise<ViewportRenderer> {
    const renderer = new ViewportRenderer();
    renderer.calls.push({ method: "create", args: [canvas] });
    renderers.push(renderer);
    return renderer;
  }

  resize(...args: [number, number, number]) {
    this.calls.push({ method: "resize", args });
  }

  set_camera(...args: [number, number, number]) {
    this.calls.push({ method: "set_camera", args });
  }

  /** How many of the next draws find no surface texture and are skipped. */
  skips = 0;

  draw(...args: [string, string, string]) {
    this.calls.push({ method: "draw", args });
    if (this.skips === 0) return true;
    this.skips--;
    return false;
  }

  free() {
    this.calls.push({ method: "free", args: [] });
  }
}
