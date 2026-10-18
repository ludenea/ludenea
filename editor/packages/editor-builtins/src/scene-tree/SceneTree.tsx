import type { ControlClient, Entity } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useEffect, useMemo, useState } from "react";
import { EntityList, entityKey } from "../shared/EntityList";
import { shared, useGame, useStore } from "../shared";
import { ResimulatingNotice } from "../timeline/editing";

const LABEL_COMPONENTS = 3;

const shortName = (name: string) => name.split("::").at(-1)!;

export function SceneTree({ editor }: { editor: Editor }) {
  const control = useGame(editor);
  const { build } = useStore(shared(editor).project);
  if (!control) return <p className="ludenea-muted">{build.status === "building" ? "Building the game..." : "No game is running."}</p>;
  return <Entities editor={editor} control={control} />;
}

/** Every entity of the current tick, each labelled by its first components. */
function Entities({ editor, control }: { editor: Editor; control: ControlClient }) {
  // ponytail: labels are cached per entity, one cache per game, so adding or removing a
  // component keeps the old label; refetch on `entity_changed` if that matters.
  const labels = useMemo(() => new Map<string, string>(), [control]);
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="ludenea-scene-tree">
      <ResimulatingNotice editor={editor} />
      {error && <p className="ludenea-error">{error}</p>}
      <EntityList
        editor={editor}
        control={control}
        filter=""
        label="Entities"
        onError={setError}
        row={(row) => (
          <>
            <Label control={control} entity={row.entity} labels={labels} /> <span className="ludenea-muted ludenea-mono">{entityKey(row.entity)}</span>
          </>
        )}
      />
    </div>
  );
}

function Label({ control, entity, labels }: { control: ControlClient; entity: Entity; labels: Map<string, string> }) {
  const key = entityKey(entity);
  const [label, setLabel] = useState(labels.get(key));
  useEffect(() => {
    if (labels.has(key)) return;
    let current = true;
    control.request({ type: "get_entity", entity: { index: entity.index, generation: entity.generation } }).then(
      (reply) => {
        const names = Object.keys(reply.components).slice(0, LABEL_COMPONENTS).map(shortName);
        labels.set(key, names.join(", ") || "(no components)");
        if (current) setLabel(labels.get(key));
      },
      (error) => console.warn(`entity ${key}: ${String(error)}`),
    );
    return () => {
      current = false;
    };
  }, [control, key, labels, entity.index, entity.generation]);
  return <span>{label ?? ""}</span>;
}
