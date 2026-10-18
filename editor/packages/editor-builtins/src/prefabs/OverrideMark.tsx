import type { Entity, PathSegment } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useState } from "react";
import { errorText, useStore } from "../shared";
import { pathText } from "../inspector/fields/inputs";
import { instanceTarget, type Prefabs } from "./index";

const samePath = (a: PathSegment[], b: PathSegment[]) => JSON.stringify(a) === JSON.stringify(b);
const shortName = (name: string) => name.split("::").at(-1)!;

/**
 * Beside an overridden field of an instance entity: the mark and its "Revert to prefab" and
 * "Apply to prefab" actions. A component holding any override gets them for the whole component.
 */
export function OverrideMark({ entity, component, path, prefabs }: { editor: Editor; entity: Entity; component: string; path: PathSegment[]; prefabs: Prefabs }) {
  const state = useStore(prefabs.state);
  const [error, setError] = useState<string | null>(null);
  const target = instanceTarget(state, entity);
  if (!target) return null;
  const overrides = state.overrides.filter(
    (override) => override.instance === target.instance[0] && override.component === component && override.entity.join() === target.entity.join(),
  );
  const overridden = path.length === 0 ? overrides.length > 0 : overrides.some((override) => samePath(override.path, path));
  if (!overridden) return null;

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
      </span>{" "}
      <button aria-label={`Revert ${label} to prefab`} onClick={() => run(prefabs.revert({ entity, component, path }))}>
        Revert to prefab
      </button>
      <button aria-label={`Apply ${label} to prefab`} onClick={() => run(prefabs.apply({ entity, component, path }))}>
        Apply to prefab
      </button>
      {error && <span className="ludenea-error">{error}</span>}
    </span>
  );
}
