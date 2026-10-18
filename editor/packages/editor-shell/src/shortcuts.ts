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

/** Lower-cased name to `KeyboardEvent.key` value. */
const NAMED_KEYS = new Map(
  [
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Enter",
    "Escape",
    "Tab",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    ...Array.from({ length: 24 }, (_, index) => `F${index + 1}`),
  ].map((key): [string, string] => [key.toLowerCase(), key]),
);
NAMED_KEYS.set("esc", "Escape").set("del", "Delete");

/** `Mod` is Cmd on macOS and Ctrl elsewhere. */
export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
}

/**
 * Single characters are upper-cased; other printable characters already carry Shift (`+`, `]`),
 * so it is dropped for them. Longer names become their `KeyboardEvent.key` value.
 */
function canonical(modifiers: Set<string>, key: string, named: string): string {
  if (key.length === 1 && named === key.toLowerCase()) modifiers.delete("Shift");
  return [...MODIFIERS.filter((modifier) => modifiers.has(modifier)), named].join("+");
}

/** `shift+ctrl+z`, `Ctrl+Shift+Z`, and `Control+Shift+z` all become `Ctrl+Shift+Z`; `Mod+S` becomes `Cmd+S` on macOS. */
export function normalizeShortcut(shortcut: string, mac: boolean = isMac()): string {
  const parts = shortcut.endsWith("+") ? [...shortcut.slice(0, -1).split("+").slice(0, -1), "+"] : shortcut.split("+");
  const key = parts.pop() ?? "";
  const modifiers = new Set(
    parts.map((part) => {
      if (part.toLowerCase() === "mod") return mac ? "Cmd" : "Ctrl";
      const modifier = ALIASES[part.toLowerCase()];
      if (!modifier) throw new Error(`unknown modifier "${part}" in shortcut "${shortcut}"`);
      return modifier;
    }),
  );
  const named = key.length === 1 ? key.toUpperCase() : NAMED_KEYS.get(key.toLowerCase());
  if (!named) throw new Error(`unknown key "${key}" in shortcut "${shortcut}"`);
  return canonical(modifiers, key, named);
}

export function shortcutOf(event: KeyboardEvent): string {
  const modifiers = new Set<string>();
  if (event.ctrlKey) modifiers.add("Ctrl");
  if (event.metaKey) modifiers.add("Cmd");
  if (event.altKey) modifiers.add("Alt");
  if (event.shiftKey) modifiers.add("Shift");
  const key = event.key;
  return canonical(modifiers, key, key.length === 1 ? key.toUpperCase() : (NAMED_KEYS.get(key.toLowerCase()) ?? key));
}

/** `F5` and `Mod` combinations: the editor's even while the game captures live input. */
export function isEditorKey(event: KeyboardEvent): boolean {
  return event.key === "F5" || (isMac() ? event.metaKey : event.ctrlKey);
}

const NON_TEXT_INPUTS = new Set(["button", "checkbox", "color", "file", "image", "radio", "range", "reset", "submit"]);

export function isTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  return target instanceof HTMLInputElement && !NON_TEXT_INPUTS.has(target.type);
}

/**
 * Runs registered shortcuts on key presses in `target`, except while a text field has focus.
 * While `editor.shortcuts.captured`, every key but `F5` and `Mod` combinations is the game's.
 */
export function installShortcuts(editor: Editor, registry: Registry, target: Window = window): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.defaultPrevented || isTextField(event.target)) return;
    const shortcut = shortcutOf(event);
    if (editor.shortcuts.captured && !isEditorKey(event)) return;
    const command = registry.shortcuts.get(shortcut);
    if (!command) return;
    event.preventDefault();
    editor.commands.run(command).catch((error) => console.error(`command "${command}" failed:`, error));
  };
  target.addEventListener("keydown", onKeyDown);
  return () => target.removeEventListener("keydown", onKeyDown);
}
