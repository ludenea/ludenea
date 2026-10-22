// `@ludenea/editor-shell`: the API plugins extend the editor through. The applications also
// import `@ludenea/editor-shell/app`.
export { runReported, type ConsoleEntry, type ConsoleLocation, type EditorConsole } from "./console";
export type { Editor, Observed } from "./editor";
export { AssetFailure, type AssetAccess, type AssetTypes } from "./assets";
export type { ComponentTypes, EntityAccess } from "./entities";
export {
  definePlugin,
  type AssetInspector,
  type AssetKindName,
  type CommandDefinition,
  type Disposable,
  type ExplorerColumn,
  type FieldDecoration,
  type InspectorComponent,
  type OverlayContext,
  type PanelDefinition,
  type PluginDefinition,
  type SettingDefinition,
  type TextureCut,
  type TextureFrames,
  type TextureInfo,
  type ViewportOverlay,
} from "./plugin";
export { PluginBoundary, type PluginBoundaryProps, type PluginFailure } from "./layout/PluginBoundary";
export { DuplicateRegistration } from "./registry";
export { isEditorKey, isTextField, normalizeShortcut } from "./shortcuts";
export type { BranchState, BranchStore } from "./stores/branches";
export type { DocumentState, DocumentStore, Loaded, OpenDocument } from "./stores/document";
export type { SelectionState, SelectionStore } from "./stores/selection";
export { textureOf, type TextureAccess } from "./textures";
export type { TimelineState, TimelineStore } from "./stores/timeline";
