import { describe, expect, it, vi } from "vitest";
import { PROJECT_PROTOCOL_FINGERPRINT, PROJECT_PROTOCOL_VERSION, ProjectClient, ProtocolFailure, fakeTransport } from "../src";

const identity = { protocol_version: PROJECT_PROTOCOL_VERSION, protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT };
const welcome = JSON.stringify({ type: "welcome", ...identity });

/** A transport whose peer welcomes the client and records every request it receives. */
function connected() {
  const sent: { id: number; request: { type: string } }[] = [];
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [welcome];
    sent.push(message);
    return [];
  });
  return { transport, sent, client: ProjectClient.connect(transport) };
}

describe("ProjectClient", () => {
  it("sends_hello_with_the_project_protocol_version", async () => {
    const hellos: unknown[] = [];
    const transport = fakeTransport((raw) => {
      hellos.push(JSON.parse(raw));
      return [welcome];
    });
    await ProjectClient.connect(transport);
    expect(hellos).toEqual([{ type: "hello", ...identity }]);
  });

  it("connect_rejects_a_welcome_of_another_protocol_version", async () => {
    const transport = fakeTransport(() => [JSON.stringify({ type: "welcome", ...identity, protocol_version: PROJECT_PROTOCOL_VERSION - 1 })]);
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    const failure = await ProjectClient.connect(transport).catch((error) => error);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure.code).toBe("version_mismatch");
    expect(failure.message).toContain("pnpm -C editor build");
    expect(closes).toEqual(["closed"]);
  });

  it("connect_rejects_a_refusal_with_its_message", async () => {
    const transport = fakeTransport(() => [
      JSON.stringify({ type: "refused", error: { code: "version_mismatch", message: "service speaks 1" } }),
    ]);
    await expect(ProjectClient.connect(transport)).rejects.toThrow("service speaks 1");
  });

  it("refused_handshake_closes_the_transport_and_rejects_with_the_code", async () => {
    const transport = fakeTransport(() => [
      JSON.stringify({ type: "refused", error: { code: "version_mismatch", message: "service speaks 1" } }),
    ]);
    const closes: string[] = [];
    transport.onClose((reason) => closes.push(reason));
    const failure = await ProjectClient.connect(transport).catch((error) => error);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure.code).toBe("version_mismatch");
    expect(closes).toEqual(["closed"]);
  });

  it("malformed_frames_are_ignored_and_do_not_break_pending_requests", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "list_files" });
    transport.push("{not json");
    transport.push('{"type":"response","result":{"ok":{"type":"done"}}}');
    transport.push('{"type":"event","event":null}');
    transport.push(JSON.stringify({ type: "response", id: 1, result: { ok: { type: "files", entries: [] } } }));
    expect((await pending).entries).toEqual([]);
    expect(warn).toHaveBeenCalledTimes(3);
    warn.mockRestore();
  });

  it("resolves_a_request_by_id_with_its_typed_reply", async () => {
    const { transport, sent, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "read_text", path: "scenes/main.scene.ron" });
    expect(sent).toEqual([{ type: "request", id: 1, request: { type: "read_text", path: "scenes/main.scene.ron" } }]);
    transport.push(JSON.stringify({ type: "response", id: 1, result: { ok: { type: "text", text: "()" } } }));
    expect((await pending).text).toBe("()");
  });

  it("error_result_rejects_with_code", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "read_text", path: "../secret" });
    transport.push(
      JSON.stringify({ type: "response", id: 1, result: { error: { code: "outside_project", message: "outside" } } }),
    );
    const failure = await pending.catch((error) => error);
    expect(failure).toBeInstanceOf(ProtocolFailure);
    expect(failure.code).toBe("outside_project");
  });

  it("dispatches_events_to_listeners_of_their_type", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const changes: unknown[] = [];
    const stop = client.on("files_changed", (event) => changes.push(event.changes));
    transport.push(JSON.stringify({ type: "event", event: { type: "game_ready" } }));
    transport.push(
      JSON.stringify({ type: "event", event: { type: "files_changed", changes: [{ Created: { path: "scenes/a.scene.ron", kind: "Scene" } }] } }),
    );
    stop();
    transport.push(JSON.stringify({ type: "event", event: { type: "files_changed", changes: [] } }));
    expect(changes).toEqual([[{ Created: { path: "scenes/a.scene.ron", kind: "Scene" } }]]);
  });

  it("a_refused_message_fails_the_pending_requests_with_the_service_reason", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "store_session", document: "1", bytes: "" });
    const reason = "the project service refused a message of at least 300 bytes: it takes up to 256 bytes";
    // The service answers a message it cannot read whole with a refusal, then closes.
    transport.push(JSON.stringify({ type: "refused", error: { code: "invalid_request", message: reason } }));
    transport.drop("message of at least 300 bytes, over the limit of 256 bytes");
    await expect(pending).rejects.toThrow(reason);
  });

  it("rejects_pending_requests_when_the_transport_closes", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "list_files" });
    transport.drop("gone");
    await expect(pending).rejects.toThrow("gone");
  });
});
