import { Component, type ReactNode } from "react";
import { failureText } from "../console";
import type { Editor } from "../editor";

/** What a `PluginBoundary` caught: the line the editor console shows, and how to render the slot again. */
export interface PluginFailure {
  text: string;
  retry(): void;
}

export interface PluginBoundaryProps {
  editor: Editor;
  /** What the slot renders, as the plugin registered it: a panel definition, an inspector, a decoration, or a column. */
  registration: object;
  /** The slot in a sentence, such as `The "Stats" panel`; the plugin's name follows it. */
  name: string;
  /** Shown instead of the slot once it threw; by default the failure and a "Reload panel" button. */
  fallback?(failure: PluginFailure): ReactNode;
  /**
   * What the slot shows, such as the entity and the component a decoration marks: once the slot
   * threw, a change to any of them (compared with `Object.is`) renders it again. A slot that did
   * not throw keeps its state.
   */
  resetKeys?: readonly unknown[];
  children?: ReactNode;
}

interface State {
  /** Wrapped: a plugin may throw `undefined`. */
  failed: { thrown: unknown } | null;
  registration: object;
  resetKeys: readonly unknown[];
}

const reloadPanel = ({ text, retry }: PluginFailure) => (
  <div role="alert" className="ludenea-plugin-failure">
    <p>{text}</p>
    <button onClick={retry}>Reload panel</button>
  </div>
);

// The registrations whose failure each editor reported: one line however many slots render it.
const reported = new WeakMap<Editor, WeakSet<object>>();

const changed = (keys: readonly unknown[], previous: readonly unknown[]) => keys.length !== previous.length || keys.some((key, at) => !Object.is(key, previous[at]));

/** The error boundary behind `PluginBoundary`. */
class Boundary extends Component<PluginBoundaryProps, State> {
  state: State = { failed: null, registration: this.props.registration, resetKeys: this.props.resetKeys ?? [] };

  static getDerivedStateFromError(thrown: unknown): Partial<State> {
    return { failed: { thrown } };
  }

  static getDerivedStateFromProps({ registration, resetKeys = [] }: PluginBoundaryProps, state: State): Partial<State> | null {
    return registration === state.registration && !changed(resetKeys, state.resetKeys) ? null : { failed: null, registration, resetKeys };
  }

  componentDidCatch(thrown: unknown): void {
    const { editor, registration } = this.props;
    let seen = reported.get(editor);
    if (!seen) reported.set(editor, (seen = new WeakSet()));
    if (seen.has(registration)) return;
    seen.add(registration);
    editor.console.report(thrown, this.slot());
  }

  render(): ReactNode {
    const { failed } = this.state;
    if (!failed) return this.props.children;
    return (this.props.fallback ?? reloadPanel)({ text: failureText(failed.thrown, this.slot()), retry: this.retry });
  }

  private slot(): string {
    const { editor, registration, name } = this.props;
    const plugin = editor.pluginOf(registration);
    return plugin === undefined ? name : `${name} of plugin "${plugin}"`;
  }

  private retry = () => this.setState({ failed: null });
}

/**
 * Renders plugin code so that what throws while rendering costs only its own slot: the slot
 * shows `fallback` instead, and the editor console gets the failure once per registration.
 * Another registration (the plugin reloaded) or a change to `resetKeys` renders the slot again.
 */
export function PluginBoundary(props: PluginBoundaryProps): ReactNode {
  return <Boundary {...props} />;
}

/** Whether React's `errorBoundary` for a caught error is a `PluginBoundary`, which reports it. */
export const isPluginBoundary = (errorBoundary: unknown): boolean => errorBoundary instanceof Boundary;
