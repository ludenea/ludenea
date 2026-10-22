import type { AssetId, TextureMeta } from "@ludenea/control-client";
import { textureOf, type Editor, type TextureFrames } from "@ludenea/editor-shell";
import { useEffect, useState, useSyncExternalStore } from "react";
import { errorText } from "../shared";

/** A texture as the inspector shows it: its file, its meta, its frames, and its PNG as a URL an image takes. */
export interface ShownTexture {
  path: string;
  meta: TextureMeta;
  frames: TextureFrames;
  url: string;
}

export type TextureState = { loading: true } | { error: string } | { texture: ShownTexture };

/** How much of the panel's width a preview fills at most, in pixels, zoomed by a whole factor. */
const PREVIEW_PIXELS = 256;

function dataUrl(png: Uint8Array): string {
  let binary = "";
  // In chunks: one call with every byte as an argument overflows the stack for a large image.
  for (let start = 0; start < png.length; start += 0x8000) binary += String.fromCharCode(...png.subarray(start, start + 0x8000));
  return `data:image/png;base64,${btoa(binary)}`;
}

/**
 * The texture `id`, read again whenever the project's textures change (`editor.textures`);
 * the error says why it cannot be shown, naming its file.
 */
export function useTexture(editor: Editor, id: AssetId): TextureState {
  const textures = useSyncExternalStore(editor.textures.subscribe, editor.textures.list);
  const [shown, setShown] = useState<{ id: AssetId; state: TextureState } | null>(null);

  useEffect(() => {
    let current = true;
    const show = (state: TextureState) => current && setShown({ id, state });
    const load = async (): Promise<TextureState> => {
      const { entries } = await editor.project.request({ type: "list_files" });
      const entry = entries.find((file) => file.id === id && textureOf(file) !== null);
      const texture = entry && textureOf(entry);
      if (!texture) return { error: `No texture of the project has the identifier ${id}.` };
      if (texture === "unreadable") return { error: "The texture's meta cannot be read: the console shows the project's warning, which names the file and the cause." };
      try {
        const [frames, png] = await Promise.all([editor.textures.frames(texture.path), editor.textures.bytes(texture.path)]);
        return { texture: { path: texture.path, meta: texture.meta, frames, url: dataUrl(png) } };
      } catch (error) {
        return { error: `${texture.path}: ${errorText(error)}` };
      }
    };
    load().then(show, (error: unknown) => show({ error: errorText(error) }));
    return () => {
      current = false;
    };
  }, [editor, id, textures]);

  // Another texture's state is not this one's: until it loads, this one is loading.
  return shown?.id === id ? shown.state : { loading: true };
}

/** The texture `id`: its image zoomed by a whole factor with nearest filtering, each frame outlined and numbered, and each pivot marked. */
export function TexturePreview({ editor, id }: { editor: Editor; id: AssetId }) {
  const state = useTexture(editor, id);
  if ("loading" in state) return <p className="ludenea-muted">Loading the texture…</p>;
  if ("error" in state) return <p role="alert" className="ludenea-error">{state.error}</p>;

  const { path, meta, url, frames } = state.texture;
  const { width, height } = frames;
  const zoom = Math.max(1, Math.floor(PREVIEW_PIXELS / Math.max(width, height, 1)));
  const count = frames.frames.length;
  return (
    <section aria-label={`Texture ${path}`} className="ludenea-component ludenea-texture">
      <h3 title={path}>{path.slice(path.lastIndexOf("/") + 1)}</h3>
      <p className="ludenea-muted">{`${width} × ${height} pixels, ${count} ${count === 1 ? "frame" : "frames"}, ${meta.pixels_per_unit} pixels per unit`}</p>
      <div className="ludenea-texture-preview">
        <svg role="img" aria-label={path} width={width * zoom} height={height * zoom} viewBox={`0 0 ${width} ${height}`}>
          <image href={url} width={width} height={height} style={{ imageRendering: "pixelated" }} />
          {frames.frames.map((frame, index) => (
            <g key={index} aria-label={`Frame ${index}`} className="ludenea-texture-frame">
              <rect x={frame.x} y={frame.y} width={frame.width} height={frame.height} vectorEffect="non-scaling-stroke" />
              <text x={frame.x + 2 / zoom} y={frame.y + 2 / zoom} fontSize={10 / zoom} dominantBaseline="hanging">
                {index}
              </text>
              <circle aria-label={`Pivot of frame ${index}`} className="ludenea-texture-pivot" cx={frame.x + frame.pivot[0]} cy={frame.y + frame.pivot[1]} r={3 / zoom} vectorEffect="non-scaling-stroke" />
            </g>
          ))}
        </svg>
      </div>
    </section>
  );
}
