import { describe, expect, it } from "vitest";
import { ProjectClient, fakeTransport } from "../src";

describe("ProjectClient", () => {
  it("resolves_a_request_by_id", async () => {
    const transport = fakeTransport((raw) => {
      const { id } = JSON.parse(raw);
      return [JSON.stringify({ type: "response", id, result: { ok: { type: "done" } } })];
    });
    const client = new ProjectClient(transport);
    expect(await client.request({ type: "anything" })).toEqual({ type: "done" });
  });

  it("rejects_pending_requests_when_the_transport_closes", async () => {
    const transport = fakeTransport(() => []);
    const client = new ProjectClient(transport);
    const pending = client.request({ type: "anything" });
    transport.drop("gone");
    await expect(pending).rejects.toThrow("gone");
  });
});
