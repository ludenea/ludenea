import type { Entity, PathSegment, Vec2 } from "@ludenea/control-client";
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
  worldToScreen(point: Vec2): { x: number; y: number };
  tick: number;
  selection: readonly Entity[];
}

/** Drawn by the viewport on its 2D overlay canvas, every frame it draws. */
export interface ViewportOverlay {
  id: string;
  draw(context: OverlayContext): void;
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
