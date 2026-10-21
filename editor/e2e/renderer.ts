import { existsSync } from "node:fs";
import { join } from "node:path";
import { expect, type Page } from "@playwright/test";

// The browser renderer, which the fixture page loads from its `pkg/`.
const RENDERER = join(import.meta.dirname, "fixtures/renderer/pkg/ludenea_render.js");

export type Rgba = [number, number, number, number];
export type Point = [number, number];
/** What `push_frame` and `push_frame_bytes` return. */
export interface Pushed {
  tick: number;
  missing: unknown[];
  debug: { layers: Record<string, unknown[]> };
}
export interface Drawn {
  report: Pushed;
  pixels: Rgba[];
}
/** A JavaScript `Error` the renderer threw. */
export interface Thrown {
  name: string;
  message: string;
}
export interface Entity {
  index: number;
  generation: number;
}
export interface Vec2 {
  x: number;
  y: number;
}
export interface Bounds {
  min: Vec2;
  max: Vec2;
}
/** A texture the renderer refused. */
export interface Refused {
  id: string;
  path: string | null;
  message: string;
}

declare global {
  interface Window {
    /** Set by `fixtures/renderer/main.js`. */
    fixture: {
      start(): Promise<{ backend: string; errors: Refused[] }>;
      drawReference(points: Point[]): Drawn;
      drawAfterGap(points: Point[]): Rgba[];
      redraw(alpha: number, points: Point[], camera?: { center: { x: number; y: number }; view: { x: number; y: number } }): Rgba[];
      drawSprites(count: number, placed: Record<number, Point>, points: Point[]): Drawn;
      drawError(): Thrown | null;
      failOnTheDevice(): void;
      loseTheDevice(): void;
      createOn(width: number, height: number): Promise<Thrown | null>;
      resize(width: number, height: number): void;
      pushBothWays(points: Point[]): Promise<{ fromJson: Drawn; fromBytes: Drawn }>;
      answers(
        point: Point,
        entities: Entity[],
      ): {
        picked: Entity[];
        pickedThroughAFreeCamera: Entity[];
        bounds: Bounds | null;
        world: Vec2;
        background: Vec2;
      };
      framesOf(meta: unknown): Promise<{ frames?: unknown; error?: Thrown }>;
      badInputs(): Record<"json" | "overlay" | "bytes" | "camera" | "pngCount", Thrown | null>;
      panicked(): { received: string[]; thrown: boolean };
      pickHalfway(): Record<"beforeDraw" | "afterDraw", { halfway: Entity[]; current: Entity[]; bounds: Bounds | null }>;
      pickAcrossNewTextures(): Promise<Record<"drawn" | "blank" | "pushedAgain", { picked: Entity[]; bounds: Bounds | null }>>;
    };
  }
}

/**
 * Opens the fixture page and its renderer. `pageErrors` collects the page's errors from then
 * on (a Rust panic among them); it is empty once started.
 */
export async function start(page: Page): Promise<{ backend: string; refused: Refused[]; pageErrors: string[] }> {
  expect(existsSync(RENDERER), `${RENDERER} is missing: run cargo xtask wasm-render --test-hooks editor/e2e/fixtures/renderer/pkg`).toBe(true);
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") pageErrors.push(message.text());
    console.log(`[page] ${message.text()}`);
  });
  await page.goto("/renderer/index.html");
  await page.waitForFunction(() => window.fixture !== undefined);
  const { backend, errors } = await page.evaluate(() => window.fixture.start());
  expect(pageErrors).toEqual([]);
  return { backend, refused: errors, pageErrors };
}
