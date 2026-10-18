// `@ludenea/editor-shell`: the API plugins extend the editor through. The applications also
// import `@ludenea/editor-shell/app`.
export type { Editor } from "./editor";
export type { ComponentTypes, EntityAccess } from "./entities";
export {
  definePlugin,
  type CommandDefinition,
  type Disposable,
  type InspectorComponent,
  type OverlayContext,
  type PanelDefinition,
  type PluginDefinition,
  type SettingDefinition,
  type ViewportOverlay,
} from "./plugin";
export { DuplicateRegistration } from "./registry";
export { isEditorKey, isTextField, normalizeShortcut } from "./shortcuts";
export type { SelectionState, SelectionStore } from "./stores/selection";
export type { TimelineState, TimelineStore } from "./stores/timeline";
