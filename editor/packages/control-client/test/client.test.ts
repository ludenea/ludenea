import { describe, expect, it, vi } from "vitest";
import { ControlClient, ProtocolFailure, fakeTransport, type GameInfo } from "../src";

const game: GameInfo = { components: [], resources: [], input: null, tick_rate: 60, debug_layers: [] };
const welcome = JSON.stringify({ type: "welcome", protocol_version: 2, engine_version: "0", game });

/** A transport whose peer welcomes the client and records every request it receives. */
function connected() {
  const sent: { id: number; command: { type: string } }[] = [];
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [welcome];
    sent.push(message);
    return [];
  });
  return { transport, sent, client: ControlClient.connect(transport) };
}

const ok = (id: number, reply: object) => JSON.stringify({ type: "response", id, result: { ok: reply } });

describe("ControlClient", () => {
  it("connect_rejects_version_mismatch_with_message", async () => {
    const transport = fakeTransport(() => [
      JSON.stringify({ type: "refused", error: { code: "unsupported_version", message: "need protocol 2" } }),
    ]);
    await expect(ControlClient.connect(transport)).rejects.toThrow("need protocol 2");
  });

  it("sends_hello_with_the_protocol_version_and_exposes_the_game", async () => {
    const hellos: unknown[] = [];
    const transport = fakeTransport((raw) => {
      hellos.push(JSON.parse(raw));
      return [welcome];
    });
    const client = await ControlClient.connect(transport);
    expect(hellos).toEqual([{ type: "hello", protocol_version: 2 }]);
    expect(client.game).toEqual(game);
  });

  it("responses_resolve_by_id_out_of_order", async () => {
    const { transport, sent, client: connecting } = connected();
    const client = await connecting;
    const first = client.request({ type: "get_hash" });
    const second = client.request({ type: "get_timeline" });
    expect(sent.map((request) => request.id)).toEqual([1, 2]);
    transport.push(ok(2, { type: "done" }));
    transport.push(ok(1, { type: "hash", tick: 3, hash: "7", pending: false }));
    expect((await first).tick).toBe(3);
    expect(await second).toEqual({ type: "done" });
  });

  it("error_result_rejects_with_code", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "play" });
    transport.push(
      JSON.stringify({ type: "response", id: 1, result: { error: { code: "unknown_entity", message: "gone" } } }),
    );
    const failure = await pending.catch((error) => error);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure.code).toBe("unknown_entity");
    expect(failure.message).toBe("gone");
  });

  it("events_reach_typed_listeners_and_unsubscribe_works", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const ticks: number[] = [];
    const stop = client.on("entity_despawned", (event) => ticks.push(Number(event.entity)));
    const other: string[] = [];
    client.on("console", (event) => other.push(event.text));
    transport.push(JSON.stringify({ type: "event", event: { type: "entity_despawned", entity: 5 } }));
    transport.push(JSON.stringify({ type: "event", event: { type: "console", level: "info", text: "hi" } }));
    stop();
    transport.push(JSON.stringify({ type: "event", event: { type: "entity_despawned", entity: 6 } }));
    expect(ticks).toEqual([5]);
    expect(other).toEqual(["hi"]);
  });

  it("malformed_frame_is_ignored_and_does_not_break_pending_requests", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "get_hash" });
    transport.push("{not json");
    transport.push(ok(1, { type: "hash", tick: 1, hash: "1", pending: false }));
    expect((await pending).tick).toBe(1);
    expect(warn).toHaveBeenCalledOnce();
    warn.mockRestore();
  });

  it("close_rejects_pending_requests", async () => {
    const { client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "get_hash" });
    client.close();
    await expect(pending).rejects.toThrow("client closed");
    await expect(client.request({ type: "get_hash" })).rejects.toThrow("closed");
  });

  it("transport_close_rejects_pending_requests_with_reason", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "get_hash" });
    transport.drop("socket reset");
    await expect(pending).rejects.toThrow("socket reset");
  });
});
