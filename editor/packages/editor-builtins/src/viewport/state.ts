import type { ControlClient, DebugDrawFrame, Entity, Vec2 } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { onEntity } from "../inspector";
import type { Store } from "../shared";
import { assertEditable, edit } from "../timeline/editing";
import type { Camera, Size } from "./camera";

/** What the viewport's commands and panel share; kept while the panel is hidden. */
export type ViewportState = Store<{
  camera: Camera;
  size: Size;
  /** Debug layers toggled off, by name; the game's other layers are drawn. */
  hidden: string[];
  frame: DebugDrawFrame | null;
  tick: number;
  /** Bumped to ask the panel for a fresh frame (after an edit). */
  revision: number;
}>;

const POSE = "ludenea::Pose";

export const visibleLayers = (control: ControlClient | null, state: ViewportState): string[] =>
  (control?.game.debug_layers ?? []).filter((layer) => !state.get().hidden.includes(layer));

/** Moves `entity`'s `Pose` by `offset` meters with one `SetField`; entities without a `Pose` stay put. */
export async function moveBy(editor: Editor, control: ControlClient, entity: Entity, offset: Vec2, state: ViewportState): Promise<void> {
  assertEditable(editor);
  const { components } = await onEntity(editor, control.request({ type: "get_entity", entity }));
  const pose = components[POSE];
  if (pose?.type !== "json") return;
  const { position } = pose.value as { position: Vec2 };
  await onEntity(
    editor,
    edit(editor, control, {
      type: "set_field",
      target: { type: "component", entity, component: POSE },
      path: [{ type: "field", name: "position" }],
      value: { x: position.x + offset.x, y: position.y + offset.y },
    }),
  );
  state.set({ revision: state.get().revision + 1 });
}
