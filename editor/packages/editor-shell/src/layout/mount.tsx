import { createRoot } from "react-dom/client";
import type { EditorHost } from "../editor";
import { isPluginBoundary } from "./PluginBoundary";
import { Shell } from "./Shell";

const CONTEXT = "Rendering the editor";

/**
 * Renders the shell into `element`; returns the unmount. `react-dom/client` lives in the shared
 * shell module and uses the shared `react-dom`, the one the dock's portals and the built-ins use.
 * Render errors go to the editor console: a `PluginBoundary` reports its own, and one that no
 * boundary catches replaces the window with its message.
 */
export function mountShell(element: Element, host: EditorHost): () => void {
  const { console: editorConsole } = host.editor;
  const root = createRoot(element, {
    onCaughtError(error, { errorBoundary }) {
      if (!isPluginBoundary(errorBoundary)) editorConsole.report(error, CONTEXT);
    },
    onUncaughtError(error) {
      const text = editorConsole.report(error, CONTEXT);
      root.render(
        <div role="alert" className="ludenea-crash">
          <p>{text}</p>
          <p>Reload the editor to continue.</p>
        </div>,
      );
    },
  });
  root.render(<Shell host={host} />);
  return () => root.unmount();
}
