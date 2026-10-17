import { ControlClient, ProjectClient, fakeTransport, type GameInfo } from "@ludenea/control-client";
import { createEditor } from "../src";

const game: GameInfo = { components: [], resources: [], input: null, tick_rate: 60, debug_layers: [] };

/** An editor on a project service that welcomes it and answers nothing else. */
export async function testHost(importModule?: (url: string) => Promise<unknown>) {
  const project = await ProjectClient.connect(
    fakeTransport((raw) => (JSON.parse(raw).type === "hello" ? [JSON.stringify({ type: "welcome", protocol_version: 1 })] : [])),
  );
  return createEditor({ project, importModule });
}

/** A game whose host answers each command with `answer(command)` (nothing when undefined). */
export async function testGame(answer: (command: { type: string }) => object | undefined = () => undefined) {
  const sent: { type: string; [key: string]: unknown }[] = [];
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") {
      return [JSON.stringify({ type: "welcome", protocol_version: 2, engine_version: "0", game })];
    }
    sent.push(message.command);
    const reply = answer(message.command);
    return reply ? [JSON.stringify({ type: "response", id: message.id, result: { ok: reply } })] : [];
  });
  const control = await ControlClient.connect(transport);
  return { control, sent, push: (event: object) => transport.push(JSON.stringify({ type: "event", event })) };
}
