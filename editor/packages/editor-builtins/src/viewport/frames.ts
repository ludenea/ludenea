import type { ControlClient, DebugDrawFrame, Entity, RenderFrame, RenderFramesEvent } from "@ludenea/control-client";
import type { Disposable, Editor } from "@ludenea/editor-shell";
import { reportedOnce } from "../shared";

/** A render frame to push: postcard bytes as play pushes them, or JSON text as fetched while paused. */
export type Frame = { json?: string; bytes?: Uint8Array };

/** What frames are pushed into: the renderer (`WebRenderer`). */
export interface FrameTarget {
  push_frame(json: string, overlay: string): string;
  push_frame_bytes(bytes: Uint8Array, overlay: string): string;
}

/** What a push returns: the frame's tick, what is drawn as a placeholder, and its debug drawing. */
export type Pushed = { tick: number; missing: unknown[]; debug: DebugDrawFrame };

export interface FrameFeedOptions {
  /** The debug layers drawn, by name. */
  layers(): string[];
  /** The entities drawn highlighted. */
  selection(): readonly Entity[];
  /** Hears each frame pushed. */
  pushed(report: Pushed): void;
}

const TOPIC = { type: "render_frames" } as const;
/** What the viewport shows without a game. */
const EMPTY: RenderFrame = { tick: 0, camera: null, layers: [], sprites: [], tilemaps: [], teleported: [], debug: { layers: {} }, warnings: [] };

/**
 * The tick of a frame in the control protocol's postcard encoding: its first field, a `u64`,
 * which postcard writes as a LEB128 varint. Read without decoding the frame.
 */
export function tickOf(bytes: Uint8Array): number {
  let tick = 0;
  let scale = 1;
  for (const byte of bytes) {
    tick += (byte & 0x7f) * scale;
    if (byte < 0x80) return tick;
    scale *= 128;
  }
  throw new Error(`the ${bytes.length} bytes end inside the tick of a render frame`);
}

/**
 * Feeds the renderer the frames of the running game: while it plays, those the host pushes
 * (subscribed to `render_frames`); paused, the tick on screen, fetched after each change of
 * the timeline or of the layers. A frame or reply older than the newest frame pushed, or other
 * than the tick on screen while paused, is dropped.
 */
export class FrameFeed {
  private target: FrameTarget | null = null;
  /** The last two frames pushed, oldest first: pushed again in order, they blend as before. */
  private frames: Frame[] = [];
  /** The tick of the newest frame pushed; -1 once the timeline goes back, pauses, or moves while paused. */
  private newest = -1;
  private timing = { progress: 0, rate: 0, arrival: 0 };
  /** The game the feed subscribed to while it plays. */
  private following: ControlClient | null = null;
  /** The game a `get_render_frame` is in flight to, and whether the tick on screen changed since. */
  private fetching: ControlClient | null = null;
  private stale = false;
  private stopFrames = () => {};
  private readonly stops: (() => void)[];
  private readonly fetchFailures;
  private readonly followFailures;
  private readonly pushFailures;

  constructor(
    private readonly editor: Editor,
    private readonly options: FrameFeedOptions,
  ) {
    this.fetchFailures = reportedOnce(editor, "Fetching the render frame");
    this.followFailures = reportedOnce(editor, "Following the render frames");
    this.pushFailures = reportedOnce(editor, "Showing the render frame");
    const game: Disposable = editor.onGameChanged((control) => this.gameChanged(control));
    this.stops = [editor.timeline.subscribe((state, previous) => this.timelineChanged(state, previous)), () => game.dispose()];
    this.gameChanged(editor.control);
  }

  /** Pushes into `target` from now on, starting with the frames on screen. */
  setTarget(target: FrameTarget | null): void {
    this.target = target;
    this.again();
  }

  /** The newest frame pushed, which a new renderer starts from. */
  lastFrame(): Frame | null {
    return this.frames.at(-1) ?? null;
  }

  /** Pushes the frames on screen again, with the current overlay (after a selection change or new textures). */
  again(): void {
    for (const frame of this.frames) this.push(frame);
  }

  /** How far from the previous tick to the current one to draw at `now` (milliseconds, `performance.now()`). */
  alpha(now: number): number {
    if (!this.editor.timeline.getState().playing) return 1;
    const { progress, rate, arrival } = this.timing;
    return Math.min(1, Math.max(0, progress + ((now - arrival) / 1000) * rate));
  }

  /** The debug layers drawn changed: frames pushed from now on hold them, and paused, the tick on screen is fetched with them. */
  layersChanged(): void {
    const control = this.following;
    if (control) this.report(control, this.followFailures, control.request({ type: "set_pushed_layers", debug_layers: this.options.layers() }));
    else this.fetch();
  }

  /** The tick on screen changed in a way the timeline does not show (an edit's revision): fetched again while paused. */
  refresh(): void {
    if (!this.editor.timeline.getState().playing) this.fetch();
  }

  dispose(): void {
    this.stops.forEach((stop) => stop());
    this.stopFrames();
    this.unfollow();
    this.target = null;
  }

  private gameChanged(control: ControlClient | null): void {
    this.stopFrames();
    // The game left is gone: it is told nothing more.
    if (this.following !== control) this.following = null;
    this.fetchFailures.worked();
    this.followFailures.worked();
    this.frames = [];
    this.push({ json: JSON.stringify(EMPTY) });
    this.newest = -1;
    this.stopFrames = control?.on("render_frames", (event) => this.arrived(event)) ?? (() => {});
    if (this.editor.timeline.getState().playing) this.follow();
    else this.fetch();
  }

  private timelineChanged(state: TimelineView, previous: TimelineView): void {
    const [current, before] = [state.timeline?.current, previous.timeline?.current];
    // Back in time (a seek, an undo, another branch): older frames are the ones to show now.
    if (current !== undefined && before !== undefined && current < before) this.newest = -1;
    if (state.playing !== previous.playing) {
      if (state.playing) return this.follow();
      this.unfollow();
      // Paused, the tick on screen decides, even behind frames play pushed past it.
      this.newest = -1;
      return this.fetch();
    }
    if (state.playing) return;
    if (current !== before) this.newest = -1;
    if (current !== before || state.hash !== previous.hash || state.pending !== previous.pending || state.timeline?.branch !== previous.timeline?.branch) this.fetch();
  }

  private follow(): void {
    const control = this.editor.control;
    if (!control || this.following === control) return;
    this.following = control;
    // Sent in this order on one connection: the first frames pushed hold the layers.
    const layers = control.request({ type: "set_pushed_layers", debug_layers: this.options.layers() });
    const subscribed = control.request({ type: "subscribe", topic: TOPIC });
    this.report(control, this.followFailures, Promise.all([layers, subscribed]));
  }

  private unfollow(): void {
    const control = this.following;
    this.following = null;
    if (control && control === this.editor.control) this.report(control, this.followFailures, control.request({ type: "unsubscribe", topic: TOPIC }));
  }

  /** Reports `request`'s failure once until one works; a game being replaced refuses its last requests, which are not news. */
  private report(control: ControlClient, failures: ReturnType<typeof reportedOnce>, request: Promise<unknown>): void {
    request.then(
      () => failures.worked(),
      (error) => {
        if (this.editor.control === control) failures.failed(error);
      },
    );
  }

  /** Fetches the tick on screen; one request in flight at most, then one more if it changed meanwhile. */
  private fetch(): void {
    const control = this.editor.control;
    if (!control) return;
    if (this.fetching === control) {
      this.stale = true;
      return;
    }
    this.fetching = control;
    this.stale = false;
    control
      .request({ type: "get_render_frame", debug_layers: this.options.layers() })
      .then(
        ({ frame }) => {
          if (this.editor.control !== control) return;
          this.fetchFailures.worked();
          // Asked before a change (of layers, an edit, the tick): the request that follows answers.
          if (!this.stale) this.replied(frame);
        },
        (error) => {
          if (this.editor.control === control) this.fetchFailures.failed(error);
        },
      )
      .finally(() => {
        if (this.fetching !== control) return;
        this.fetching = null;
        if (this.stale) this.fetch();
      });
  }

  private replied(frame: RenderFrame): void {
    const { playing, timeline } = this.editor.timeline.getState();
    if (!playing && timeline && frame.tick !== timeline.current) return;
    this.show({ json: JSON.stringify(frame) }, frame.tick);
  }

  private arrived({ frames, progress }: RenderFramesEvent): void {
    const { playing, effectiveRate } = this.editor.timeline.getState();
    // What play pushed on its way to a pause: the tick on screen is fetched instead.
    if (!playing) return;
    let shown = false;
    for (const frame of frames) {
      let tick: number;
      try {
        tick = tickOf(frame);
      } catch (error) {
        this.pushFailures.failed(error);
        continue;
      }
      shown = this.show({ bytes: frame }, tick) || shown;
    }
    // The timing belongs to the frames on screen: an event dropped whole leaves it.
    if (shown) this.timing = { progress, rate: effectiveRate, arrival: performance.now() };
  }

  /** Shows `frame` unless it is older than the newest one shown; returns whether it did. */
  private show(frame: Frame, tick: number): boolean {
    if (tick < this.newest) return false;
    this.newest = tick;
    this.frames = [...this.frames.slice(-1), frame];
    this.push(frame);
    return true;
  }

  private push(frame: Frame): void {
    const target = this.target;
    if (!target) return;
    try {
      const overlay = JSON.stringify({ debug_layers: this.options.layers(), selected: this.options.selection() });
      const pushed = frame.bytes ? target.push_frame_bytes(frame.bytes, overlay) : target.push_frame(frame.json!, overlay);
      this.pushFailures.worked();
      this.options.pushed(JSON.parse(pushed) as Pushed);
    } catch (error) {
      this.pushFailures.failed(error);
    }
  }
}

type TimelineView = ReturnType<Editor["timeline"]["getState"]>;
