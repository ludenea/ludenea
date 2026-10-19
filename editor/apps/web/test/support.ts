import { PROJECT_PROTOCOL_FINGERPRINT, PROJECT_PROTOCOL_VERSION, ProjectClient, fakeTransport } from "@ludenea/control-client";

type Request = { type: string; [key: string]: unknown };

/** Where the project keeps the stored session of `document`. */
export const sessionFile = (document: string) => `.ludenea/sessions/${document}.session`;

/** `saved` as the editor stores it: `{ version: 1, saved }`, base64-encoded. */
export const storedBytes = (saved: object) => btoa(JSON.stringify({ version: 1, saved }));

/**
 * A project service with the scenes `scenes/a.scene.ron` and `scenes/b.scene.ron`, which
 * stores sessions and sets them aside as the real one does, and answers anything else with
 * `answer` (`Done` by default). It records every request; `push` sends an event.
 */
export async function documentProject(answer: (request: Request) => object = () => ({ type: "done" })) {
  const files: { [path: string]: string } = { "scenes/a.scene.ron": "(A)", "scenes/b.scene.ron": "(B)" };
  const entries = Object.keys(files).map((path, index) => ({ path, id: String(index + 1).padStart(32, "0"), kind: "Scene" }));
  const requests: Request[] = [];
  // The files of `.ludenea/sessions/`, by path.
  const sessions = new Map<string, string>();
  const reply = (request: Request): object => {
    const file = sessionFile(request.document as string);
    switch (request.type) {
      case "list_files":
        return { type: "files", entries };
      case "read_text":
        return { type: "text", text: files[request.path as string] };
      case "read_asset_texts":
        return { type: "asset_texts", assets: [], paths: [] };
      case "load_stored_session":
        return { type: "stored_session", bytes: sessions.get(file) ?? null };
      case "store_session":
        sessions.set(file, request.bytes as string);
        return { type: "done" };
      case "set_aside_stored_session": {
        const bytes = sessions.get(file);
        if (bytes === undefined) return { type: "set_aside", path: null };
        let n = 1;
        while (sessions.has(`.ludenea/sessions/${request.document}.${n}.rejected.session`)) n++;
        const path = `.ludenea/sessions/${request.document}.${n}.rejected.session`;
        sessions.set(path, bytes);
        sessions.delete(file);
        return { type: "set_aside", path };
      }
      default:
        return answer(request);
    }
  };
  const transport = fakeTransport((raw) => {
    const message = JSON.parse(raw);
    if (message.type === "hello") return [JSON.stringify({ type: "welcome", protocol_version: PROJECT_PROTOCOL_VERSION, protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT })];
    requests.push(message.request);
    return [JSON.stringify({ type: "response", id: message.id, result: { ok: reply(message.request) } })];
  });
  const project = await ProjectClient.connect(transport);
  return {
    project,
    ids: entries.map(({ id }) => id) as [string, string],
    requests,
    sessions,
    push: (event: object) => transport.push(JSON.stringify({ type: "event", event })),
  };
}
