import type { Entity, PathSegment } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useState } from "react";
import { errorText, useStore } from "../shared";
import { pathText } from "../inspector/fields/inputs";
import { overridesOf, type Prefabs } from "./index";

const segmentText = (segment: PathSegment) => JSON.stringify(segment);
const startsWith = (path: PathSegment[], prefix: PathSegment[]) => prefix.length <= path.length && prefix.every((segment, at) => segmentText(segment) === segmentText(path[at]!));
const shortName = (name: string) => name.split("::").at(-1)!;

/**
 * Beside an overridden field of an instance entity: the mark, and the "Revert to prefab" and
 * "Apply to prefab" actions where an override targets exactly that field. A field under an
 * overridden parent (or a whole-component override) is marked only. A component holding any
 * override gets the actions for the whole component.
 */
export function OverrideMark({ entity, component, path, prefabs }: { editor: Editor; entity: Entity; component: string; path: PathSegment[]; prefabs: Prefabs }) {
  const state = useStore(prefabs.state);
  const [error, setError] = useState<string | null>(null);
  const overrides = overridesOf(state, entity, component);
  const exact = path.length === 0 ? overrides.length > 0 : overrides.some((override) => override.path.length === path.length && startsWith(path, override.path));
  const covered = overrides.some((override) => startsWith(path, override.path));
  if (!exact && !covered) return null;

  const label = path.length === 0 ? shortName(component) : pathText(path);
  const run = (action: Promise<void>) =>
    action.then(
      () => setError(null),
      (failure) => setError(errorText(failure)),
    );
  return (
    <span className="ludenea-override">
      <span aria-label={`${label} overridden`} title="Overridden in this instance">
        overridden
      </span>
      {exact && (
        <>
          {" "}
          <button aria-label={`Revert ${label} to prefab`} onClick={() => run(prefabs.revert({ entity, component, path }))}>
            Revert to prefab
          </button>
          <button aria-label={`Apply ${label} to prefab`} onClick={() => run(prefabs.apply({ entity, component, path }))}>
            Apply to prefab
          </button>
        </>
      )}
      {error && <span className="ludenea-error">{error}</span>}
    </span>
  );
}
