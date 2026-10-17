import type { FileEntry } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useRef, type KeyboardEvent } from "react";
import { errorText, shared, useStore } from "../shared";
import type { Action, Assets } from "./index";

export const baseName = (path: string) => path.slice(path.lastIndexOf("/") + 1);
export const folderOf = (path: string) => path.slice(0, Math.max(path.lastIndexOf("/"), 0));

const ROOT_FOLDERS = ["assets", "scenes"];

/** Files grouped under their folder, folders sorted, filtered by a case-insensitive substring of the path. */
function grouped(files: FileEntry[], filter: string): [string, FileEntry[]][] {
  const needle = filter.trim().toLowerCase();
  const folders = new Map<string, FileEntry[]>(ROOT_FOLDERS.map((folder) => [folder, []]));
  for (const file of files) {
    const folder = folderOf(file.path);
    if (!folders.has(folder)) folders.set(folder, []);
    if (file.path.toLowerCase().includes(needle)) folders.get(folder)!.push(file);
  }
  const sorted = [...folders].sort(([a], [b]) => a.localeCompare(b));
  for (const [, entries] of sorted) entries.sort((a, b) => a.path.localeCompare(b.path));
  return needle ? sorted.filter(([, entries]) => entries.length > 0) : sorted;
}

export function AssetsPanel({ editor, assets }: { editor: Editor; assets: Assets }) {
  const { files, filter, selected, action, menu, banner, error } = useStore(assets.state);
  const openScene = useStore(shared(editor).scene).path;
  const unsaved = editor.timeline((state) => state.timeline?.scene_dirty ?? false);
  const dragged = useRef<string | null>(null);
  const set = assets.state.set;
  const attempt = (work: Promise<void>) =>
    void work.then(
      () => set({ error: null }),
      (failure) => set({ error: errorText(failure) }),
    );
  const groups = grouped(files, filter);

  return (
    <div className="ludenea-assets">
      <input type="search" aria-label="Filter assets" placeholder="Filter" value={filter} onChange={(event) => set({ filter: event.target.value })} />
      {banner && (
        <div role="alert" className="ludenea-banner">
          {banner.path} changed on disk.
          <button onClick={() => attempt(assets.reload())}>Reload</button>
          <button onClick={assets.keepMine}>Keep mine</button>
        </div>
      )}
      {error && <p className="ludenea-error">{error}</p>}
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
                  onClick={() => set({ selected: file.path, menu: null })}
                  onDoubleClick={() => attempt(assets.open(file.path))}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") attempt(assets.open(file.path));
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
      {filter && groups.length === 0 && <p className="ludenea-muted">No asset matches {filter}.</p>}
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
      {file.kind === "Scene" && (
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
