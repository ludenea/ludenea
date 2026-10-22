import type { AssetId } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useStore } from "../shared";
import { useTexture } from "../textures/TexturePreview";
import type { TilemapState } from ".";

/** How large a tile button shows its frame, in pixels. */
const TILE_PIXELS = 32;

/** The selected tilemap's tileset frames as buttons: the one pressed is the tile the paint tool paints. */
export function TilePalette({ editor, state }: { editor: Editor; state: TilemapState }) {
  const { target, tile } = useStore(state);
  if (!target) return <p className="ludenea-muted">Select one entity with a ludenea::Tilemap and a ludenea::Pose to paint it.</p>;
  return <Tiles editor={editor} tileset={target.tilemap.tileset} tile={tile} choose={(tile) => state.set({ tile })} />;
}

function Tiles({ editor, tileset, tile, choose }: { editor: Editor; tileset: AssetId; tile: number | null; choose(tile: number): void }) {
  const texture = useTexture(editor, tileset);
  if ("loading" in texture) return <p className="ludenea-muted">Loading the tileset…</p>;
  if ("error" in texture) return <p role="alert" className="ludenea-error">{texture.error}</p>;

  const { path, url, frames } = texture.texture;
  return (
    <section aria-label="Tile palette" className="ludenea-tile-palette">
      <p className="ludenea-muted">
        {path}: B paints the chosen tile, E erases; a right drag erases too. The game must be paused.
      </p>
      <div role="group" aria-label={`Tiles of ${path}`} className="ludenea-tiles">
        {frames.frames.map((frame, index) => (
          <button key={index} aria-label={`Tile ${index}`} aria-pressed={tile === index} title={`Tile ${index}`} onClick={() => choose(index)}>
            <svg width={TILE_PIXELS} height={TILE_PIXELS} viewBox={`${frame.x} ${frame.y} ${frame.width} ${frame.height}`} aria-hidden="true">
              <image href={url} width={frames.width} height={frames.height} style={{ imageRendering: "pixelated" }} />
            </svg>
          </button>
        ))}
      </div>
    </section>
  );
}
