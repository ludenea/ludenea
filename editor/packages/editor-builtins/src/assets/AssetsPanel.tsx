import type { FileEntry, JsonValue, PathSegment } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useRef, type KeyboardEvent } from "react";
import { setAt } from "../inspector/default-value";
import { Field } from "../inspector/fields/Field";
import { ASSET_FOLDERS, errorText, useGame, useKeptScroll, useStore } from "../shared";
import { kindName, type Action, type Assets, type KindName, type NewData } from "./index";
import { emptyValue, isComplete } from "./value";

const ICONS: { [kind in KindName]: string } = { Scene: "▣", Prefab: "◇", Data: "≡", Replay: "▶", Trace: "∿", Snapshot: "◉", Other: "·" };
const KINDS = Object.keys(ICONS) as KindName[];
const OPENABLE = new Set<KindName>(["Scene", "Prefab", "Data", "Replay", "Trace", "Snapshot"]);

export const baseName = (path: string) => path.slice(path.lastIndexOf("/") + 1);
export const folderOf = (path: string) => path.slice(0, Math.max(path.lastIndexOf("/"), 0));

/** Files grouped under their folder, folders sorted, filtered by kind and by a case-insensitive substring of the path. */
function grouped(files: FileEntry[], filter: string, kind: KindName | null): [string, FileEntry[]][] {
  const needle = filter.trim().toLowerCase();
  const folders = new Map<string, FileEntry[]>(ASSET_FOLDERS.map((folder) => [folder, []]));
  for (const file of files) {
    const folder = folderOf(file.path);
    if (!folders.has(folder)) folders.set(folder, []);
    if (file.path.toLowerCase().includes(needle) && (kind === null || kindName(file.kind) === kind)) folders.get(folder)!.push(file);
  }
  const sorted = [...folders].sort(([a], [b]) => a.localeCompare(b));
  for (const [, entries] of sorted) entries.sort((a, b) => a.path.localeCompare(b.path));
  return needle || kind ? sorted.filter(([, entries]) => entries.length > 0) : sorted;
}

export function AssetsPanel({ editor, assets }: { editor: Editor; assets: Assets }) {
  const { files, filter, kind, selected, action, menu, creating, banners, error, listError } = useStore(assets.state);
  const openScene = editor.document((state) => state.current?.path ?? null);
  const unsaved = editor.timeline((state) => state.timeline?.scene_dirty ?? false);
  const dragged = useRef<string | null>(null);
  const scrolled = useKeptScroll<HTMLDivElement>(assets.scroll);
  const set = assets.state.set;
  const attempt = (work: Promise<void>) =>
    void work.then(
      () => set({ error: null }),
      (failure) => set({ error: errorText(failure) }),
    );
  const groups = grouped(files, filter, kind);
  // A data asset selected in the browser is the one the inspector edits.
  const select = (file: FileEntry) => {
    set({ selected: file.path, menu: null });
    if (kindName(file.kind) === "Data") editor.selection.getState().selectAsset(file.id);
  };

  return (
    <div className="ludenea-assets" ref={scrolled.ref} onScroll={scrolled.onScroll}>
      <input type="search" aria-label="Filter assets" placeholder="Filter" value={filter} onChange={(event) => set({ filter: event.target.value })} />
      <div className="ludenea-toolbar">
        <select aria-label="Kind" value={kind ?? ""} onChange={(event) => set({ kind: (event.target.value || null) as KindName | null })}>
          <option value="">All kinds</option>
          {KINDS.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
        <button onClick={() => void editor.commands.run("assets.new-data")}>New data asset</button>
      </div>
      {creating && <NewDataForm editor={editor} creating={creating} assets={assets} attempt={attempt} />}
      {banners.map((banner) => (
        <div key={banner.path} role="alert" className="ludenea-banner">
          {banner.path} changed on disk.
          <button onClick={() => attempt(assets.reload(banner))}>Reload</button>
          <button onClick={() => assets.keepMine(banner)}>Keep mine</button>
        </div>
      ))}
      {error && <p className="ludenea-error">{error}</p>}
      {listError && <p className="ludenea-error">{listError}</p>}
      {action && <ActionForm action={action} assets={assets} attempt={attempt} />}
      <ul role="tree" aria-label="Assets">
        {groups.map(([folder, entries]) => (
          <li
            key={folder}
            role="none"
            onDragOver={(event) => event.preventDefault()}
            onDrop={(event) => {
              event.preventDefault();
              const path = dragged.current;
              dragged.current = null;
              if (path && folderOf(path) !== folder) attempt(assets.move(path, folder));
            }}
          >
            <span className="ludenea-folder">{folder}/</span>
            <ul role="group">
              {entries.map((file) => (
                <li
                  key={file.path}
                  role="treeitem"
                  aria-selected={file.path === selected}
                  tabIndex={0}
                  draggable
                  title={typeof file.kind === "object" ? file.kind.Data.type : file.kind}
                  onClick={() => select(file)}
                  onFocus={() => set({ selected: file.path })}
                  onDoubleClick={() => attempt(assets.open(file.path))}
                  onKeyDown={(event) => {
                    if (event.target !== event.currentTarget) return;
                    if (event.key === " ") {
                      event.preventDefault();
                      select(file);
                    } else if (event.key === "Enter") {
                      set({ selected: file.path });
                      attempt(assets.open(file.path));
                    } else if (event.key === "Delete") {
                      // Confirmed in the panel, like the menu's Delete.
                      set({ selected: file.path, action: { kind: "delete", path: file.path }, menu: null });
                    }
                  }}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    set({ selected: file.path, menu: file.path });
                  }}
                  onDragStart={(event) => {
                    dragged.current = file.path;
                    event.dataTransfer?.setData("text/plain", file.path);
                  }}
                >
                  <span className="ludenea-asset-icon" aria-hidden="true">
                    {ICONS[kindName(file.kind)]}
                  </span>{" "}
                  <span>{baseName(file.path)}</span>
                  {file.path === openScene && (
                    <span className="ludenea-open-scene">
                      {" "}
                      (open){unsaved && <span aria-label="unsaved changes"> ●</span>}
                    </span>
                  )}
                  {menu === file.path && <ContextMenu file={file} assets={assets} attempt={attempt} />}
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
      {(filter || kind) && groups.length === 0 && <p className="ludenea-muted">No asset matches {filter || kind}.</p>}
    </div>
  );
}

type Attempt = (work: Promise<void>) => void;

function ContextMenu({ file, assets, attempt }: { file: FileEntry; assets: Assets; attempt: Attempt }) {
  const ask = (kind: Action["kind"]) => () => assets.state.set({ action: { kind, path: file.path }, menu: null });
  return (
    <div
      role="menu"
      onClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        if (event.key === "Escape") assets.state.set({ menu: null });
      }}
    >
      {OPENABLE.has(kindName(file.kind)) && (
        <button
          role="menuitem"
          onClick={() => {
            assets.state.set({ menu: null });
            attempt(assets.open(file.path));
          }}
        >
          Open
        </button>
      )}
      <button role="menuitem" onClick={ask("rename")}>
        Rename
      </button>
      <button role="menuitem" onClick={ask("move")}>
        Move
      </button>
      <button role="menuitem" onClick={ask("delete")}>
        Delete
      </button>
    </div>
  );
}

function ActionForm({ action, assets, attempt }: { action: Action; assets: Assets; attempt: Attempt }) {
  const cancel = () => assets.state.set({ action: null });
  const submitOnEnter = (submit: (value: string) => void) => (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") cancel();
    if (event.key !== "Enter") return;
    const value = event.currentTarget.value.trim();
    if (value) submit(value);
  };
  const folder = folderOf(action.path);

  if (action.kind === "delete") {
    return (
      <div className="ludenea-confirm">
        Delete {action.path}?{" "}
        <button aria-label={`Delete ${action.path}`} onClick={() => attempt(assets.remove(action.path))}>
          Delete
        </button>
        <button onClick={cancel}>Cancel</button>
      </div>
    );
  }
  if (action.kind === "rename") {
    return (
      <input
        key={action.path}
        aria-label="New name"
        autoFocus
        defaultValue={baseName(action.path)}
        onKeyDown={submitOnEnter((name) => attempt(assets.rename(action.path, folder ? `${folder}/${name}` : name)))}
        onBlur={cancel}
      />
    );
  }
  return (
    <input
      key={action.path}
      aria-label="Destination folder"
      autoFocus
      defaultValue={folder}
      onKeyDown={submitOnEnter((destination) => attempt(assets.move(action.path, destination.replace(/\/+$/, ""))))}
      onBlur={cancel}
    />
  );
}

/** The asset type and path; then, for a type without a default value, every field of the value, required. */
function NewDataForm({ editor, creating, assets, attempt }: { editor: Editor; creating: NewData; assets: Assets; attempt: Attempt }) {
  const control = useGame(editor);
  const update = (patch: Partial<NewData>) => assets.state.set({ creating: { ...creating, ...patch } });
  const cancel = <button onClick={() => assets.state.set({ creating: null })}>Cancel</button>;
  const { value } = creating;

  if (value) {
    const edit = async (path: PathSegment[], next: JsonValue) => update({ value: { ...value, draft: setAt(value.draft, path, next) } });
    return (
      <form aria-label={`New ${creating.type}`} className="ludenea-new-data" onSubmit={(event) => event.preventDefault()}>
        <p>
          {creating.type} has no default value: fill in every field.
        </p>
        <Field schema={value.schema} value={value.draft} path={[]} edit={edit} readOnly={false} editor={editor} seed={emptyValue} />
        <button aria-label={`Create ${creating.path}`} disabled={!isComplete(value.schema, value.draft)} onClick={() => attempt(assets.create())}>
          Create
        </button>
        {cancel}
      </form>
    );
  }
  return (
    <form aria-label="New data asset" className="ludenea-new-data" onSubmit={(event) => event.preventDefault()}>
      <select aria-label="Asset type" value={creating.type} onChange={(event) => update({ type: event.target.value })}>
        {(control?.game.assets ?? []).map(({ name }) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </select>
      <input aria-label="Path" placeholder="assets/name.ron" value={creating.path} onChange={(event) => update({ path: event.target.value })} />
      <button onClick={() => attempt(assets.create())}>Create</button>
      {cancel}
    </form>
  );
}
