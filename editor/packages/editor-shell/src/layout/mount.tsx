import { createRoot } from "react-dom/client";
import type { EditorHost } from "../editor";
import { Shell } from "./Shell";

/**
 * Renders the shell into `element`; returns the unmount. `react-dom/client` lives in the shared
 * shell module and uses the shared `react-dom`, the one the dock's portals and the built-ins use.
 */
export function mountShell(element: Element, host: EditorHost): () => void {
  const root = createRoot(element);
  root.render(<Shell host={host} />);
  return () => root.unmount();
}
