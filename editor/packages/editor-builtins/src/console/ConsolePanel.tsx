import type { ConsoleLevel } from "@ludenea/control-client";
import { useStore, type Store } from "../shared";

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

const where = ({ file, line, column }: NonNullable<Entry["location"]>) =>
  [file, line, line === null ? null : column].filter((part) => part !== null).join(":");

export function ConsolePanel({ state, onReveal }: { state: Store<{ entries: Entry[]; levels: Levels }>; onReveal(file: string): void }) {
  const { entries, levels } = useStore(state);
  return (
    <div className="ludenea-console">
      <div className="ludenea-toolbar">
        {LEVELS.map(([level, title]) => (
          <label key={level}>
            <input type="checkbox" checked={levels[level]} onChange={(event) => state.set({ levels: { ...levels, [level]: event.target.checked } })} />
            {title}
          </label>
        ))}
      </div>
      <ul aria-label="Console" className="ludenea-console-lines">
        {entries
          .filter((entry) => levels[entry.level])
          .map((entry) => (
            <li key={entry.id} className={`ludenea-console-${entry.level}`}>
              {entry.location && (
                <button className="ludenea-link" onClick={() => onReveal(entry.location!.file)}>
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
