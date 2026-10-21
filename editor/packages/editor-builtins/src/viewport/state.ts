import type { Command, ControlClient, DebugDrawFrame, Entity, Vec2 } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { onEntity } from "../inspector";
import type { Store } from "../shared";
import { assertEditable, batched, edit } from "../timeline/editing";
import type { Camera, Size } from "./camera";

/** What the viewport's commands and panel share; kept while the panel is hidden. */
export type ViewportState = Store<{
  camera: Camera;
  size: Size;
  /** Debug layers toggled off, by name; the game's other layers are drawn. */
  hidden: string[];
  /** The debug drawing of the frame on screen, which the overlay's labels and outlines read. */
  frame: DebugDrawFrame | null;
  tick: number;
  /** Whether the viewport shows what the game camera shows instead of the editor camera. */
  gameCamera: boolean;
  /** Bumped to ask the panel for a fresh frame (after an edit). */
  revision: number;
}>;

const POSE = "ludenea::Pose";

export const visibleLayers = (control: ControlClient | null, state: ViewportState): string[] =>
  (control?.game.debug_layers ?? []).filter((layer) => !state.get().hidden.includes(layer));

/** Moves the `Pose` of each of `entities` by `offset` meters, as one edit; entities without a `Pose` stay put. */
export async function moveBy(editor: Editor, control: ControlClient, entities: readonly Entity[], offset: Vec2, state: ViewportState): Promise<void> {
  assertEditable(editor);
  const replies = await Promise.all(entities.map((entity) => onEntity(editor, control.request({ type: "get_entity", entity }))));
  const moves = replies.flatMap(({ entity, components }): Command[] => {
    const pose = components[POSE];
    if (pose?.type !== "json") return [];
    const { position } = pose.value as { position: Vec2 };
    const value = { x: position.x + offset.x, y: position.y + offset.y };
    return [{ type: "set_field", target: { type: "component", entity, component: POSE }, path: [{ type: "field", name: "position" }], value }];
  });
  if (moves.length === 0) return;
  await onEntity(editor, edit(editor, control, batched(moves)));
  state.set({ revision: state.get().revision + 1 });
}
