import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import type { Editor } from "../editor";
import type { CommandDefinition } from "../plugin";
import type { Registry } from "../registry";

/** Whether the letters of `query` appear in order in `text`, ignoring case. */
export function fuzzyMatch(query: string, text: string): boolean {
  const haystack = text.toLowerCase();
  let position = 0;
  for (const letter of query.toLowerCase().replaceAll(" ", "")) {
    position = haystack.indexOf(letter, position) + 1;
    if (position === 0) return false;
  }
  return true;
}

const optionId = (index: number) => `ludenea-palette-option-${index}`;

function matching(commands: CommandDefinition[], query: string): CommandDefinition[] {
  return commands
    .filter((command) => fuzzyMatch(query, command.title) || fuzzyMatch(query, command.id))
    .sort((a, b) => a.title.localeCompare(b.title));
}

/** Every registered command, opened with `Ctrl+K` or `Cmd+K` (also from text fields). */
export function CommandPalette({ editor, registry }: { editor: Editor; registry?: Registry }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [highlighted, setHighlighted] = useState(0);
  const list = useRef<HTMLUListElement>(null);

  // Arrow keys move the highlight past the visible part of a long list.
  useEffect(() => {
    list.current?.children[highlighted]?.scrollIntoView?.({ block: "nearest" });
  }, [highlighted, open, query]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen((open) => !open);
        setQuery("");
        setHighlighted(0);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  if (!open) return null;
  const commands = matching(editor.commands.list(), query);
  const shortcutOf = new Map(registry?.shortcuts.entries().map(([key, command]) => [command, key]));
  const run = (command: CommandDefinition | undefined) => {
    if (!command) return;
    setOpen(false);
    editor.commands.run(command.id).catch((error) => console.error(`command "${command.id}" failed:`, error));
  };
  const onKeyDown = (event: ReactKeyboardEvent) => {
    if (event.key === "Escape") setOpen(false);
    else if (event.key === "Enter") run(commands[highlighted]);
    else if (event.key === "ArrowDown") setHighlighted((index) => Math.min(index + 1, commands.length - 1));
    else if (event.key === "ArrowUp") setHighlighted((index) => Math.max(index - 1, 0));
    else return;
    event.preventDefault();
  };

  return (
    <div className="ludenea-palette-backdrop" data-testid="palette-backdrop" onClick={(event) => event.target === event.currentTarget && setOpen(false)}>
      <div role="dialog" aria-label="Command palette" className="ludenea-palette">
        <input
          role="combobox"
          aria-expanded
          aria-controls="ludenea-palette-commands"
          aria-activedescendant={commands[highlighted] && optionId(highlighted)}
          aria-label="Command"
          placeholder="Type a command"
          autoFocus
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setHighlighted(0);
          }}
          onKeyDown={onKeyDown}
        />
        <ul ref={list} role="listbox" id="ludenea-palette-commands">
          {commands.map((command, index) => (
            <li key={command.id} id={optionId(index)} role="option" aria-selected={index === highlighted} onClick={() => run(command)}>
              <span>{command.title}</span>
              {shortcutOf.has(command.id) && <kbd>{shortcutOf.get(command.id)}</kbd>}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
