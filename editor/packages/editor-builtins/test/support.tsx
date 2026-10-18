import { ControlClient, ProjectClient, fakeTransport, type GameInfo, type TypeSchema } from "@ludenea/control-client";
import type { PluginDefinition } from "@ludenea/editor-shell";
import { createEditor, type EditorHost } from "@ludenea/editor-shell/app";
import { act, render } from "@testing-library/react";
import { logToConsole } from "../src";

type Message = { type: string; [key: string]: unknown };
type Answer = (message: Message) => object | undefined;

const game: GameInfo = { components: [], resources: [], assets: [], events: [], fingerprint: "0000000000000000", input: null, tick_rate: 60, debug_layers: [] };

/** A client over a fake peer answering each request with `answer(request)` (nothing when undefined). */
async function fake<C>(
  welcome: object,
  field: "command" | "request",
  answer: Answer,
  connect: (transport: ReturnType<typeof fakeTransport>) => Promise<C>,
) {
  const sent: Message[] = [];
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [JSON.stringify(welcome)];
    sent.push(message[field]);
    const reply = answer(message[field]);
    // `{ error: { code, message } }` answers with a failure.
    const result = reply && "error" in reply ? reply : { ok: reply };
    return reply ? [JSON.stringify({ type: "response", id: message.id, result })] : [];
  });
  const client = await connect(transport);
  return { client, sent, push: (event: object) => transport.push(JSON.stringify({ type: "event", event })) };
}

export const testProject = (answer: Answer = () => undefined) =>
  fake({ type: "welcome", protocol_version: 1 }, "request", answer, ProjectClient.connect);

export const testGame = (answer: Answer = () => ({ type: "done" }), info: Partial<GameInfo> = {}) =>
  fake({ type: "welcome", protocol_version: 2, engine_version: "0", game: { ...game, ...info } }, "command", answer, ControlClient.connect);

/** An editor on a fake project service with `plugins` active. */
export async function testEditor(plugins: PluginDefinition[], answer?: Answer) {
  const project = await testProject(answer);
  const host: EditorHost = createEditor({ project: project.client, log: (level, text, location) => logToConsole(host.editor, level, text, location) });
  for (const plugin of plugins) host.loader.activate(plugin);
  return { ...host, project };
}

export function renderPanel(host: EditorHost, id: string): void {
  const Panel = host.registry.panels.get(id)!.component;
  render(<Panel editor={host.editor} />);
}

/** Lets pending replies and the renders they cause settle. */
export const settle = () => act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));

/** Sets what the game's timeline reports about unsaved scene edits. */
export function setSceneDirty(host: EditorHost, sceneDirty: boolean) {
  const timeline = { head: 0, simulated: 0, current: 0, checkpoints: [], edit_ticks: [], can_undo: false, can_redo: false, undo_label: null, redo_label: null, resimulating: false, branch: 0, branches: [{ id: 0, name: "Branch 1", parent: null, fork_tick: 0, head: 0, stale: false, edit_ticks: [] }], compare: null };
  act(() => host.editor.timeline.setState({ timeline: { ...timeline, scene_dirty: sceneDirty } }));
}

const f32 = { kind: "f32" } as const;
const vec2: TypeSchema = { kind: "struct", name: "ludenea_math::Vec2", fields: [{ name: "x", schema: f32 }, { name: "y", schema: f32 }] };
const circle: TypeSchema = { kind: "struct", name: "ludenea_physics::shape::Circle", fields: [{ name: "center", schema: vec2 }, { name: "radius", schema: f32 }] };

/** Engine-like schemas, as `GameInfo` carries them. */
export const schemas = {
  body: {
    kind: "struct",
    name: "ludenea_physics::components::Body",
    fields: [
      { name: "kind", schema: { kind: "enum", name: "ludenea_physics::components::BodyKind", variants: ["Static", "Kinematic", "Dynamic"].map((name) => ({ name, fields: { kind: "unit" } })) } },
      { name: "linear_velocity", schema: vec2 },
      { name: "angular_velocity", schema: f32 },
      { name: "linear_damping", schema: f32 },
      { name: "angular_damping", schema: f32 },
      { name: "gravity_scale", schema: f32 },
    ],
  },
  collider: {
    kind: "struct",
    name: "ludenea_physics::components::Collider",
    fields: [
      {
        name: "shape",
        schema: {
          kind: "enum",
          name: "ludenea_physics::shape::Shape",
          variants: [
            { name: "Circle", fields: { kind: "newtype", item: circle } },
            { name: "Capsule", fields: { kind: "newtype", item: { kind: "struct", name: "ludenea_physics::shape::Capsule", fields: [{ name: "radius", schema: f32 }] } } },
          ],
        },
      },
      { name: "density", schema: f32 },
    ],
  },
  score: { kind: "struct", name: "my_game::Score", fields: [{ name: "points", schema: { kind: "u64" } }] },
} satisfies Record<string, TypeSchema>;
