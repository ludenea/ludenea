import { DockviewReact, type DockviewApi, type IDockviewPanelProps } from "dockview-react";
import { createContext, useContext, useEffect, useRef, useSyncExternalStore } from "react";
import type { EditorHost } from "../editor";
import { CommandPalette } from "../palette/CommandPalette";
import type { Disposable, PanelDefinition } from "../plugin";
import type { Registry } from "../registry";
import { installShortcuts } from "../shortcuts";

const HostContext = createContext<EditorHost | null>(null);

/**
 * Dockview keeps a hidden tab's component mounted; a plugin panel is unmounted instead, so its
 * subscriptions (and the host's recording behind them) last only while it shows.
 */
function PluginPanel({ params, api }: IDockviewPanelProps<{ id: string }>) {
  const host = useContext(HostContext)!;
  const panels = host.registry.panels;
  const panel = useSyncExternalStore(panels.subscribe, () => panels.get(params.id));
  const visible = useSyncExternalStore(
    (listener) => {
      const subscription = api.onDidVisibilityChange(listener);
      return () => subscription.dispose();
    },
    () => api.isVisible,
  );
  return panel && visible ? <panel.component editor={host.editor} /> : null;
}

const COMPONENTS = { plugin: PluginPanel };
const ORDER = ["center", "left", "right", "bottom"] as const;
const DIRECTIONS = { left: "left", right: "right", bottom: "below" } as const;

/**
 * Makes the dock match the registered panels: removed ones close, new ones open at their
 * default position, except those the user closed (reopened through their `panels.open.<id>` command).
 */
function syncPanels(api: DockviewApi, registry: Registry, closed: ReadonlySet<string>): void {
  const position = (panel: PanelDefinition) => panel.defaultPosition ?? "center";
  for (const open of [...api.panels]) {
    const panel = registry.panels.get(open.id);
    if (!panel) open.api.close();
    else if (open.title !== panel.title) open.api.setTitle(panel.title);
  }
  const added = registry.panels.values().filter((panel) => !api.getPanel(panel.id) && !closed.has(panel.id));
  added.sort((a, b) => ORDER.indexOf(position(a)) - ORDER.indexOf(position(b)));
  for (const panel of added) {
    const where = position(panel);
    const sibling = registry.panels.values().find((other) => other !== panel && position(other) === where && api.getPanel(other.id));
    api.addPanel({
      id: panel.id,
      component: "plugin",
      title: panel.title,
      params: { id: panel.id },
      position: sibling ? { referencePanel: sibling.id, direction: "within" } : where === "center" ? undefined : { direction: DIRECTIONS[where] },
    });
  }
}

/**
 * The editor window: registered panels in a dockview layout, the command palette, and global
 * shortcuts. The application imports `dockview-react/dist/styles/dockview.css` and the shell's `src/theme.css`.
 */
export function Shell({ host }: { host: EditorHost }) {
  const stop = useRef<() => void>(undefined);
  useEffect(() => installShortcuts(host.editor, host.registry), [host]);
  useEffect(() => () => stop.current?.(), []);

  const onReady = ({ api }: { api: DockviewApi }) => {
    const { editor, registry } = host;
    const closed = new Set<string>();
    const openCommands = new Map<string, { title: string; command: Disposable }>();
    // Panels whose open command another plugin holds, reported once per conflict.
    const taken = new Set<string>();
    const sync = () => {
      // A panel unregistered while closed opens again if a plugin registers it later.
      for (const id of closed) if (!registry.panels.get(id)) closed.delete(id);
      syncPanels(api, registry, closed);
      // One palette command per panel, so a closed panel has a keyboard path back.
      for (const [id, { command }] of openCommands) {
        if (registry.panels.get(id)) continue;
        command.dispose();
        openCommands.delete(id);
      }
      for (const { id, title } of registry.panels.values()) {
        const existing = openCommands.get(id);
        if (existing?.title === title) continue;
        existing?.command.dispose();
        const run = () => {
          closed.delete(id);
          sync();
          api.getPanel(id)?.api.setActive();
        };
        // Another plugin holding the id must not stop the sync (it runs in a microtask).
        try {
          openCommands.set(id, { title, command: editor.commands.register({ id: `panels.open.${id}`, title: `Show ${title} panel`, run }) });
          taken.delete(id);
        } catch (error) {
          openCommands.delete(id);
          if (taken.has(id)) continue;
          taken.add(id);
          editor.console.report(error, `Adding the command that shows panel "${id}"`);
        }
      }
    };
    // A reload disposes and re-registers in one synchronous step; syncing after it keeps the layout.
    let scheduled = false;
    const schedule = () => {
      if (scheduled) return;
      scheduled = true;
      queueMicrotask(() => {
        scheduled = false;
        sync();
      });
    };
    stop.current?.();
    const removed = api.onDidRemovePanel((panel) => {
      if (registry.panels.get(panel.id)) closed.add(panel.id);
    });
    const unsubscribe = registry.panels.subscribe(schedule);
    stop.current = () => {
      unsubscribe();
      removed.dispose();
      for (const { command } of openCommands.values()) command.dispose();
    };
    sync();
  };

  return (
    <HostContext.Provider value={host}>
      <DockviewReact components={COMPONENTS} onReady={onReady} className="dockview-theme-abyss ludenea-dock" />
      <CommandPalette editor={host.editor} registry={host.registry} />
    </HostContext.Provider>
  );
}
