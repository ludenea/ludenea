import type { AssetId, Entity, PathSegment, TextureMeta, Vec2 } from "@ludenea/control-client";
import type { ComponentType, ReactNode } from "react";
import type { Editor } from "./editor";

export interface Disposable {
  dispose(): void;
}

/** A plugin: built-in, engine, and project plugins all take this shape and get the same `Editor`. */
export interface PluginDefinition {
  name: string;
  /**
   * Registers what the plugin adds; may return a cleanup called on deactivation. What it
   * registers while `activate` runs belongs to the plugin and is disposed on deactivation.
   * Every plugin shares one `Editor`, so a registration made later (after an `await`, in a
   * callback) cannot be told apart: it belongs to the editor, and the plugin disposes it
   * itself, at the latest in the cleanup it returns.
   */
  activate(editor: Editor): void | (() => void);
}

export function definePlugin(plugin: PluginDefinition): PluginDefinition {
  return plugin;
}

export interface PanelDefinition {
  id: string;
  title: string;
  component: ComponentType<{ editor: Editor }>;
  defaultPosition?: "left" | "right" | "bottom" | "center";
}

export interface CommandDefinition {
  id: string;
  title: string;
  /** `args` are what the caller passed to `commands.run` after the identifier; the palette and shortcuts pass none. */
  run(editor: Editor, ...args: unknown[]): void | Promise<void>;
}

/** Replaces the generic inspector for one component. */
export type InspectorComponent<T> = ComponentType<{
  value: T;
  onChange(path: PathSegment[], value: unknown): void;
  entity: Entity;
}>;

/** What kind of project file a selected asset is: its `AssetKind` without a data type or texture meta. */
export type AssetKindName = "Scene" | "Prefab" | "Data" | "Texture" | "Replay" | "Trace" | "Snapshot" | "Other";

/**
 * Inspects one type of asset, registered by its data type name (`"ludenea::SpriteSheet"`) or
 * as `"Texture"`. A data asset's inspector shows below its generic fields, with its value
 * (null when it has no JSON form) and `onChange` setting one field as the generic fields do; a
 * texture's shows alone, with a null `value`, whether or not a game runs.
 */
export type AssetInspector = ComponentType<{
  id: AssetId;
  value: unknown | null;
  onChange(path: PathSegment[], value: unknown): void;
}>;

/** A texture of the project whose meta can be read. */
export interface TextureInfo {
  id: AssetId;
  /** Relative to the project root. */
  path: string;
  meta: TextureMeta;
}

/**
 * What a texture's meta cuts its PNG into, in pixels: the image's size and its frames,
 * row-major, each with its rectangle from the image's top-left corner and its pivot from the
 * frame's top-left corner.
 */
export interface TextureFrames {
  width: number;
  height: number;
  frames: { x: number; y: number; width: number; height: number; pivot: [number, number] }[];
}

/** A texture as `editor.textures.frames` reads it: the frames its meta cuts it into, and the PNG they were cut from. */
export interface TextureCut extends TextureFrames {
  png: Uint8Array;
}

/** A value persisted per installation (`localStorage`), falling back to `default`. */
export interface SettingDefinition<T> {
  id: string;
  title: string;
  default: T;
  /** What `T` is; `list` is a `string[]`. */
  kind: "boolean" | "number" | "string" | "path" | "list";
}

export interface OverlayContext {
  canvas: CanvasRenderingContext2D;
  /** Where a layer of `parallax` (1 when omitted) shows `point`, in CSS pixels, as the viewport's renderer draws it. */
  worldToScreen(point: Vec2, parallax?: number): { x: number; y: number };
  tick: number;
  selection: readonly Entity[];
}

/** Drawn by the viewport on its 2D overlay canvas, every frame it draws. */
export interface ViewportOverlay {
  id: string;
  draw(context: OverlayContext): void;
}

/** A pointer event over the viewport as a tool hears it: `screen` in CSS pixels from the viewport's top-left corner, `world` in world units on a layer of parallax 1. */
export interface ToolPointer {
  screen: { x: number; y: number };
  world: Vec2;
  /** The DOM `PointerEvent.button`: 0 for the main button, -1 for a move with no button change. */
  button: number;
  shift: boolean;
  alt: boolean;
}

/** What a viewport tool works with while it handles an event. */
export interface ToolContext {
  editor: Editor;
  /** Where `screen` (CSS pixels) falls on a layer of `parallax`, as the viewport's renderer draws it. */
  screenToWorld(screen: { x: number; y: number }, parallax: number): Vec2;
  /** Where a layer of `parallax` shows `point`, in CSS pixels: the inverse of `screenToWorld`. */
  worldToScreen(point: Vec2, parallax: number): { x: number; y: number };
  /** Draws the viewport again, its overlays included. */
  redraw(): void;
}

/**
 * A tool of the viewport: while it is active (`editor.viewport.setTool`), the presses, moves,
 * and releases of the pointer over the viewport are its own instead of selecting and moving
 * entities. Panning (Space-drag, middle-drag) and zooming stay the viewport's.
 */
export interface ViewportTool {
  id: string;
  title: string;
  /** The CSS cursor over the viewport while the tool is active. */
  cursor?: string;
  /**
   * Whether the tool can be used now; asked again on every selection, timeline, and game
   * change, and the tool is deactivated once it answers false. The timeline changes on every
   * tick while the game plays, so this runs that often: keep it cheap and synchronous, reading
   * what the editor already holds (`editor.control`, the selection and timeline stores)
   * rather than asking the game.
   */
  available(editor: Editor): boolean;
  pointerDown?(event: ToolPointer, context: ToolContext): void;
  /** Every move over the viewport while the tool is active, pressed or not. */
  pointerMove?(event: ToolPointer, context: ToolContext): void;
  pointerUp?(event: ToolPointer, context: ToolContext): void;
  /**
   * The press `pointerDown` started ends without its `pointerUp`: the browser took the pointer,
   * the tool stopped being active (Escape, another tool, unavailable, unregistered), or the
   * viewport closed. Called once per such press; a stroke in progress is dropped here.
   */
  pointerCancel?(context: ToolContext): void;
  /** Drawn over the viewport after the overlays, while the tool is active. */
  overlay?(context: OverlayContext): void;
}

/** Drawn by the inspector beside every field of every component, the component itself (empty `path`) included. */
export interface FieldDecoration {
  id: string;
  /** Rendered as a component: it may use hooks. */
  render(props: { editor: Editor; entity: Entity; component: string; path: PathSegment[] }): ReactNode;
}

/** A column of the explorer's entity list; `width` in pixels. */
export interface ExplorerColumn {
  id: string;
  title: string;
  width: number;
  value(entity: Entity): string | null;
  /** Calls `listener` whenever `value` may answer differently, so the explorer reads it again in place; returns the unsubscribe. */
  subscribe?(listener: () => void): () => void;
}
