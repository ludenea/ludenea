import type { InstanceInfo } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useState, type MouseEvent } from "react";
import { createStore, errorText, useStore, useUnsavedAsset } from "../shared";
import { entityKey } from "../shared/EntityList";
import { prefabsOf, type Prefabs } from "./index";

const fileName = (path: string) => path.split("/").at(-1)!;
// Without the prefabs plugin there is nothing to show.
const inactive = createStore({ instances: [] as InstanceInfo[], placing: false, error: null as string | null });

/**
 * The scene's prefab instances as collapsible groups named after their prefab file, with a save
 * button while the session holds unsaved changes of the prefab, and the "Place prefab" picker.
 */
export function InstanceGroups({ editor }: { editor: Editor }) {
  const prefabs = prefabsOf(editor);
  const { instances, placing, error } = useStore<{ instances: InstanceInfo[]; placing: boolean; error: string | null }>(prefabs?.state ?? inactive);
  if (!prefabs) return null;
  const top = instances.filter((instance) => instance.address.length === 1);
  return (
    <div className="ludenea-instances">
      {error && <p className="ludenea-error">{error}</p>}
      {placing && <PlacePicker prefabs={prefabs} />}
      {top.map((instance) => (
        <Group key={instance.address.join("/")} editor={editor} prefabs={prefabs} instance={instance} />
      ))}
    </div>
  );
}

function Group({ editor, prefabs, instance }: { editor: Editor; prefabs: Prefabs; instance: InstanceInfo }) {
  const { instances, prefabs: files } = useStore(prefabs.state);
  const name = fileName(files.find((file) => file.id === instance.prefab)?.path ?? instance.prefab);
  const unsaved = useUnsavedAsset(editor, instance.prefab);
  const depth = instance.address.length;
  const inside = (other: InstanceInfo) => other.address.length > depth && other.address.slice(0, depth).join() === instance.address.join();
  const nested = instances.filter((other) => other.address.length === depth + 1 && inside(other));
  const nestedEntities = new Set(instances.filter(inside).flatMap((other) => other.entities.map(entityKey)));
  const own = instance.entities.filter((entity) => !nestedEntities.has(entityKey(entity)));
  const choose = (entity: InstanceInfo["entities"][number], event: MouseEvent) => {
    const selection = editor.selection.getState();
    if (event.shiftKey || event.ctrlKey || event.metaKey) selection.add(entity);
    else selection.select([entity]);
  };
  const save = () =>
    editor.commands.run("inspector.save-asset", instance.prefab).then(
      () => prefabs.state.set({ error: null }),
      (failure: unknown) => prefabs.state.set({ error: errorText(failure) }),
    );
  // The summary toggles the group, so the save button is in the group, not inside it.
  return (
    <details open aria-label={name}>
      <summary title={instance.prefab}>{name}</summary>
      {unsaved && (
        <div>
          <span aria-label="unsaved changes">●</span>{" "}
          <button aria-label={`Save prefab ${name}`} onClick={save}>
            Save prefab
          </button>
        </div>
      )}
      {own.map((entity) => (
        <button key={entityKey(entity)} className="ludenea-link ludenea-mono" onClick={(event) => choose(entity, event)}>
          {entityKey(entity)}
        </button>
      ))}
      {nested.map((child) => (
        <Group key={child.address.join("/")} editor={editor} prefabs={prefabs} instance={child} />
      ))}
    </details>
  );
}

function PlacePicker({ prefabs }: { prefabs: Prefabs }) {
  const { prefabs: files } = useStore(prefabs.state);
  const [chosen, setChosen] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const prefab = chosen ?? files[0]?.id ?? "";
  return (
    <form className="ludenea-place" onSubmit={(event) => event.preventDefault()}>
      <select aria-label="Prefab" value={prefab} onChange={(event) => setChosen(event.target.value)}>
        {files.map((file) => (
          <option key={file.id} value={file.id}>
            {file.path}
          </option>
        ))}
      </select>
      <button disabled={!prefab} onClick={() => void prefabs.place(prefab).catch((failure) => setError(errorText(failure)))}>
        Place
      </button>
      <button onClick={() => prefabs.state.set({ placing: false })}>Cancel</button>
      {error && <p className="ludenea-error">{error}</p>}
    </form>
  );
}
