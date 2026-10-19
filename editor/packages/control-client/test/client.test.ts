import { describe, expect, it, vi } from "vitest";
import { ControlClient, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, ProtocolFailure, fakeTransport, type GameInfo } from "../src";

const game: GameInfo = { components: [], resources: [], assets: [], events: [], fingerprint: "0000000000000000", input: null, tick_rate: 60, debug_layers: [] };
const identity = { protocol_version: PROTOCOL_VERSION, protocol_fingerprint: PROTOCOL_FINGERPRINT };
const welcome = JSON.stringify({ type: "welcome", ...identity, engine_version: "0", game });

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
      JSON.stringify({ type: "refused", error: { code: "version_mismatch", message: "need protocol 2" } }),
    ]);
    await expect(ControlClient.connect(transport)).rejects.toThrow("need protocol 2");
  });

  it("refused_handshake_closes_the_transport_and_rejects_with_the_code", async () => {
    const transport = fakeTransport(() => [
      JSON.stringify({ type: "refused", error: { code: "version_mismatch", message: "need protocol 2" } }),
    ]);
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    const failure = await ControlClient.connect(transport).catch((error) => error);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure.code).toBe("version_mismatch");
    expect(closes).toEqual(["closed"]);
  });

  it("connect_rejects_a_welcome_of_another_protocol_version", async () => {
    const older = { ...identity, protocol_version: PROTOCOL_VERSION - 1 };
    const transport = fakeTransport(() => [JSON.stringify({ type: "welcome", ...older, engine_version: "0", game })]);
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    const failure = await ControlClient.connect(transport).catch((error) => error);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure.code).toBe("version_mismatch");
    expect(failure.message).toContain(`protocol ${PROTOCOL_VERSION - 1}, this client speaks ${PROTOCOL_VERSION}`);
    expect(failure.message).toContain("pnpm -C editor build");
    expect(closes).toEqual(["closed"]);
  });

  it("connect_rejects_a_welcome_of_another_revision_of_the_protocol", async () => {
    const other = { ...identity, protocol_fingerprint: "0123456789abcdef" };
    const transport = fakeTransport(() => [JSON.stringify({ type: "welcome", ...other, engine_version: "0", game })]);
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    const failure = await ControlClient.connect(transport).catch((error) => error);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure.code).toBe("version_mismatch");
    expect(failure.message).toContain(`fingerprint 0123456789abcdef, this client ${PROTOCOL_FINGERPRINT}`);
    expect(closes).toEqual(["closed"]);
  });

  it("refusal_without_an_error_is_ignored_and_the_handshake_still_settles", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const transport = fakeTransport(() => ['{"type":"refused"}', '{"type":"refused","error":"no"}', welcome]);
    const client = await ControlClient.connect(transport);
    expect(client.game).toEqual(game);
    expect(warn).toHaveBeenCalledTimes(2);
    warn.mockRestore();
  });

  it("sends_hello_with_the_protocol_version_and_exposes_the_game", async () => {
    const hellos: unknown[] = [];
    const transport = fakeTransport((raw) => {
      hellos.push(JSON.parse(raw));
      return [welcome];
    });
    const client = await ControlClient.connect(transport);
    expect(hellos).toEqual([{ type: "hello", ...identity }]);
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
      JSON.stringify({ type: "response", id: 1, result: { error: { code: "no_such_entity", message: "gone" } } }),
    );
    const failure = await pending.catch((error) => error);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure.code).toBe("no_such_entity");
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

  it("frames_of_the_wrong_shape_are_ignored", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "get_hash" });
    for (const frame of ["null", "[]", '{"id":1}', '{"type":"response","id":"1","result":{}}', '{"type":"response","id":1}', '{"type":"event"}']) {
      transport.push(frame);
    }
    transport.push(ok(1, { type: "hash", tick: 1, hash: "1", pending: false }));
    expect((await pending).tick).toBe(1);
    expect(warn).toHaveBeenCalledTimes(6);
    warn.mockRestore();
  });

  it("response_with_an_unknown_id_is_ignored", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "get_hash" });
    transport.push(ok(99, { type: "done" }));
    transport.push(ok(1, { type: "hash", tick: 2, hash: "2", pending: false }));
    expect((await pending).tick).toBe(2);
  });

  it("request_after_the_transport_dropped_rejects", async () => {
    const { transport, sent, client: connecting } = connected();
    const client = await connecting;
    transport.drop("socket reset");
    await expect(client.request({ type: "get_hash" })).rejects.toThrow("connection closed");
    expect(sent).toEqual([]);
  });

  it("topic_subscriptions_are_counted_so_only_the_last_unsubscribe_is_sent", async () => {
    const { transport, sent, client: connecting } = connected();
    const client = await connecting;
    const commands = () => sent.map((request) => request.command);
    const answer = () => transport.push(ok(sent.at(-1)!.id, { type: "done" }));
    const introspection = { type: "introspection" } as const;

    const first = client.request({ type: "subscribe", topic: introspection });
    answer();
    await first;
    await client.request({ type: "subscribe", topic: introspection });
    // Key order does not make another topic.
    const entity = client.request({ type: "subscribe", topic: { entity: { generation: 0, index: 1 }, type: "entity" } });
    answer();
    await entity;
    expect(commands()).toEqual([
      { type: "subscribe", topic: introspection },
      { type: "subscribe", topic: { entity: { generation: 0, index: 1 }, type: "entity" } },
    ]);
    await client.request({ type: "subscribe", topic: { type: "entity", entity: { index: 1, generation: 0 } } });
    expect(commands()).toHaveLength(2);

    expect(await client.request({ type: "unsubscribe", topic: introspection })).toEqual({ type: "done" });
    expect(commands()).toHaveLength(2);
    const last = client.request({ type: "unsubscribe", topic: introspection });
    answer();
    await last;
    expect(commands().at(-1)).toEqual({ type: "unsubscribe", topic: introspection });
    expect(commands()).toHaveLength(3);
  });

  it("refused_subscription_is_not_counted", async () => {
    const { transport, sent, client: connecting } = connected();
    const client = await connecting;
    const topic = { type: "entities", filter: "With<Nope>" } as const;
    const refuse = () =>
      transport.push(JSON.stringify({ type: "response", id: sent.at(-1)!.id, result: { error: { code: "invalid_filter", message: "no component" } } }));

    const first = client.request({ type: "subscribe", topic });
    refuse();
    await expect(first).rejects.toThrow("no component");
    const again = client.request({ type: "subscribe", topic });
    refuse();
    await expect(again).rejects.toThrow("no component");
    expect(sent).toHaveLength(2);
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
