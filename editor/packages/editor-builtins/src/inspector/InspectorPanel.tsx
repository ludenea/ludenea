import { ProtocolFailure, type AssetId, type ComponentValue, type ControlClient, type Entity, type FieldTarget, type JsonValue, type PathSegment, type Timeline, type TypeSchema } from "@ludenea/control-client";
import { PluginBoundary, type Editor, type PluginFailure } from "@ludenea/editor-shell";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { errorText, useGame, useStore, useUnsavedAsset } from "../shared";
import { edit, ResimulatingNotice, useResimulating } from "../timeline/editing";
import { defaultValue, setAt } from "./default-value";
import { Field } from "./fields/Field";
import type { Edit } from "./fields/inputs";
import { onEntity, type AddDraft, type InspectorState } from "./index";

type Values = { [name: string]: ComponentValue };

// The timeline drives the time; editing it would fight the timeline.
const READ_ONLY_RESOURCES = new Set(["ludenea::Time"]);

const shortName = (name: string) => name.split("::").at(-1)!;
const schemaMap = (named: { name: string; schema: TypeSchema }[]) => new Map(named.map(({ name, schema }) => [name, schema]));

export function InspectorPanel({ editor, state }: { editor: Editor; state: InspectorState }) {
  const control = useGame(editor);
  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  return <Inspector editor={editor} control={control} state={state} />;
}

function Inspector({ editor, control, state }: { editor: Editor; control: ControlClient; state: InspectorState }) {
  const entity = editor.selection((selection) => selection.entities[0]);
  const asset = editor.selection((selection) => selection.asset);
  const { adding, focused, expanded } = useStore(state);
  const components = useMemo(() => schemaMap(control.game.components), [control]);
  const resourceSchemas = useMemo(() => schemaMap(control.game.resources), [control]);
  const values = useEntity(editor, control, entity);
  const resimulating = useResimulating(editor);

  const setField = (target: FieldTarget, refresh: () => void): Edit => async (path, value) => {
    await onEntity(editor, edit(editor, control, { type: "set_field", target, path, value }));
    refresh();
  };

  return (
    <div className="ludenea-inspector">
      <ResimulatingNotice editor={editor} />
      {asset && <DataAsset editor={editor} control={control} id={asset} />}
      {entity && (
        <>
          <h2>
            Entity {entity.index}v{entity.generation}{" "}
            <button disabled={resimulating} onClick={() => void editor.commands.run("inspector.add-component").catch((error) => values.setError(errorText(error)))}>Add component</button>
          </h2>
          {values.error && <p className="ludenea-error">{values.error}</p>}
          {adding && <AddComponent editor={editor} control={control} entity={entity} schemas={components} present={values.components ?? {}} state={state} onAdded={values.refresh} />}
          {Object.entries(values.components ?? {}).map(([name, value]) => (
            <Section
              key={name}
              editor={editor}
              name={name}
              value={value}
              schema={components.get(name)}
              entity={entity}
              edit={setField({ type: "component", entity, component: name }, values.refresh)}
              state={state}
              focused={focused === name}
              readOnly={resimulating}
            />
          ))}
        </>
      )}
      <h2>Resources</h2>
      {[...resourceSchemas].map(([name, schema]) => (
        <Resource
          key={name}
          editor={editor}
          control={control}
          name={name}
          schema={schema}
          expanded={expanded.includes(name)}
          toggle={() => {
            const others = state.get().expanded.filter((other) => other !== name);
            state.set({ expanded: expanded.includes(name) ? others : [...others, name] });
          }}
          // The game pushes the edited value.
          edit={setField({ type: "resource", resource: name }, () => {})}
          readOnly={resimulating || READ_ONLY_RESOURCES.has(name)}
        />
      ))}
    </div>
  );
}

/**
 * The selected data asset, edited through its schema. At tick 0 an edit changes the asset's
 * text, to save explicitly; later it is a recorded edit, which "Apply to asset" copies back.
 */
function DataAsset({ editor, control, id }: { editor: Editor; control: ControlClient; id: AssetId }) {
  const tick = editor.timeline((timeline) => timeline.timeline?.current ?? 0);
  const changes = editor.timeline(({ timeline }) => assetChanges(timeline));
  const reloaded = editor.document((document) => document.loaded);
  const unsaved = useUnsavedAsset(editor, id);
  const resimulating = useResimulating(editor);
  const [asset, setAsset] = useState<{ asset: string; value: ComponentValue } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);

  // What may change the value: an edit, undo or redo, a branch switch, a seek across an edit
  // tick, or a reload of the session; not every tick played.
  useEffect(() => {
    let current = true;
    control.request({ type: "get_asset", id }).then(
      (reply) => {
        if (!current) return;
        setAsset(reply);
        setError(null);
      },
      (failure) => current && setError(errorText(failure)),
    );
    return () => {
      current = false;
    };
  }, [control, id, changes, reloaded, version]);

  const setField: Edit = async (path, value) => {
    await edit(editor, control, { type: "set_asset_field", id, path, value });
    setVersion((version) => version + 1);
  };
  const run = (command: string) => () =>
    void editor.commands.run(command).then(
      () => setError(null),
      (failure) => setError(errorText(failure)),
    );
  const schema = asset && control.game.assets.find(({ name }) => name === asset.asset)?.schema;

  return (
    <>
      <h2>
        Data asset {asset && shortName(asset.asset)}
        {unsaved && <span aria-label="unsaved changes"> ●</span>}{" "}
        {unsaved && <button onClick={run("inspector.save-asset")}>Save asset</button>}
        {tick > 0 && (
          <button disabled={resimulating} onClick={run("inspector.apply-to-asset")}>
            Apply to asset
          </button>
        )}
      </h2>
      {error && <p className="ludenea-error">{error}</p>}
      {asset && <Section editor={editor} name={asset.asset} value={asset.value} schema={schema ?? undefined} edit={setField} readOnly={resimulating} />}
    </>
  );
}

/**
 * A text that changes with what the timeline reports that can change a data asset's value at
 * the current tick: the edits before it (an edit applies at the start of the next tick, so
 * both sides of each edit tick count), undo and redo, and the branch.
 */
function assetChanges(timeline: Timeline | null): string {
  if (!timeline) return "";
  const { current, edit_ticks, undo_label, redo_label, branch } = timeline;
  const before = edit_ticks.filter((tick) => tick < current).length;
  const upTo = edit_ticks.filter((tick) => tick <= current).length;
  return JSON.stringify([current === 0, before, upTo, edit_ticks, undo_label, redo_label, branch]);
}

/** The selected entity's components, reloaded when the host reports a change. */
function useEntity(editor: Editor, control: ControlClient, entity: Entity | undefined) {
  const [components, setComponents] = useState<Values | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  const index = entity?.index;
  const generation = entity?.generation;

  useEffect(() => {
    setComponents(null);
    setError(null);
  }, [index, generation]);
  // Reloading keeps the shown values (and the focused field) until the new ones arrive.
  useEffect(() => {
    if (index === undefined || generation === undefined) return;
    const entity = { index, generation };
    let current = true;
    const load = () =>
      onEntity(editor, control.request({ type: "get_entity", entity })).then(
        (reply) => current && setComponents(reply.components),
        (failure) => current && setError(errorText(failure)),
      );
    load();
    const watch = editor.entities.watch(entity, load);
    return () => {
      current = false;
      watch.dispose();
    };
  }, [editor, control, index, generation, version]);

  return { components, error, setError, refresh: () => setVersion((version) => version + 1) };
}

/**
 * A resource, collapsed until opened. Only an open one follows the game, through its own
 * `Resource` topic: the game pushes it when its value changes, at a capped rate while playing,
 * so a large resource such as the contact cache costs nothing until shown.
 */
function Resource(props: { editor: Editor; control: ControlClient; name: string; schema: TypeSchema; expanded: boolean; toggle(): void; edit: Edit; readOnly: boolean }) {
  const { editor, control, name, schema, expanded, toggle, edit, readOnly } = props;
  const [value, setValue] = useState<ComponentValue | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setValue(null);
    setError(null);
    if (!expanded) return;
    const topic = { type: "resource", name } as const;
    const stop = control.on("resources_changed", ({ resources }) => {
      const next = resources[name];
      if (next) setValue(next);
    });
    control.request({ type: "subscribe", topic }).catch((failure) => setError(errorText(failure)));
    return () => {
      stop();
      control.request({ type: "unsubscribe", topic }).catch(() => {});
    };
  }, [control, name, expanded]);

  const title = (
    <button className="ludenea-resource-toggle" aria-expanded={expanded} onClick={toggle} title={name}>
      {shortName(name)}
    </button>
  );
  if (!expanded || !value) {
    return (
      <section aria-label={name} className="ludenea-component">
        <h3>{title}</h3>
        {error && <p className="ludenea-error">{error}</p>}
      </section>
    );
  }
  return <Section editor={editor} name={name} title={title} value={value} schema={schema} edit={edit} readOnly={readOnly} />;
}

interface SectionProps {
  editor: Editor;
  name: string;
  value: ComponentValue;
  schema: TypeSchema | undefined;
  edit: Edit;
  /** Set for a component; a resource has none. */
  entity?: Entity;
  /** What the heading shows instead of the short name. */
  title?: ReactNode;
  state?: InspectorState;
  focused?: boolean;
  readOnly?: boolean;
}

/**
 * One component or resource: its custom inspector, its schema-driven fields, or read-only text.
 * A custom inspector that throws gives way to the schema-driven fields.
 */
function Section({ editor, name, title, value, schema, edit, entity, state, focused = false, readOnly = false }: SectionProps) {
  const [error, setError] = useState<string | null>(null);
  const Custom = entity ? editor.inspectors.get(name) : undefined;
  const run = (command: string) => {
    state?.set({ focused: name });
    editor.commands.run(command).then(
      () => setError(null),
      (failure) => setError(errorText(failure)),
    );
  };

  let body;
  if (value.type === "ron") body = <RonText name={name} text={value.text} />;
  else {
    const generic = schema ? (
      <Field schema={schema} value={value.value} path={[]} edit={edit} readOnly={readOnly} editor={editor} owner={entity && { entity, component: name }} />
    ) : (
      <pre>{JSON.stringify(value.value, null, 2)}</pre>
    );
    if (Custom && entity) {
      const onChange = (path: PathSegment[], next: unknown) =>
        void edit(path, next as JsonValue).then(
          () => setError(null),
          (failure) => setError(errorText(failure)),
        );
      const fallback = ({ text, retry }: PluginFailure) => (
        <>
          <p role="alert" className="ludenea-error">
            {text} <button onClick={retry}>Reload inspector</button>
          </p>
          {generic}
        </>
      );
      body = (
        <PluginBoundary editor={editor} registration={Custom} name={`The "${name}" inspector`} fallback={fallback}>
          <Custom value={value.value} entity={entity} onChange={onChange} />
        </PluginBoundary>
      );
    } else body = generic;
  }

  return (
    <section aria-label={name} aria-current={focused || undefined} className="ludenea-component" onFocus={() => state?.set({ focused: name })}>
      <h3 tabIndex={title ? undefined : 0} title={name}>
        {title ?? shortName(name)}
        {state && (
          <>
            {" "}
            <button disabled={readOnly} onClick={() => run("inspector.remove-component")}>Remove</button>
            <button disabled={readOnly} onClick={() => run("inspector.apply-to-scene")}>Apply to scene</button>
          </>
        )}
      </h3>
      {body}
      {error && <p className="ludenea-error">{error}</p>}
    </section>
  );
}

/**
 * How much of a value's RON text the inspector lays out: a resource such as the contact cache
 * holds hundreds of kilobytes, which take the browser a frame's budget many times over to lay
 * out, again on every change while playing.
 */
const RON_SHOWN = 2000;

function RonText({ name, text }: { name: string; text: string }) {
  const [all, setAll] = useState(false);
  const hidden = all ? 0 : text.length - RON_SHOWN;
  return (
    <>
      <pre aria-label={`${name} as RON`}>{hidden > 0 ? `${text.slice(0, RON_SHOWN)}…` : text}</pre>
      {hidden > 0 && (
        <p className="ludenea-muted">
          {hidden.toLocaleString("en")} more characters. <button onClick={() => setAll(true)}>Show all</button>
        </p>
      )}
    </>
  );
}

/** Picks a component the entity lacks and inserts its default value; a rejected value opens a form to fix it. */
function AddComponent(props: {
  editor: Editor;
  control: ControlClient;
  entity: Entity;
  schemas: Map<string, TypeSchema>;
  present: Values;
  state: InspectorState;
  onAdded(): void;
}) {
  const { editor, control, entity, schemas, present, state, onAdded } = props;
  const adding = useStore(state).adding!;
  const update = (patch: Partial<AddDraft>) => state.set({ adding: { ...state.get().adding!, ...patch } });
  const candidates = [...schemas.keys()].filter((name) => !(name in present));
  // Components come and go while the picker shows: a choice no longer offered falls back to the first.
  const chosen = candidates.includes(adding.chosen) ? adding.chosen : (candidates[0] ?? "");
  const draft = adding.draft?.component === chosen ? adding.draft.value : null;
  const { error } = adding;
  const schema = schemas.get(chosen);

  const insert = async () => {
    if (!schema) return;
    const value = draft ?? defaultValue(schema);
    try {
      await onEntity(editor, edit(editor, control, { type: "insert_component", entity, component: chosen, value }));
      state.set({ adding: null, focused: chosen });
      onAdded();
    } catch (failure) {
      const rejected = failure instanceof ProtocolFailure && failure.code === "invalid_edit";
      update({ error: errorText(failure), ...(rejected ? { draft: { component: chosen, value } } : {}) });
    }
  };
  // A change to the value makes the last failure, about the previous value, out of date.
  const editDraft: Edit = async (path, value) => update({ draft: { component: chosen, value: setAt(state.get().adding!.draft!.value, path, value) }, error: null });

  return (
    <div className="ludenea-add-component">
      <select aria-label="Component" value={chosen} onChange={(event) => update({ chosen: event.target.value, draft: null, error: null })}>
        {candidates.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </select>
      {schema && draft !== null && (
        <form aria-label={`New ${chosen}`} onSubmit={(event) => event.preventDefault()}>
          <Field schema={schema} value={draft} path={[]} readOnly={false} editor={editor} edit={editDraft} />
          {error && <p className="ludenea-error">{error}</p>}
        </form>
      )}
      {error && draft === null && <p className="ludenea-error">{error}</p>}
      <button disabled={!schema} onClick={() => void insert()}>
        Insert
      </button>
      <button onClick={() => state.set({ adding: null })}>Cancel</button>
    </div>
  );
}
