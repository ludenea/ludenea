import type { Editor } from "./editor";
import type { Registry } from "./registry";

const MODIFIERS = ["Ctrl", "Cmd", "Alt", "Shift"] as const;
const ALIASES: { [name: string]: (typeof MODIFIERS)[number] } = {
  ctrl: "Ctrl",
  control: "Ctrl",
  cmd: "Cmd",
  meta: "Cmd",
  alt: "Alt",
  option: "Alt",
  shift: "Shift",
};

/** Single printable characters other than letters already carry Shift (`+`, `]`), so it is dropped for them. */
function canonical(modifiers: Set<string>, key: string): string {
  const named = key.length === 1 ? key.toUpperCase() : key[0]!.toUpperCase() + key.slice(1);
  if (key.length === 1 && named === key.toLowerCase()) modifiers.delete("Shift");
  return [...MODIFIERS.filter((modifier) => modifiers.has(modifier)), named].join("+");
}

/** `shift+ctrl+z`, `Ctrl+Shift+Z`, and `Control+Shift+z` all become `Ctrl+Shift+Z`. */
export function normalizeShortcut(shortcut: string): string {
  const parts = shortcut.endsWith("+") ? [...shortcut.slice(0, -1).split("+").slice(0, -1), "+"] : shortcut.split("+");
  const key = parts.pop() ?? "";
  const modifiers = new Set(
    parts.map((part) => {
      const modifier = ALIASES[part.toLowerCase()];
      if (!modifier) throw new Error(`unknown modifier "${part}" in shortcut "${shortcut}"`);
      return modifier;
    }),
  );
  return canonical(modifiers, key);
}

export function shortcutOf(event: KeyboardEvent): string {
  const modifiers = new Set<string>();
  if (event.ctrlKey) modifiers.add("Ctrl");
  if (event.metaKey) modifiers.add("Cmd");
  if (event.altKey) modifiers.add("Alt");
  if (event.shiftKey) modifiers.add("Shift");
  return canonical(modifiers, event.key);
}

const NON_TEXT_INPUTS = new Set(["button", "checkbox", "color", "file", "image", "radio", "range", "reset", "submit"]);

export function isTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  return target instanceof HTMLInputElement && !NON_TEXT_INPUTS.has(target.type);
}

/** Runs registered shortcuts on key presses in `target`, except while a text field has focus. */
export function installShortcuts(editor: Editor, registry: Registry, target: Window = window): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.defaultPrevented || isTextField(event.target)) return;
    const command = registry.shortcuts.get(shortcutOf(event));
    if (!command) return;
    event.preventDefault();
    editor.commands.run(command).catch((error) => console.error(`command "${command}" failed:`, error));
  };
  target.addEventListener("keydown", onKeyDown);
  return () => target.removeEventListener("keydown", onKeyDown);
}
