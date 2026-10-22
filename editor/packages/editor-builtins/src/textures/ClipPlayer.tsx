import type { AssetId } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import { useEffect, useState } from "react";
import { useGame } from "../shared";
import { useTexture, type ShownTexture } from "./TexturePreview";

/** A clip of a `ludenea::SpriteSheet`: `count` frames from `first`, each shown for `ticks_per_frame` ticks. */
export interface Clip {
  first: number;
  count: number;
  ticks_per_frame: number;
}

/** A `ludenea::SpriteSheet` value, as the game gives it in JSON. */
interface SpriteSheet {
  texture: AssetId;
  clips: { [name: string]: Clip };
}

/** How many screen pixels a clip's frame takes at most, zoomed by a whole factor. */
const FRAME_PIXELS = 96;

const isSheet = (value: unknown): value is SpriteSheet =>
  typeof value === "object" && value !== null && typeof (value as SpriteSheet).texture === "string" && typeof (value as SpriteSheet).clips === "object" && (value as SpriteSheet).clips !== null;

/** The inspector of a `ludenea::SpriteSheet`: a player for each of its clips, over the frames of its texture. */
export function SpriteSheetClips({ editor, value }: { editor: Editor; value: unknown }) {
  if (!isSheet(value)) return <p role="alert" className="ludenea-error">This sprite sheet's value has no texture and clips to play.</p>;
  return <Clips editor={editor} sheet={value} />;
}

function Clips({ editor, sheet }: { editor: Editor; sheet: SpriteSheet }) {
  const control = useGame(editor);
  const texture = useTexture(editor, sheet.texture);
  // A clip plays at the game's tick rate.
  if (!control) return <p className="ludenea-muted">No game is running.</p>;
  const names = Object.keys(sheet.clips);
  if (names.length === 0) return <p className="ludenea-muted">This sprite sheet has no clips.</p>;
  if ("loading" in texture) return <p className="ludenea-muted">Loading the texture…</p>;
  if ("error" in texture) return <p role="alert" className="ludenea-error">{texture.error}</p>;
  return (
    <div className="ludenea-clips">
      {names.map((name) => (
        <ClipPlayer key={name} name={name} clip={sheet.clips[name]!} texture={texture.texture} tickRate={control.game.tick_rate} />
      ))}
    </div>
  );
}

const frameRange = (count: number) => (count === 1 ? "1 frame (0)" : `${count} frames (0 to ${count - 1})`);

/**
 * Plays `clip` over `texture`'s frames, looping, one frame every `ticks_per_frame` ticks at
 * `tickRate` ticks per second, as the game's `animate` system does. A clip reaching past the
 * texture's frames says so and does not play.
 */
export function ClipPlayer({ name, clip, texture, tickRate }: { name: string; clip: Clip; texture: ShownTexture; tickRate: number }) {
  const [playing, setPlaying] = useState(false);
  const [step, setStep] = useState(0);
  const { frames } = texture.frames;
  const last = clip.first + clip.count - 1;
  const plays = clip.count >= 1 && clip.ticks_per_frame >= 1 && last < frames.length;

  useEffect(() => {
    if (!playing || !plays) return;
    const timer = setInterval(() => setStep((step) => step + 1), (clip.ticks_per_frame * 1000) / tickRate);
    return () => clearInterval(timer);
  }, [playing, plays, clip.ticks_per_frame, tickRate]);

  if (!plays) {
    // The game refuses a clip without frames or ticks per frame: it shows nothing to play either.
    const why = last < frames.length ? "The clip needs at least one frame and one tick per frame." : `The clip shows frames ${clip.first} to ${last}, but ${texture.path} has ${frameRange(frames.length)}.`;
    return (
      <section aria-label={`Clip ${name}`} className="ludenea-clip">
        <h4>{name}</h4>
        <p role="alert" className="ludenea-error">
          {why}
        </p>
      </section>
    );
  }

  const index = clip.first + (step % clip.count);
  const frame = frames[index]!;
  const zoom = Math.max(1, Math.floor(FRAME_PIXELS / Math.max(frame.width, frame.height, 1)));
  return (
    <section aria-label={`Clip ${name}`} className="ludenea-clip">
      <h4>{name}</h4>
      <svg role="img" aria-label={`Frame ${index} of ${name}`} width={frame.width * zoom} height={frame.height * zoom} viewBox={`${frame.x} ${frame.y} ${frame.width} ${frame.height}`}>
        <image href={texture.url} width={texture.frames.width} height={texture.frames.height} style={{ imageRendering: "pixelated" }} />
      </svg>
      <p>
        <button onClick={() => setPlaying(!playing)}>{playing ? "Pause" : "Play"}</button> Frame <output aria-label="Frame">{index}</output>, of {clip.first} to {last}, every {clip.ticks_per_frame} {clip.ticks_per_frame === 1 ? "tick" : "ticks"}
      </p>
    </section>
  );
}
