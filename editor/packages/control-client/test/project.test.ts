import { describe, expect, it } from "vitest";
import { ProjectClient, ProtocolFailure, fakeTransport } from "../src";

const welcome = JSON.stringify({ type: "welcome", protocol_version: 1 });

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
    expect(hellos).toEqual([{ type: "hello", protocol_version: 1 }]);
  });

  it("connect_rejects_a_refusal_with_its_message", async () => {
    const transport = fakeTransport(() => [
      JSON.stringify({ type: "refused", error: { code: "version_mismatch", message: "service speaks 1" } }),
    ]);
    await expect(ProjectClient.connect(transport)).rejects.toThrow("service speaks 1");
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
      JSON.stringify({ type: "event", event: { type: "files_changed", changes: [{ Created: "scenes/a.scene.ron" }] } }),
    );
    stop();
    transport.push(JSON.stringify({ type: "event", event: { type: "files_changed", changes: [] } }));
    expect(changes).toEqual([[{ Created: "scenes/a.scene.ron" }]]);
  });

  it("rejects_pending_requests_when_the_transport_closes", async () => {
    const { transport, client: connecting } = connected();
    const client = await connecting;
    const pending = client.request({ type: "list_files" });
    transport.drop("gone");
    await expect(pending).rejects.toThrow("gone");
  });
});
