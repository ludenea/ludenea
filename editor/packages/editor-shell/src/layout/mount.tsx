import { createRoot } from "react-dom/client";
import type { EditorHost } from "../editor";
import { Shell } from "./Shell";

/**
 * Renders the shell into `element`; returns the unmount. The applications mount through this so
 * the one `react-dom` lives in the shared shell module, next to the dock's own portals.
 */
export function mountShell(element: Element, host: EditorHost): () => void {
  const root = createRoot(element);
  root.render(<Shell host={host} />);
  return () => root.unmount();
}
