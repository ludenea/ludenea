import { ProtocolFailure, type AssetId, type ComponentValue, type ControlClient, type Entity, type FieldTarget, type JsonValue, type PathSegment, type Timeline, type TypeSchema } from "@ludenea/control-client";
import { PluginBoundary, type EditCommand, type Editor, type PluginFailure } from "@ludenea/editor-shell";
import { useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { errorText, useGame, useStore, useUnsavedAsset } from "../shared";
import { ResimulatingNotice, useResimulating } from "../timeline/editing";
import { defaultValue, setAt } from "./default-value";
import { Field } from "./fields/Field";
import { MixedPaths, type Edit } from "./fields/inputs";
import { mixedPaths } from "./fields/mixed";
import { onEntity, type AddDraft, type InspectorState } from "./index";

type Values = { [name: string]: ComponentValue };
/** A selected entity and its components. */
type Loaded = { entity: Entity; components: Values };

// The timeline drives the time; editing it would fight the timeline.
const READ_ONLY_RESOURCES = new Set(["ludenea::Time"]);

/** Shows the failure of `work` through `setError`, or clears the error shown once it succeeds. */
const showOutcome = (work: Promise<unknown>, setError: (error: string | null) => void) =>
  void work.then(
    () => setError(null),
    (failure) => setError(errorText(failure)),
  );

const shortName = (name: string) => name.split("::").at(-1)!;
const schemaMap = (named: { name: string; schema: TypeSchema }[]) => new Map(named.map(({ name, schema }) => [name, schema]));

export function InspectorPanel({ editor, state }: { editor: Editor; state: InspectorState }) {
  const control = useGame(editor);
  const asset = editor.selection((selection) => selection.asset);
  // A texture is the project's, not the game's: its inspector shows alone, game or none.
  if (asset?.kind === "Texture") {
    const registered = editor.inspectors.getAsset("Texture");
    if (!registered) return <p className="ludenea-muted">No inspector is registered for textures.</p>;
    // A texture has no value: its PNG and its meta are files of the project.
    const changeTexture = () =>
      editor.console.report(new Error("a texture has no value to change: write its PNG or its meta through `editor.project`"), `The "Texture" asset inspector of plugin "${editor.pluginOf(registered)}"`);
    return (
      <div className="ludenea-inspector">
        <AssetInspectorSlot editor={editor} name="Texture" id={asset.id} value={null} onChange={changeTexture} />
      </div>
    );
  }
  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  return <Inspector editor={editor} control={control} state={state} />;
}

/**
 * The asset inspector registered as `name`, if any, isolated from the panel: one that throws
 * shows its failure with a button to render it again.
 */
function AssetInspectorSlot({ editor, name, id, value, onChange }: { editor: Editor; name: string; id: AssetId; value: unknown | null; onChange(path: PathSegment[], value: unknown): void }) {
  const Custom = editor.inspectors.getAsset(name);
  if (!Custom) return null;
  const fallback = ({ text, retry }: PluginFailure) => (
    <p role="alert" className="ludenea-error">
      {text} <button onClick={retry}>Reload inspector</button>
    </p>
  );
  return (
    <PluginBoundary editor={editor} registration={Custom} name={`The "${name}" asset inspector`} fallback={fallback} resetKeys={[id]}>
      <Custom id={id} value={value} onChange={onChange} />
    </PluginBoundary>
  );
}

function Inspector({ editor, control, state }: { editor: Editor; control: ControlClient; state: InspectorState }) {
  const entities = editor.selection((selection) => selection.entities);
  const asset = editor.selection((selection) => (selection.asset?.kind === "Data" ? selection.asset.id : null));
  const { adding, focused, expanded } = useStore(state);
  const components = useMemo(() => schemaMap(control.game.components), [control]);
  const resourceSchemas = useMemo(() => schemaMap(control.game.resources), [control]);
  const values = useEntities(editor, control, entities);
  const resimulating = useResimulating(editor);
  // One entity shows its custom inspectors and field decorations; several show their shared fields.
  const single = entities.length === 1 ? entities[0] : undefined;

  const setResource = (resource: string): Edit => async (path, value) => {
    await editor.edit({ type: "set_field", target: { type: "resource", resource }, path, value });
  };
  // A field of several entities changes in one batch: one undo step, all of them or none.
  const setComponent = (component: string): Edit => async (path, value) => {
    const target = (entity: Entity): FieldTarget => ({ type: "component", entity, component });
    await onEntity(editor, editor.batched(entities.map((entity): EditCommand => ({ type: "set_field", target: target(entity), path, value }))));
    values.refresh();
  };

  return (
    <div className="ludenea-inspector">
      <ResimulatingNotice editor={editor} />
      {asset && <DataAsset editor={editor} control={control} id={asset} />}
      {entities.length > 0 && (
        <>
          <h2>
            {single ? `Entity ${single.index}v${single.generation}` : `${entities.length} entities selected`}{" "}
            <button disabled={resimulating} onClick={() => void editor.commands.run("inspector.add-component").catch((error) => values.setError(errorText(error)))}>Add component</button>
          </h2>
          {values.error && <p className="ludenea-error">{values.error}</p>}
          {adding && <AddComponent editor={editor} entities={values.loaded ?? []} schemas={components} state={state} onAdded={values.refresh} />}
          {shared(values.loaded ?? [], components).map(({ name, value, mixed }) => (
            <MixedPaths.Provider key={name} value={mixed}>
              <Section
                editor={editor}
                name={name}
                value={value}
                schema={components.get(name)}
                entity={single}
                edit={setComponent(name)}
                state={state}
                focused={focused === name}
                readOnly={resimulating}
              />
            </MixedPaths.Provider>
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
          edit={setResource(name)}
          readOnly={resimulating || READ_ONLY_RESOURCES.has(name)}
        />
      ))}
    </div>
  );
}

/**
 * The components every one of `loaded` has, in the first one's order, with the first one's value
 * and the paths where their values differ.
 */
function shared(loaded: Loaded[], schemas: Map<string, TypeSchema>): { name: string; value: ComponentValue; mixed: ReadonlySet<string> }[] {
  const [first, ...others] = loaded;
  if (!first) return [];
  return Object.entries(first.components).flatMap(([name, value]) => {
    const all = [value, ...others.flatMap(({ components }) => components[name] ?? [])];
    if (all.length !== loaded.length) return [];
    const schema = schemas.get(name);
    const json = all.flatMap((each) => (each.type === "json" ? [each.value] : []));
    // A value without a schema or without a JSON form shows whole: it differs as a whole.
    const whole = all.every((each) => JSON.stringify(each) === JSON.stringify(value)) ? new Set<string>() : new Set([""]);
    return [{ name, value, mixed: schema && json.length === all.length ? mixedPaths(schema, json) : whole }];
  });
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
    await editor.edit({ type: "set_asset_field", id, path, value });
    setVersion((version) => version + 1);
  };
  const run = (command: string) => () => showOutcome(editor.commands.run(command), setError);
  const schema = asset && control.game.assets.find(({ name }) => name === asset.asset)?.schema;
  const onChange = (path: PathSegment[], value: unknown) => showOutcome(setField(path, value as JsonValue), setError);
  // A value without a JSON form is shown as RON, which an asset inspector cannot read.
  const below = asset && asset.value.type === "json" && <AssetInspectorSlot editor={editor} name={asset.asset} id={id} value={asset.value.value} onChange={onChange} />;

  return (
    <>
      <h2>
        Data asset {asset && shortName(asset.asset)}
        {unsaved && <span role="img" aria-label="unsaved changes"> ●</span>}{" "}
        {unsaved && <button onClick={run("inspector.save-asset")}>Save asset</button>}
        {tick > 0 && (
          <button disabled={resimulating} onClick={run("inspector.apply-to-asset")}>
            Apply to asset
          </button>
        )}
      </h2>
      {error && <p className="ludenea-error">{error}</p>}
      {asset && <Section editor={editor} name={asset.asset} value={asset.value} schema={schema ?? undefined} edit={setField} readOnly={resimulating} below={below} />}
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

/** The selected entities' components, in selection order; a change the host reports reloads that entity only. */
function useEntities(editor: Editor, control: ControlClient, entities: Entity[]) {
  const [loaded, setLoaded] = useState<Loaded[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  // The same entities in a new array load nothing again.
  const key = entities.map(({ index, generation }) => `${index}v${generation}`).join(" ");

  useEffect(() => {
    setLoaded(null);
    setError(null);
  }, [key]);
  // Reloading keeps the shown values (and the focused field) until the new ones arrive.
  useEffect(() => {
    if (entities.length === 0) return;
    let current = true;
    // Shown once every entity has loaded; replies for one entity arrive in request order.
    const replies: (Loaded | undefined)[] = entities.map(() => undefined);
    const load = (entity: Entity, at: number) =>
      void onEntity(editor, control.request({ type: "get_entity", entity })).then(
        ({ components }) => {
          replies[at] = { entity, components };
          const all = replies.filter((reply) => reply !== undefined);
          if (current && all.length === entities.length) setLoaded(all);
        },
        (failure) => current && setError(errorText(failure)),
      );
    entities.forEach(load);
    const watches = entities.map((entity, at) => editor.entities.watch(entity, () => load(entity, at)));
    return () => {
      current = false;
      watches.forEach((watch) => watch.dispose());
    };
    // `key` names `entities`.
  }, [editor, control, key, version]);

  return { loaded, error, setError, refresh: () => setVersion((version) => version + 1) };
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
  /** The entity of a component of one selected entity; several selected entities, or a resource, have none. */
  entity?: Entity;
  /** What the heading shows instead of the short name. */
  title?: ReactNode;
  state?: InspectorState;
  focused?: boolean;
  readOnly?: boolean;
  /** What shows below the fields, such as a data asset's own inspector. */
  below?: ReactNode;
}

/**
 * One component, resource, or data asset: its custom inspector, its schema-driven fields, or
 * read-only text. A custom inspector that throws gives way to the schema-driven fields.
 */
function Section({ editor, name, title, value, schema, edit, entity, state, focused = false, readOnly = false, below }: SectionProps) {
  const [error, setError] = useState<string | null>(null);
  const Custom = entity ? editor.inspectors.get(name) : undefined;
  // A value shown whole (as RON, or without a schema) differs as a whole.
  const differs = useContext(MixedPaths).has("") && (value.type === "ron" || !schema);
  const run = (command: string) => {
    state?.set({ focused: name });
    showOutcome(editor.commands.run(command), setError);
  };

  let body;
  if (value.type === "ron") body = <RonText name={name} text={value.text} />;
  else {
    const generic = schema ? (
      <Field schema={schema} value={value.value} path={[]} edit={edit} readOnly={readOnly} editor={editor} owner={entity && { entity, component: name, value: value.value }} />
    ) : (
      <pre>{JSON.stringify(value.value, null, 2)}</pre>
    );
    if (Custom && entity) {
      const onChange = (path: PathSegment[], next: unknown) => showOutcome(edit(path, next as JsonValue), setError);
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
            {entity && (
              <button disabled={readOnly} onClick={() => run("inspector.apply-to-scene")}>
                Apply to scene
              </button>
            )}
          </>
        )}
      </h3>
      {differs && <p className="ludenea-mixed">The selected entities have different values; the first one's is shown.</p>}
      {body}
      {below}
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

/**
 * Picks a component a selected entity lacks and inserts its default value into each one lacking
 * it, in one batch; a rejected value opens a form to fix it.
 */
function AddComponent(props: { editor: Editor; entities: Loaded[]; schemas: Map<string, TypeSchema>; state: InspectorState; onAdded(): void }) {
  const { editor, entities, schemas, state, onAdded } = props;
  const adding = useStore(state).adding!;
  const update = (patch: Partial<AddDraft>) => state.set({ adding: { ...state.get().adding!, ...patch } });
  const lacking = (name: string) => entities.filter(({ components }) => !(name in components)).map(({ entity }) => entity);
  const candidates = [...schemas.keys()].filter((name) => lacking(name).length > 0);
  // Components come and go while the picker shows: a choice no longer offered falls back to the first.
  const chosen = candidates.includes(adding.chosen) ? adding.chosen : (candidates[0] ?? "");
  const draft = adding.draft?.component === chosen ? adding.draft.value : null;
  const { error } = adding;
  const schema = schemas.get(chosen);

  const insert = async () => {
    if (!schema) return;
    const value = draft ?? defaultValue(schema);
    const commands = lacking(chosen).map((entity): EditCommand => ({ type: "insert_component", entity, component: chosen, value }));
    try {
      await onEntity(editor, editor.batched(commands));
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
