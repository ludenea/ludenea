import { ProtocolFailure, type AssetId, type ControlClient, type FileEntry, type JsonValue, type PathSegment, type ProjectClient, type TypeSchema } from "@ludenea/control-client";

/**
 * Stable asset type name to value type. Empty here; a project's generated `game.d.ts`
 * augments it from the game's schemas, as it does `ComponentTypes`.
 */
export interface AssetTypes {}

/** Typed access to data assets, over `GetAsset`, `SetAssetField`, `NewAssetValue`, and `CreateAsset`. */
export interface AssetAccess {
  /** The asset's value; fails when it has no exact JSON form (read RON through `control`). */
  get<N extends keyof AssetTypes>(id: AssetId): Promise<AssetTypes[N]>;
  setField(id: AssetId, path: PathSegment[], value: unknown): Promise<void>;
  /**
   * Creates the data asset file `path` holding the default value of `type`; a type without a
   * default value needs one from the user, edited from its schema.
   */
  create(type: string, path: string): Promise<{ kind: "created"; entry: FileEntry } | { kind: "needs-value"; schema: TypeSchema }>;
}

/** A host failure about an asset, located in the asset's file. */
export class AssetFailure extends ProtocolFailure {
  constructor(
    code: ProtocolFailure["code"],
    message: string,
    readonly path: string,
    readonly line: number,
    readonly column: number,
  ) {
    super(code, `${path}:${line}:${column}: ${message}`);
    this.name = "AssetFailure";
  }
}

// The host names an asset by identifier (it knows no paths), then the position of the error.
const INVALID_ASSET = /asset ([0-9a-f]{32})(?: \([^)]*\))? is invalid: (\d+):(\d+)/;

/** `error` as an `AssetFailure` when it names an asset of `paths` and a position, else unchanged. */
export function locateAssetError(error: unknown, paths: [AssetId, string][]): unknown {
  if (!(error instanceof ProtocolFailure)) return error;
  const match = INVALID_ASSET.exec(error.message);
  const path = match && paths.find(([id]) => id === match[1])?.[1];
  if (!match || !path) return error;
  return new AssetFailure(error.code, error.message, path, Number(match[2]), Number(match[3]));
}

export function createAssetAccess(control: () => ControlClient | null, project: ProjectClient): AssetAccess {
  const game = () => {
    const current = control();
    if (!current) throw new Error("no game is connected");
    return current;
  };
  return {
    async get(id) {
      const { value } = await game().request({ type: "get_asset", id });
      if (value.type !== "json") throw new Error(`asset ${id} has no JSON form; read its RON through the control client`);
      return value.value as never;
    },
    async setField(id, path, value) {
      await game().request({ type: "set_asset_field", id, path, value: value as JsonValue });
    },
    async create(type, path) {
      const current = game();
      let text: string;
      try {
        ({ text } = await current.request({ type: "new_asset_value", asset: type }));
      } catch (error) {
        // How the host refuses a type registered without a default value.
        const schema = current.game.assets.find((asset) => asset.name === type)?.schema;
        if (schema && error instanceof ProtocolFailure && error.message.includes("has no default value")) return { kind: "needs-value", schema };
        throw error;
      }
      const { type: _, ...entry } = await project.request({ type: "create_asset", path, kind: { Data: { type } }, text });
      return { kind: "created", entry };
    },
  };
}
