import type { ControlClient, Entity, FileEntry, InstanceInfo, PathSegment } from "@ludenea/control-client";
import { definePlugin, type Editor, type Loaded, type TimelineState } from "@ludenea/editor-shell";
import { logToConsole } from "../console";
import { createStore, errorText, showPanel, type Store } from "../shared";
import { entityKey } from "../shared/EntityList";
import { assertEditable, edit } from "../timeline/editing";
import { OverrideMark } from "./OverrideMark";

/** A field override the open scene gives entity `entity` (a path of local identifiers) of its instance `instance`. */
export interface Override {
  instance: number;
  entity: number[];
  component: string;
  path: PathSegment[];
}

export interface PrefabsState {
  /** Every instance of the loaded scene, nested ones included, parents first. */
  instances: InstanceInfo[];
  /** Each loaded entity's load address, by `entityKey`. */
  addresses: Map<string, number[]>;
  overrides: Override[];
  /** The project's prefab files. */
  prefabs: FileEntry[];
  /** The "Place prefab" picker shows. */
  placing: boolean;
  error: string | null;
}

/** A field (empty `path`: the whole component) of a component of an instance entity. */
export interface Target {
  entity: Entity;
  component: string;
  path: PathSegment[];
}

export interface Prefabs {
  state: Store<PrefabsState>;
  place(prefab: string): Promise<void>;
  revert(target: Target): Promise<void>;
  apply(target: Target): Promise<void>;
}

const PREFAB_ROOT = "assets";
const plugins = new WeakMap<Editor, Prefabs>();

/** The prefabs plugin of `editor`, when active: the scene tree shows its instance groups. */
export const prefabsOf = (editor: Editor): Prefabs | undefined => plugins.get(editor);

/** Where `target` lives for the prefab commands: its top-level instance and its path inside it; null outside instances. */
export function instanceTarget(state: PrefabsState, entity: Entity): { instance: number[]; entity: number[] } | null {
  const address = state.addresses.get(entityKey(entity));
  if (!address || address.length < 2) return null;
  return { instance: address.slice(0, 1), entity: address.slice(1) };
}

const segment = (text: string): PathSegment | null => {
  const name = /name\s*:\s*"([^"]*)"/.exec(text);
  const index = /index\s*:\s*(\d+)/.exec(text);
  if (/type\s*:\s*"field"/.test(text) && name) return { type: "field", name: name[1]! };
  if (/type\s*:\s*"index"/.test(text) && index) return { type: "index", index: Number(index[1]) };
  return null;
};

const INSTANCE = /\(\s*id\s*:\s*(\d+)\s*,\s*prefab\s*:/g;
const OVERRIDE = /\(\s*entity\s*:\s*\[([\d\s,]*)\]\s*,\s*component\s*:\s*"([^"]*)"\s*,\s*path\s*:\s*\[([^\]]*)\]/g;

/**
 * The field overrides of a scene or prefab file's instances, read from its text: the host
 * reports none. Added and removed components have no path and are not overrides.
 */
// ponytail: pattern matching on the host's own layout of `instances` (written last, entries as
// `(id: N, prefab: ...)`); a component field named `instances` after them would confuse it.
export function overridesIn(file: string): Override[] {
  const start = [...file.matchAll(/\binstances\s*:\s*\[/g)].at(-1)?.index;
  if (start === undefined) return [];
  const text = file.slice(start);
  const starts = [...text.matchAll(INSTANCE)];
  return starts.flatMap((match, position) => {
    const entry = text.slice(match.index, starts[position + 1]?.index);
    return [...entry.matchAll(OVERRIDE)].map(([, entity, component, path]) => ({
      instance: Number(match[1]),
      entity: entity!.split(",").filter((id) => id.trim() !== "").map(Number),
      component: component!,
      path: [...path!.matchAll(/\(([^()]*)\)/g)].map(([, inner]) => segment(inner!)).filter((part) => part !== null),
    }));
  });
}

const addressMap = (addresses: Loaded["load_addresses"]) => new Map(addresses.map(([entity, address]) => [entityKey(entity), address]));

/** `assets/new.prefab.ron`, or the first `assets/new-N.prefab.ron` no file has. */
function freePath(files: FileEntry[]): string {
  const taken = new Set(files.map((file) => file.path));
  let path = `${PREFAB_ROOT}/new.prefab.ron`;
  for (let number = 2; taken.has(path); number++) path = `${PREFAB_ROOT}/new-${number}.prefab.ron`;
  return path;
}

export const prefabsPlugin = definePlugin({
  name: "ludenea.prefabs",
  activate(editor) {
    const state = createStore<PrefabsState>({ instances: [], addresses: new Map(), overrides: [], prefabs: [], placing: false, error: null });
    const game = (): ControlClient => {
      if (!editor.control) throw new Error("no game is running");
      return editor.control;
    };
    const report = (error: unknown) => state.set({ error: errorText(error) });
    const clear = () => state.set({ instances: [], addresses: new Map(), overrides: [] });
    const editing = () => {
      const kind = editor.document.getState().current?.kind;
      return kind === "scene" || kind === "prefab";
    };

    const listPrefabs = () =>
      editor.project.request({ type: "list_files" }).then(
        ({ entries }) => state.set({ prefabs: entries.filter((file) => file.kind === "Prefab") }),
        report,
      );
    // Overrides live in the session's scene text, unsaved edits included.
    const readOverrides = async (control: ControlClient) => {
      const { scene } = await control.request({ type: "export_session" });
      state.set({ overrides: overridesIn(scene), error: null });
    };
    /** Reads the instances, load addresses, and overrides of the scene as the session holds it now. */
    const refresh = async () => {
      const control = editor.control;
      if (!control || !editing()) return clear();
      const [{ instances, load_addresses }] = await Promise.all([control.request({ type: "get_instances" }), readOverrides(control)]);
      state.set({ instances, addresses: addressMap(load_addresses) });
    };
    const onLoaded = (loaded: Loaded | null) => {
      if (!loaded) return clear();
      for (const warning of loaded.warnings) logToConsole(editor, "warning", warning);
      state.set({ instances: loaded.instances, addresses: addressMap(loaded.load_addresses) });
      void listPrefabs();
      if (editor.control) readOverrides(editor.control).catch(report);
    };
    const targetOf = ({ entity }: Target) => {
      const target = instanceTarget(state.get(), entity);
      if (!target) throw new Error(`entity ${entityKey(entity)} is not in a prefab instance`);
      return target;
    };

    const prefabs: Prefabs = {
      state,
      async place(prefab) {
        await edit(editor, game(), { type: "place_instance", prefab });
        state.set({ placing: false });
        await refresh();
      },
      async revert(target) {
        await edit(editor, game(), { type: "revert_override", ...targetOf(target), component: target.component, path: target.path });
        await refresh();
      },
      async apply(target) {
        const located = targetOf(target);
        const prefab = state.get().instances.find((instance) => instance.address.join() === located.instance.join())?.prefab;
        const { entries } = await editor.project.request({ type: "list_files" });
        const path = entries.find((file) => file.id === prefab)?.path;
        if (!path) throw new Error(`the prefab ${prefab ?? "of the instance"} has no file in the project`);
        const { text } = await game().request({ type: "apply_to_prefab", ...located, component: target.component, path: target.path });
        // The change on disk reloads the open scene with the new prefab.
        await editor.project.request({ type: "write_text", path, text });
      },
    };
    plugins.set(editor, prefabs);

    editor.commands.register({
      id: "prefabs.place",
      title: "Place prefab",
      run() {
        game();
        assertEditable(editor);
        state.set({ placing: true, error: null });
        void listPrefabs();
        showPanel(editor, "ludenea.scene-tree");
      },
    });
    editor.commands.register({
      id: "prefabs.create-from-selection",
      title: "Create prefab from selection",
      async run() {
        const control = game();
        const { entities } = editor.selection.getState();
        if (entities.length === 0) throw new Error("select the entities of the new prefab first");
        assertEditable(editor);
        const { text } = await control.request({ type: "create_prefab", entities });
        const { entries } = await editor.project.request({ type: "list_files" });
        const path = freePath(entries);
        const created = await editor.project.request({ type: "create_asset", path, kind: "Prefab", text });
        try {
          await edit(editor, control, { type: "replace_with_instance", entities, prefab: created.id, prefab_text: text });
        } catch (error) {
          // Refused: the new file is no one's prefab.
          await editor.project.request({ type: "delete", path }).catch(() => {});
          throw error;
        }
        editor.selection.getState().clear();
        await refresh();
      },
    });
    editor.commands.register({
      id: "prefabs.open",
      title: "Open the selected instance's prefab",
      async run() {
        const entity = editor.selection.getState().entities[0];
        if (!entity) throw new Error("select an entity of a prefab instance first");
        // The innermost instance holding it.
        const instance = [...state.get().instances].reverse().find((candidate) => candidate.entities.some((other) => entityKey(other) === entityKey(entity)));
        if (!instance) throw new Error(`entity ${entityKey(entity)} is not in a prefab instance`);
        await editor.document.getState().open(instance.prefab);
      },
    });
    editor.inspectors.decorate({ id: "ludenea.prefabs.overrides", render: (props) => <OverrideMark {...props} prefabs={prefabs} /> });

    // ponytail: refreshes when an edit, undo, redo, or branch switch changes the timeline's
    // labels; a scene change that leaves them as they were is read on the next load.
    const editsOf = (timeline: TimelineState["timeline"]) =>
      timeline && [timeline.undo_label, timeline.redo_label, timeline.scene_dirty, timeline.branch].join("|");
    const stops = [
      editor.document.subscribe((current, previous) => {
        if (current.loaded !== previous.loaded) onLoaded(current.loaded);
      }),
      editor.timeline.subscribe((current, previous) => {
        if (editsOf(current.timeline) !== editsOf(previous.timeline)) refresh().catch(report);
      }),
    ];
    editor.onGameChanged((control) => {
      if (!control) clear();
    });
    return () => {
      plugins.delete(editor);
      stops.forEach((stop) => stop());
    };
  },
});
