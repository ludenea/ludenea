import type { AssetKind, Entity, FileEntry, JsonValue, PathSegment } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { errorText } from "../../shared";

/** Sends one field's new value; rejects with the host's error. */
export type Edit = (path: PathSegment[], value: JsonValue) => Promise<void>;

export interface LeafProps {
  value: JsonValue;
  path: PathSegment[];
  edit: Edit;
  readOnly: boolean;
}

export const pathText = (path: PathSegment[]) => path.map((segment) => (segment.type === "field" ? segment.name : segment.index)).join(".");

const lastName = (path: PathSegment[]) => {
  const last = path.at(-1);
  return last ? (last.type === "field" ? last.name : `[${last.index}]`) : "value";
};

/** The edit of one field, keeping its last error to show under it. */
export function useCommit(edit: Edit, path: PathSegment[]) {
  const [error, setError] = useState<string | null>(null);
  const commit = (value: JsonValue) =>
    void edit(path, value).then(
      () => setError(null),
      (failure) => setError(errorText(failure)),
    );
  return { error, setError, commit };
}

/** The paths, as `pathText` writes them, where the values of the selected entities differ (see `mixedPaths`). */
export const MixedPaths = createContext<ReadonlySet<string>>(new Set());

/** Marks a row or group whose value differs between the selected entities; it shows the first one's. */
export function MixedMark({ path }: { path: PathSegment[] }) {
  if (!useContext(MixedPaths).has(pathText(path))) return null;
  return (
    <span className="ludenea-mixed" title="The selected entities have different values here; the first one's is shown">
      {" "}
      mixed
    </span>
  );
}

/** A labelled field with its error underneath. */
export function FieldRow({ path, error, children }: { path: PathSegment[]; error?: string | null; children: ReactNode }) {
  return (
    <div className="ludenea-field">
      <label>
        <span>{lastName(path)}</span> {children}
      </label>
      <MixedMark path={path} />
      {error && <p className="ludenea-error">{error}</p>}
    </div>
  );
}

// A value not given yet (a new data asset's form) shows empty.
const shown = (value: JsonValue) => (value === null ? "" : String(value));

/** An input committing its text on blur or Enter, and only when the text changed. */
function TextInput(props: { text: string; path: PathSegment[]; readOnly: boolean; type?: "text" | "number"; step?: number | "any"; unset?: boolean; onCommit(text: string): void }) {
  const { text, path, readOnly, type = "text", step, unset = false, onCommit } = props;
  // An unset value takes even the text it shows (an empty string).
  const commit = (next: string) => {
    if (next !== text || unset) onCommit(next);
  };
  return (
    <input
      key={text}
      type={type}
      step={step}
      aria-label={pathText(path)}
      defaultValue={text}
      readOnly={readOnly}
      onBlur={(event) => commit(event.currentTarget.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") commit(event.currentTarget.value);
      }}
    />
  );
}

export function NumberField({ value, path, edit, readOnly, integer }: LeafProps & { integer: boolean }) {
  const { error, setError, commit } = useCommit(edit, path);
  const onCommit = (next: string) => {
    const number = Number(next);
    if (next.trim() === "" || !Number.isFinite(number)) setError("enter a number");
    else commit(number);
  };
  return (
    <FieldRow path={path} error={error}>
      <TextInput type="number" step={integer ? 1 : "any"} text={shown(value)} path={path} readOnly={readOnly} onCommit={onCommit} />
    </FieldRow>
  );
}

/** `u64` and `i64`: past 2^53 a JSON number loses digits, so they are edited and sent as text. */
export function LargeIntegerField({ value, path, edit, readOnly }: LeafProps) {
  const { error, commit } = useCommit(edit, path);
  return (
    <FieldRow path={path} error={error}>
      <TextInput text={shown(value)} path={path} readOnly={readOnly} onCommit={(next) => commit(next.trim())} />
    </FieldRow>
  );
}

export function StringField({ value, path, edit, readOnly }: LeafProps) {
  const { error, commit } = useCommit(edit, path);
  return (
    <FieldRow path={path} error={error}>
      <TextInput unset={value === null} text={shown(value)} path={path} readOnly={readOnly} onCommit={commit} />
    </FieldRow>
  );
}

export function BoolField({ value, path, edit, readOnly }: LeafProps) {
  const { error, commit } = useCommit(edit, path);
  return (
    <FieldRow path={path} error={error}>
      <input type="checkbox" aria-label={pathText(path)} checked={value === true} disabled={readOnly} onChange={(event) => commit(event.target.checked)} />
    </FieldRow>
  );
}

/** An entity reference, as a link selecting it. */
export function EntityField({ value, path, editor }: LeafProps & { editor: Editor }) {
  const entity = value as Entity;
  return (
    <FieldRow path={path}>
      <button className="ludenea-link" aria-label={pathText(path)} onClick={() => editor.selection.getState().select([entity])}>
        {entity.index}v{entity.generation}
      </button>
    </FieldRow>
  );
}

/** Whether a file of `kind` holds an asset of the type named `asset`. */
const holds = (kind: AssetKind, asset: string) => (asset === "ludenea::Prefab" ? kind === "Prefab" : typeof kind === "object" && "Data" in kind && kind.Data.type === asset);

/** An asset identifier, picked among the project's files: those holding an `asset` for a handle. */
export function AssetField({ value, path, edit, readOnly, editor, asset }: LeafProps & { editor: Editor; asset?: string }) {
  const { error, setError, commit } = useCommit(edit, path);
  const [files, setFiles] = useState<FileEntry[]>([]);
  useEffect(() => {
    let current = true;
    editor.project.request({ type: "list_files" }).then(
      (reply) => current && setFiles(asset === undefined ? reply.entries : reply.entries.filter((file) => holds(file.kind, asset))),
      (failure) => current && setError(errorText(failure)),
    );
    return () => {
      current = false;
    };
  }, [editor, setError, asset]);
  const known = files.some((file) => file.id === value);
  return (
    <FieldRow path={path} error={error}>
      <select aria-label={pathText(path)} value={shown(value)} disabled={readOnly} onChange={(event) => commit(event.target.value)}>
        {!known && <option value={shown(value)}>{shown(value)}</option>}
        {files.map((file) => (
          <option key={file.id} value={file.id}>
            {file.path}
          </option>
        ))}
      </select>
    </FieldRow>
  );
}
