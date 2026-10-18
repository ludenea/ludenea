import type { ConsoleLevel } from "@ludenea/control-client";
import { useKeptScroll, useStore, type Scroll, type Store } from "../shared";

export type Levels = { [level in ConsoleLevel]: boolean };

export interface Entry {
  id: number;
  level: ConsoleLevel;
  /** `cargo` marks rendered compiler lines a diagnostic may still claim. */
  source: "cargo" | "build" | "game" | "editor";
  text: string;
  location?: { file: string; line: number | null; column: number | null };
}

const LEVELS: [ConsoleLevel, string][] = [
  ["error", "Errors"],
  ["warning", "Warnings"],
  ["info", "Info"],
];

export type Location = NonNullable<Entry["location"]>;

/** `file:line:column`, as far as known. */
export const where = ({ file, line, column }: Location) =>
  [file, line, line === null ? null : column].filter((part) => part !== null).join(":");

export function ConsolePanel(props: { state: Store<{ entries: Entry[]; levels: Levels; notice: string | null }>; scroll: Scroll; onLocation(location: Location): void }) {
  const { state, scroll, onLocation } = props;
  const { entries, levels, notice } = useStore(state);
  const lines = useKeptScroll<HTMLUListElement>(scroll);
  return (
    <div className="ludenea-console">
      <div className="ludenea-toolbar">
        {LEVELS.map(([level, title]) => (
          <label key={level}>
            <input type="checkbox" checked={levels[level]} onChange={(event) => state.set({ levels: { ...levels, [level]: event.target.checked } })} />
            {title}
          </label>
        ))}
        {notice && (
          <span role="status" className="ludenea-muted">
            {notice}
          </span>
        )}
      </div>
      <ul aria-label="Console" className="ludenea-console-lines" ref={lines.ref} onScroll={lines.onScroll}>
        {entries
          .filter((entry) => levels[entry.level])
          .map((entry) => (
            <li key={entry.id} className={`ludenea-console-${entry.level}`}>
              {entry.location && (
                <button className="ludenea-link" onClick={() => onLocation(entry.location!)}>
                  {where(entry.location)}
                </button>
              )}
              <pre>{entry.text}</pre>
            </li>
          ))}
      </ul>
    </div>
  );
}
