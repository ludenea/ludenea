import type { ControlClient, DebugDrawFrame, Vec2 } from "@ludenea/control-client";
import { definePlugin, type Disposable } from "@ludenea/editor-shell";
import { createStore } from "../shared";
import { bounds, panBy, zoomAt, MAX_PIXELS_PER_METER, MIN_PIXELS_PER_METER, type Camera, type Size } from "./camera";
import { moveBy, visibleLayers, type ViewportState } from "./state";
import { ViewportPanel } from "./ViewportPanel";

const PAN_PIXELS = 64;
const ZOOM_STEP = 1.25;
const NUDGE_METERS = 0.1;
const DIRECTIONS = { left: { x: -1, y: 0 }, right: { x: 1, y: 0 }, up: { x: 0, y: 1 }, down: { x: 0, y: -1 } } as const;
const ARROWS = { left: "ArrowLeft", right: "ArrowRight", up: "ArrowUp", down: "ArrowDown" } as const;

export const viewportPlugin = definePlugin({
  name: "ludenea.viewport",
  activate(editor) {
    const state: ViewportState = createStore({
      camera: { center: { x: 0, y: 0 }, pixelsPerMeter: 50 } as Camera,
      size: { width: 0, height: 0 } as Size,
      hidden: [] as string[],
      frame: null as DebugDrawFrame | null,
      tick: 0,
      revision: 0,
    });
    const camera = (change: (camera: Camera, size: Size) => Camera) => {
      const { camera, size } = state.get();
      state.set({ camera: change(camera, size) });
    };
    const center = (size: Size) => ({ x: size.width / 2, y: size.height / 2 });

    for (const [direction, { x, y }] of Object.entries(DIRECTIONS) as [keyof typeof DIRECTIONS, Vec2][]) {
      editor.commands.register({
        id: `viewport.pan-${direction}`,
        title: `Pan the viewport ${direction}`,
        run: () => camera((current) => panBy(current, -x * PAN_PIXELS, y * PAN_PIXELS)),
      });
      editor.shortcuts.register({ key: `Shift+${ARROWS[direction]}`, command: `viewport.pan-${direction}` });
      editor.commands.register({
        id: `viewport.nudge-${direction}`,
        title: `Nudge the selection ${direction}`,
        async run() {
          const control = editor.control;
          if (!control) throw new Error("no game is running");
          // While playing, the arrows are the game's input: the same rule from the palette.
          if (editor.timeline.getState().playing) throw new Error("pause the game to nudge the selection");
          const offset = { x: x * NUDGE_METERS, y: y * NUDGE_METERS };
          await Promise.all(editor.selection.getState().entities.map((entity) => moveBy(editor, control, entity, offset, state)));
        },
      });
    }
    editor.commands.register({
      id: "viewport.zoom-in",
      title: "Zoom the viewport in",
      run: () => camera((current, size) => zoomAt(current, size, center(size), ZOOM_STEP)),
    });
    editor.commands.register({
      id: "viewport.zoom-out",
      title: "Zoom the viewport out",
      run: () => camera((current, size) => zoomAt(current, size, center(size), 1 / ZOOM_STEP)),
    });
    editor.commands.register({
      id: "viewport.frame-selection",
      title: "Frame the selection in the viewport",
      run() {
        const { frame, size } = state.get();
        const box = frame && bounds(frame, visibleLayers(editor.control, state), editor.selection.getState().entities);
        // Nothing drawn to frame (no selection, or not in a shown layer): the view stays.
        if (!box) return;
        // A fifth of the view as margin on each side.
        const fit = Math.min(size.width / Math.max(box.max.x - box.min.x, 0.5), size.height / Math.max(box.max.y - box.min.y, 0.5)) * 0.6;
        state.set({
          camera: {
            center: { x: (box.min.x + box.max.x) / 2, y: (box.min.y + box.max.y) / 2 },
            pixelsPerMeter: Math.min(MAX_PIXELS_PER_METER, Math.max(MIN_PIXELS_PER_METER, fit)),
          },
        });
      },
    });
    editor.shortcuts.register({ key: "+", command: "viewport.zoom-in" });
    editor.shortcuts.register({ key: "-", command: "viewport.zoom-out" });
    editor.shortcuts.register({ key: "F", command: "viewport.frame-selection" });

    // One toggle per layer the running game declares: the only way to show or hide a layer.
    let toggles: Disposable[] = [];
    const registerToggles = (control: ControlClient | null) => {
      toggles.forEach((toggle) => toggle.dispose());
      toggles = (control?.game.debug_layers ?? []).map((layer) =>
        editor.commands.register({
          id: `viewport.toggle-layer.${layer}`,
          title: `Toggle the "${layer}" debug layer`,
          run() {
            const { hidden } = state.get();
            state.set({ hidden: hidden.includes(layer) ? hidden.filter((name) => name !== layer) : [...hidden, layer] });
          },
        }),
      );
    };
    registerToggles(editor.control);
    editor.onGameChanged(registerToggles);

    editor.panels.register({
      id: "ludenea.viewport",
      title: "Viewport",
      defaultPosition: "center",
      component: ({ editor }) => <ViewportPanel editor={editor} state={state} />,
    });
    return () => toggles.forEach((toggle) => toggle.dispose());
  },
});
