import type { ControlClient, EntityRow } from "@ludenea/control-client";
import { isTextField, type Editor } from "@ludenea/editor-shell";
import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { InstanceGroups } from "../prefabs/InstanceGroups";
import { EntityList, entityKey } from "../shared/EntityList";
import { errorText, shared, useGame, useStore } from "../shared";
import { ResimulatingNotice } from "../timeline/editing";

const LABEL_COMPONENTS = 3;

const shortName = (name: string) => name.split("::").at(-1)!;

export function SceneTree({ editor, scroll }: { editor: Editor; scroll: { top: number } }) {
  const control = useGame(editor);
  const { build } = useStore(shared(editor).project);
  if (!control) return <p className="ludenea-muted">{build.status === "building" ? "Building the game..." : "No game is running."}</p>;
  return <Entities editor={editor} control={control} scroll={scroll} />;
}

/** Every entity of the current tick, each labelled by its first components. */
function Entities({ editor, control, scroll }: { editor: Editor; control: ControlClient; scroll: { top: number } }) {
  const labels = useMemo(() => new Map<string, Cached>(), [control]);
  const playing = editor.timeline((state) => state.playing);
  const [error, setError] = useState<string | null>(null);
  // A refused scene load (a missing prefab): the previous session stays, and so does this list,
  // until a load succeeds or the user dismisses it.
  const loadError = editor.document((state) => state.error);
  // Delete despawns here and in the viewport only: elsewhere (the asset browser) it means something else.
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Delete" || isTextField(event.target)) return;
    event.preventDefault();
    editor.commands.run("scene.despawn-selection").then(
      () => setError(null),
      (failure) => setError(errorText(failure)),
    );
  };
  return (
    <div className="ludenea-scene-tree" onKeyDown={onKeyDown}>
      <ResimulatingNotice editor={editor} />
      {loadError && (
        <p role="alert" className="ludenea-error">
          {loadError} <button onClick={() => editor.document.setState({ error: null })}>Dismiss</button>
        </p>
      )}
      {error && <p className="ludenea-error">{error}</p>}
      <InstanceGroups editor={editor} />
      <EntityList
        editor={editor}
        control={control}
        filter=""
        label="Entities"
        onError={setError}
        scroll={scroll}
        row={(row) => (
          <>
            <Label editor={editor} control={control} row={row} labels={labels} playing={playing} /> <span className="ludenea-muted ludenea-mono">{entityKey(row.entity)}</span>
          </>
        )}
      />
    </div>
  );
}

/** A label and the entity hash it was read at. */
type Cached = { hash: string; label: string };

function Label({ editor, control, row, labels, playing }: { editor: Editor; control: ControlClient; row: EntityRow; labels: Map<string, Cached>; playing: boolean }) {
  const { entity, hash } = row;
  const key = entityKey(entity);
  const [, setRead] = useState(0);
  // The hash at which the last read failed, if it did.
  const [failedAt, setFailedAt] = useState<string | null>(null);
  const cached = labels.get(key);
  // A component added or removed changes the hash. So does every tick of a moving entity:
  // while playing the label is read once, and read again once paused.
  // What the read is keyed on: the first read whatever the hash (one in flight is not sent again
  // as ticks change it), then the hash shown once paused. After a failed read, the next hash
  // shown while paused.
  const unread = failedAt === null ? "first" : !playing && failedAt !== hash ? hash : null;
  const wanted = !cached ? unread : !playing && cached.hash !== hash ? hash : null;
  useEffect(() => {
    if (wanted === null) return;
    let current = true;
    control.request({ type: "get_entity", entity: { index: entity.index, generation: entity.generation } }).then(
      (reply) => {
        const names = Object.keys(reply.components).slice(0, LABEL_COMPONENTS).map(shortName);
        labels.set(key, { hash, label: names.join(", ") || "(no components)" });
        if (current) setRead((read) => read + 1);
      },
      (error) => {
        editor.console.log("warning", `Cannot read the components of entity ${key} for its label: ${errorText(error)}`);
        if (current) setFailedAt(hash);
      },
    );
    return () => {
      current = false;
    };
    // `hash` is read through `wanted`: a change while playing must not send the read again.
  }, [editor, control, key, wanted, labels, entity.index, entity.generation]);
  return <span>{cached?.label ?? ""}</span>;
}
