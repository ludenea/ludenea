import { ProtocolFailure, type ControlClient, type Entity, type LayerInfo } from "@ludenea/control-client";
import { definePlugin, type Disposable, type Editor, type ViewportTool } from "@ludenea/editor-shell";
import { createStore, type Store } from "../shared";
import { same } from "../viewport/camera";
import { areaBetween, cellAt, cellUnder, type Cell, type Pose, type Tilemap } from "./paint";
import { TilePalette } from "./TilePalette";

const TILEMAP = "ludenea::Tilemap";
const POSE = "ludenea::Pose";
const PAINT = "tilemap.paint";
const ERASE = "tilemap.erase";
const RIGHT_BUTTON = 2;

/** The selected tilemap as the tools and the palette read it: its entity, its components, and its layer's parallax. */
export interface PaintTarget {
  entity: Entity;
  tilemap: Tilemap;
  pose: Pose;
  parallax: number;
}

/** What the tools and the palette share: the selected tilemap (null unless exactly one is selected) and the chosen tile. */
export type TilemapState = Store<{ target: PaintTarget | null; tile: number | null }>;

/** A drag in progress: the cell it started on, the cell under the pointer, and what it paints (null erases). */
type Stroke = { start: Cell; end: Cell; tile: number | null };

/** The parallax of the layer named `name` (empty for the first one); 1 for a layer the game does not declare, as it is drawn nowhere. */
function parallaxOf(layers: LayerInfo[], name: string): number {
  const layer = name === "" ? layers[0] : layers.find((layer) => layer.name === name);
  return layer?.parallax ?? 1;
}

const PREVIEW = { paint: { fill: "rgba(64, 160, 255, 0.25)", stroke: "rgb(64, 160, 255)" }, erase: { fill: "rgba(255, 72, 72, 0.25)", stroke: "rgb(255, 72, 72)" } };

/** Paints and erases the selected tilemap in the viewport, with its tileset's frames in the Tile palette panel; on the public API only. */
export const tilemapPlugin = definePlugin({
  name: "ludenea.tilemap",
  activate(editor) {
    const state: TilemapState = createStore({ target: null as PaintTarget | null, tile: null as number | null });

    // The game's render layers, read once per game: it declares them as it is built, and they
    // do not change while it runs. A read that fails is tried again on the next selection.
    let layers: { control: ControlClient; read: Promise<LayerInfo[]> } | null = null;
    const layersOf = (control: ControlClient) => {
      if (layers?.control !== control) {
        const read = control.request({ type: "get_render_frame", debug_layers: [] }).then(({ frame }) => frame.layers);
        const entry = { control, read };
        layers = entry;
        read.catch(() => {
          if (layers === entry) layers = null;
        });
      }
      return layers.read;
    };

    // The selection is read again on every change; `loads` drops the answers of an older one.
    let loads = 0;
    let watch: Disposable | null = null;
    const load = () => {
      const number = ++loads;
      watch?.dispose();
      watch = null;
      if (state.get().target) state.set({ target: null });
      const control = editor.control;
      const { entities } = editor.selection.getState();
      const [entity] = entities;
      if (!control || entities.length !== 1 || !entity) return;
      const failed = (error: unknown) => {
        if (number === loads) editor.console.report(error, `Reading the selected entity ${entity.index}v${entity.generation} for tilemap painting`);
      };
      // Both components as the game last reported them, undefined until the entity is read; the
      // game's layers, once it has a tilemap.
      let tilemap: Tilemap | null | undefined;
      let pose: Pose | null | undefined;
      let declared: LayerInfo[] | null = null;
      let reading = false;
      const update = () => {
        if (number !== loads) return;
        if (tilemap && !declared && !reading) {
          reading = true;
          layersOf(control).then((read) => {
            declared = read;
            update();
          }, failed);
        }
        state.set({ target: tilemap && pose && declared ? { entity, tilemap, pose, parallax: parallaxOf(declared, tilemap.layer) } : null });
      };
      // Watched before it is read, so no change falls between; the changes before the read are in it.
      watch = editor.entities.watch(entity, (components, removed) => {
        if (tilemap === undefined) return;
        const changed = components as { [TILEMAP]?: Tilemap; [POSE]?: Pose };
        if (changed[TILEMAP]) tilemap = changed[TILEMAP];
        if (changed[POSE]) pose = changed[POSE];
        if (removed.includes(TILEMAP)) tilemap = null;
        if (removed.includes(POSE)) pose = null;
        update();
      }, () => {
        // Undone, seeked before its spawn, or on another branch: nothing to paint until it is back.
        tilemap = null;
        pose = null;
        update();
      });
      control
        .request({ type: "get_entity", entity })
        .then((reply) => {
          // A host that answers something else (a test double) leaves nothing to paint.
          if (reply.type !== "entity") return;
          const json = (name: string) => (reply.components[name]?.type === "json" ? reply.components[name].value : null);
          tilemap = json(TILEMAP) as Tilemap | null;
          pose = json(POSE) as Pose | null;
          update();
        })
        .catch((error: unknown) => {
          // A selection of an entity gone: the inspector clears it, and there is nothing to paint.
          if (!(error instanceof ProtocolFailure && error.code === "no_such_entity")) failed(error);
        });
    };

    const available = () => {
      const { target } = state.get();
      const selected = editor.selection.getState().entities;
      return target !== null && editor.control !== null && !editor.timeline.getState().playing && selected.length === 1 && same(selected[0]!, target.entity);
    };
    // The shell asks again on selection, timeline, and game changes; a tilemap read or removed later is checked here.
    const stopState = state.subscribe(() => {
      const active = editor.viewport.activeTool.getState();
      if ((active === PAINT || active === ERASE) && !available()) editor.viewport.setTool(null);
    });
    const stopSelection = editor.selection.subscribe((selection, previous) => {
      if (selection.entities !== previous.entities) load();
    });
    editor.onGameChanged(load);
    load();

    let stroke: Stroke | null = null;
    const tool = (id: string, title: string, erases: boolean): ViewportTool => ({
      id,
      title,
      cursor: "crosshair",
      available,
      pointerDown(event, context) {
        const { target, tile } = state.get();
        if (stroke || !target || (event.button !== 0 && event.button !== RIGHT_BUTTON)) return;
        const start = cellAt(context.screenToWorld(event.screen, target.parallax), target.pose, target.tilemap);
        if (!start) return;
        const erase = erases || event.button === RIGHT_BUTTON;
        if (!erase && tile === null) {
          editor.console.report(new Error("choose a tile in the Tile palette first"), "Painting tiles");
          return;
        }
        stroke = { start, end: start, tile: erase ? null : tile };
        context.redraw();
      },
      pointerMove(event, context) {
        const { target } = state.get();
        if (!stroke || !target) return;
        stroke.end = cellUnder(context.screenToWorld(event.screen, target.parallax), target.pose, target.tilemap);
        context.redraw();
      },
      pointerUp(event, context) {
        const { target } = state.get();
        const done = stroke;
        stroke = null;
        if (!done) return;
        context.redraw();
        if (!target) return;
        const end = cellUnder(context.screenToWorld(event.screen, target.parallax), target.pose, target.tilemap);
        const area = areaBetween(done.start, end, target.tilemap);
        editor
          .edit({ type: "paint_tiles", entity: target.entity, area, tile: done.tile })
          .catch((error: unknown) => editor.console.report(error, done.tile === null ? "Erasing tiles" : "Painting tiles"));
      },
      pointerCancel(context) {
        stroke = null;
        context.redraw();
      },
      overlay({ canvas, worldToScreen }) {
        const { target } = state.get();
        if (!stroke || !target) return;
        const area = areaBetween(stroke.start, stroke.end, target.tilemap);
        const { position } = target.pose;
        const { x: width, y: height } = target.tilemap.cell_size;
        const corner = (x: number, y: number) => worldToScreen({ x: position.x + x * width, y: position.y + y * height }, target.parallax);
        const a = corner(area.x, area.y);
        const b = corner(area.x + area.width, area.y + area.height);
        const colors = stroke.tile === null ? PREVIEW.erase : PREVIEW.paint;
        const rectangle = [Math.min(a.x, b.x), Math.min(a.y, b.y), Math.abs(b.x - a.x), Math.abs(b.y - a.y)] as const;
        canvas.save();
        canvas.fillStyle = colors.fill;
        canvas.strokeStyle = colors.stroke;
        canvas.fillRect(...rectangle);
        canvas.strokeRect(...rectangle);
        canvas.restore();
      },
    });
    editor.viewport.tools.register(tool(PAINT, "Paint tiles", false));
    editor.viewport.tools.register(tool(ERASE, "Erase tiles", true));
    editor.commands.register({ id: PAINT, title: "Paint tiles", run: (editor: Editor) => editor.viewport.setTool(PAINT) });
    editor.commands.register({ id: ERASE, title: "Erase tiles", run: (editor: Editor) => editor.viewport.setTool(ERASE) });
    editor.shortcuts.register({ key: "B", command: PAINT });
    editor.shortcuts.register({ key: "E", command: ERASE });
    editor.panels.register({
      id: "ludenea.tile-palette",
      title: "Tile palette",
      defaultPosition: "right",
      component: ({ editor }) => <TilePalette editor={editor} state={state} />,
    });

    return () => {
      loads++;
      watch?.dispose();
      stopState();
      stopSelection();
    };
  },
});
