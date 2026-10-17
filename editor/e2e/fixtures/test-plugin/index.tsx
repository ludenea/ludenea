// A project plugin using only its `editor` argument and the shared modules, like a built-in.
import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { useEffect, useState } from "react";

function BodyPanel({ editor }: { editor: Editor }) {
  const entity = editor.selection((selection) => selection.entities[0]);
  const [kind, setKind] = useState<string | null>(null);
  useEffect(() => {
    if (!entity) return;
    let current = true;
    editor.entities.get(entity, "ludenea::Body").then(
      (body) => current && setKind(body ? String(body.kind) : "no body"),
      (error) => current && setKind(String(error)),
    );
    return () => {
      current = false;
    };
  }, [editor, entity]);
  return <p>{entity ? `Body read by the plugin: ${kind ?? "..."}` : "Select an entity."}</p>;
}

export default definePlugin({
  name: "test_plugin",
  activate(editor) {
    editor.panels.register({ id: "test_plugin.panel", title: "Test plugin", component: BodyPanel });
    editor.inspectors.register("ludenea::Body", ({ value }) => <p>Custom body inspector: {String(value.kind)}</p>);
  },
});
