import { DockviewReact, type DockviewApi, type IDockviewPanelProps } from "dockview-react";
import { createContext, useContext, useEffect, useRef, useSyncExternalStore } from "react";
import type { EditorHost } from "../editor";
import { CommandPalette } from "../palette/CommandPalette";
import type { PanelDefinition } from "../plugin";
import type { Registry } from "../registry";
import { installShortcuts } from "../shortcuts";

const HostContext = createContext<EditorHost | null>(null);

function PluginPanel({ params }: IDockviewPanelProps<{ id: string }>) {
  const host = useContext(HostContext)!;
  const panels = host.registry.panels;
  const panel = useSyncExternalStore(panels.subscribe, () => panels.get(params.id));
  return panel ? <panel.component editor={host.editor} /> : null;
}

const COMPONENTS = { plugin: PluginPanel };
const ORDER = ["center", "left", "right", "bottom"] as const;
const DIRECTIONS = { left: "left", right: "right", bottom: "below" } as const;

/** Makes the dock match the registered panels: removed ones close, new ones open at their default position. */
function syncPanels(api: DockviewApi, registry: Registry): void {
  const position = (panel: PanelDefinition) => panel.defaultPosition ?? "center";
  for (const open of [...api.panels]) {
    const panel = registry.panels.get(open.id);
    if (!panel) open.api.close();
    else if (open.title !== panel.title) open.api.setTitle(panel.title);
  }
  const added = registry.panels.values().filter((panel) => !api.getPanel(panel.id));
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
 * shortcuts. The application imports `dockview-react/dist/styles/dockview.css`.
 */
export function Shell({ host }: { host: EditorHost }) {
  const stop = useRef<() => void>(undefined);
  useEffect(() => installShortcuts(host.editor, host.registry), [host]);
  useEffect(() => () => stop.current?.(), []);

  const onReady = ({ api }: { api: DockviewApi }) => {
    // A reload disposes and re-registers in one synchronous step; syncing after it keeps the layout.
    let scheduled = false;
    const schedule = () => {
      if (scheduled) return;
      scheduled = true;
      queueMicrotask(() => {
        scheduled = false;
        syncPanels(api, host.registry);
      });
    };
    stop.current?.();
    stop.current = host.registry.panels.subscribe(schedule);
    syncPanels(api, host.registry);
  };

  return (
    <HostContext.Provider value={host}>
      <DockviewReact components={COMPONENTS} onReady={onReady} className="dockview-theme-abyss" />
      <CommandPalette editor={host.editor} />
    </HostContext.Provider>
  );
}
