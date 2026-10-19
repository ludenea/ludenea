import type { ConsoleLevel } from "@ludenea/control-client";
import type { Editor, Observed } from "./editor";

/** The file an entry points at, relative to the project root or absolute; its line and column when known. */
export interface ConsoleLocation {
  file: string;
  line: number | null;
  column: number | null;
}

export interface ConsoleEntry {
  /** Larger for each entry than for every entry logged before it. */
  id: number;
  level: ConsoleLevel;
  text: string;
  location?: ConsoleLocation;
}

/**
 * The editor's log, which the Console panel shows: entries logged before it opens wait for it.
 * Lists the last entries, oldest first.
 */
export interface EditorConsole extends Observed<ConsoleEntry> {
  /** Shows `text`, from `location` when given. */
  log(level: ConsoleLevel, text: string, location?: ConsoleLocation): void;
  /** Logs `<context> failed: <message>` at level `error`, `context` naming what failed (such as a command title); returns that line. */
  report(error: unknown, context: string): string;
}

// ponytail: keeps the last entries only, as the Console panel does; page older ones to disk if long sessions need them.
const LIMIT = 5000;

/** `error`'s message; an `AggregateError` also lists the message of each failure it holds. */
function message(error: unknown): string {
  if (error instanceof AggregateError) return `${error.message}: ${error.errors.map(message).join("; ")}`;
  return error instanceof Error ? error.message : String(error);
}

export function createConsole(): EditorConsole {
  let entries: ConsoleEntry[] = [];
  let lastId = 0;
  const listeners = new Set<() => void>();
  const log = (level: ConsoleLevel, text: string, location?: ConsoleLocation) => {
    entries = [...entries.slice(1 - LIMIT), { id: ++lastId, level, text, location }];
    for (const listener of [...listeners]) listener();
  };
  return {
    log,
    report(error, context) {
      const text = `${context} failed: ${message(error)}`;
      log("error", text);
      return text;
    },
    list: () => entries,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

/**
 * Runs the command `id` as the palette and the shortcuts do: a failure goes to the editor
 * console as `<command title> failed: <message>`. Resolves to that line, or null once the
 * command succeeded.
 */
export async function runReported(editor: Editor, id: string): Promise<string | null> {
  try {
    await editor.commands.run(id);
    return null;
  } catch (error) {
    return editor.console.report(error, editor.commands.list().find((command) => command.id === id)?.title ?? id);
  }
}
