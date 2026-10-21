// The browser renderer (`ludenea_render::web`, built by `cargo xtask wasm-render
// editor/e2e/fixtures/renderer/pkg`) drawing into a 128 by 128 canvas, for renderer.spec.ts.
// The camera shows 8 by 8 world units around (2, 0): 16 pixels per unit, world (x, y) at pixel
// (32 + 16 x, 64 - 16 y).
import init, { WebRenderer, panicForTesting, setPanicHandler, textureFrames } from "./pkg/ludenea_render.js";

const SIZE = 128;
const SPRITES = "00000000000000000000000000000001";
const TILES = "00000000000000000000000000000002";
const BROKEN = "00000000000000000000000000000003";
const grid = { frame_width: 16, frame_height: 16, margin: 0, spacing: 0 };
const meta = (grid) => ({ pixels_per_unit: 16, grid, pivot: null, filter: "Nearest" });
const entity = (index) => ({ index, generation: 0 });

const canvas = document.getElementById("target");
let renderer;

/** Loads the module, opens the renderer on the canvas, and loads the textures. */
async function start() {
  await init();
  renderer = await WebRenderer.create(canvas);
  renderer.resize(SIZE, SIZE, 1);
  const sources = [
    // Frame 0: the left half one colour, the right half another; frame 1: a third colour.
    { id: SPRITES, path: "sprites.png", meta: meta(grid) },
    { id: TILES, path: "tiles.png", meta: meta(null) },
    { id: BROKEN, path: "broken.png", meta: meta(null) },
  ];
  const pngs = await Promise.all(
    ["sprites.png", "tiles.png"].map(async (file) => new Uint8Array(await (await fetch(file)).arrayBuffer())),
  );
  pngs.push(new Uint8Array([1, 2, 3]));
  const errors = JSON.parse(renderer.set_textures(JSON.stringify(sources), pngs));
  return { backend: renderer.backend, errors };
}

/** The rotation of `degrees`, counterclockwise. */
function turn(degrees) {
  const radians = (degrees * Math.PI) / 180;
  return { cos: Math.cos(radians), sin: Math.sin(radians) };
}

function sprite(index, position, { layer = 1, order = 0, frame = 0, flip_x = false, degrees = 0 } = {}) {
  return {
    entity: entity(index),
    layer,
    order,
    texture: SPRITES,
    frame,
    position: { x: position[0], y: position[1] },
    rotation: turn(degrees),
    flip_x,
    flip_y: false,
    tint: 0xffffffff,
  };
}

function frame(sprites, extra = {}) {
  return {
    tick: 0,
    camera: { entity: entity(1000), center: { x: 2, y: 0 }, view: { x: 8, y: 8 } },
    layers: [
      { name: "background", parallax: 0 },
      { name: "world", parallax: 1 },
    ],
    sprites,
    tilemaps: [],
    teleported: [],
    debug: { layers: {} },
    warnings: [],
    ...extra,
  };
}

/** The RGBA of each `[x, y]` pixel of the canvas, read in the task that drew it. */
function pixels(points) {
  const copy = new OffscreenCanvas(canvas.width, canvas.height).getContext("2d", { willReadFrequently: true });
  copy.drawImage(canvas, 0, 0);
  return points.map(([x, y]) => Array.from(copy.getImageData(x, y, 1, 1).data));
}

/** The tick of the frame pushed last; every `draw` starts two ticks later. */
let tick = 0;

/** Pushes `frame` at `at`; returns the report of `push_frame`. */
function push(frame, overlay, at) {
  return JSON.parse(renderer.push_frame(JSON.stringify({ ...frame, tick: at }), JSON.stringify(overlay)));
}

/**
 * Pushes `previous` when given, then `current` at the next tick, and draws `alpha` of the way
 * from `previous` to `current`; without `previous`, `current` comes after a gap of a tick and
 * draws as is. Returns the report of the push of `current`.
 */
function draw(current, overlay = {}, previous = undefined, alpha = 1) {
  tick += 2;
  if (previous !== undefined) push(previous, overlay, tick - 1);
  const report = push(current, overlay, tick);
  renderer.draw(alpha, undefined);
  return report;
}

/** A sprite at (0, 3.5) then, two ticks later, at (1, 3.5), drawn halfway: the gap means no blend. */
function drawAfterGap(points) {
  tick += 2;
  push(frame([sprite(1, [0, 3.5])]), {}, tick);
  tick += 2;
  push(frame([sprite(1, [1, 3.5])]), {}, tick);
  renderer.draw(0.5, undefined);
  return pixels(points);
}

/** Draws the frames already set at `alpha`, through the free `camera` when given, and reads `points`. */
function redraw(alpha, points, camera = undefined) {
  renderer.draw(alpha, camera === undefined ? undefined : JSON.stringify(camera));
  return pixels(points);
}

/**
 * The reference frame: a sprite at (0, 2), the same sprite flipped at (4, 2), a tilemap of two
 * cells (one empty) moved from (-1, -3) to (0, -3), a sprite at (0, -1) on a layer fixed to the
 * screen, a square outline from (3, -3) to (5, -1), a selected entity's filled circle at (0, 0),
 * a sprite on the world layer over one at the same pixels on the layer fixed to the screen
 * (listed after it, with a higher order), a sprite moved from (0, 3.5) to (1, 3.5), and a sprite
 * at (2.5, 2) turned from 170 to -170 degrees, all drawn halfway.
 */
function drawReference(points) {
  const square = [[3, -3], [5, -3], [5, -1], [3, -1]].map(([x, y]) => ({ x, y }));
  const outline = { type: "polygon", points: square, color: 0x3060f0ff, fill: false };
  const dot = { type: "circle", center: { x: 0, y: 0 }, radius: 0.5, color: 0x3060f0ff, fill: true };
  const reference = frame(
    [
      sprite(1, [0, 2]),
      sprite(2, [4, 2], { flip_x: true }),
      sprite(3, [0, -1], { layer: 0, frame: 1 }),
      sprite(7, [5, 0.5]),
      sprite(8, [3, 0.5], { layer: 0, order: 10, frame: 1 }),
      sprite(9, [1, 3.5]),
      sprite(10, [2.5, 2], { degrees: -170 }),
    ],
    {
      tilemaps: [
        {
          entity: entity(4),
          layer: 1,
          order: 0,
          tileset: TILES,
          position: { x: 0, y: -3 },
          cell_size: { x: 1, y: 1 },
          width: 2,
          height: 1,
          tiles: [0, null],
        },
      ],
      debug: {
        layers: {
          physics: [
            { entity: entity(5), primitive: outline },
            { entity: entity(6), primitive: dot },
          ],
        },
      },
    },
  );
  const previous = structuredClone(reference);
  previous.sprites[5].position = { x: 0, y: 3.5 };
  previous.sprites[6].rotation = turn(170);
  previous.tilemaps[0].position = { x: -1, y: -3 };
  const report = draw(reference, { debug_layers: ["physics"], selected: [entity(6)] }, previous, 0.5);
  return { report, pixels: pixels(points) };
}

/** `count` sprites, all off screen but those `placed` at a position by index. */
function drawSprites(count, placed, points) {
  const sprites = Array.from({ length: count }, (_, index) => sprite(index, placed[index] ?? [100, 100]));
  return { report: draw(frame(sprites)), pixels: pixels(points) };
}

/** The `name` and `message` of what `run` throws, or null when it returns. */
function errorOf(run) {
  try {
    run();
    return null;
  } catch (error) {
    return { name: error.name, message: error.message };
  }
}

/** Draws an empty frame: the renderer's error, or null when it drew. */
function drawError() {
  return errorOf(() => draw(frame([])));
}

/**
 * Pushes `frame.json` as JSON and draws it, then the same frame in postcard (`frame.postcard`)
 * and draws it: each push's report and the pixels at `points`.
 */
async function pushBothWays(points) {
  const [json, bytes] = await Promise.all([
    fetch("frame.json").then((response) => response.text()),
    fetch("frame.postcard").then(async (response) => new Uint8Array(await response.arrayBuffer())),
  ]);
  const drawn = (push) => {
    const report = JSON.parse(push());
    renderer.draw(1, undefined);
    return { report, pixels: pixels(points) };
  };
  const overlay = JSON.stringify({ debug_layers: ["physics"] });
  const fromJson = drawn(() => renderer.push_frame(json, overlay));
  // A blank frame between, so the second draw cannot show the first one's pixels.
  draw(frame([]));
  const fromBytes = drawn(() => renderer.push_frame_bytes(bytes, overlay));
  return { fromJson, fromBytes };
}

/** What the renderer answers about the frame pushed last, at CSS pixel `[x, y]`. */
function answers([x, y], entities) {
  return {
    picked: JSON.parse(renderer.pick(x, y, undefined)),
    pickedThroughAFreeCamera: JSON.parse(renderer.pick(x, y, JSON.stringify({ center: { x: 3, y: 0 }, view: { x: 8, y: 8 } }))),
    bounds: JSON.parse(renderer.bounds(JSON.stringify(entities)) ?? "null"),
    world: JSON.parse(renderer.screen_to_world(x, y, 1, undefined)),
    background: JSON.parse(renderer.screen_to_world(x, y, 0, undefined)),
  };
}

/** The frames `textureFrames` cuts sprites.png into, with `meta`, or the error it throws. */
async function framesOf(meta) {
  const png = new Uint8Array(await (await fetch("sprites.png")).arrayBuffer());
  try {
    return { frames: JSON.parse(textureFrames(png, JSON.stringify(meta))) };
  } catch (error) {
    return { error: { name: error.name, message: error.message } };
  }
}

/** Each kind of input the renderer cannot read: the error each throws. */
function badInputs() {
  return {
    json: errorOf(() => renderer.push_frame("{", "{}")),
    overlay: errorOf(() => renderer.push_frame(JSON.stringify(frame([])), "[")),
    bytes: errorOf(() => renderer.push_frame_bytes(new Uint8Array([255, 255, 255]), "{}")),
    camera: errorOf(() => renderer.draw(1, "{")),
    pngCount: errorOf(() => renderer.set_textures("[]", [new Uint8Array([1])])),
  };
}

/** Sets a panic handler, then panics in the module: what the handler received. */
function panicked() {
  const received = [];
  setPanicHandler((message) => received.push(message));
  const thrown = errorOf(() => panicForTesting("a panic on purpose"));
  return { received, thrown: thrown !== null };
}

/** The renderer's WebGPU device, which the canvas is configured with. */
const device = () => canvas.getContext("webgpu").getConfiguration().device;

/** Raises a validation error on the device outside any renderer call: a buffer both read and written by mapping. */
function failOnTheDevice() {
  device().createBuffer({ label: "invalid on purpose", size: 4, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.MAP_WRITE });
}

function loseTheDevice() {
  device().destroy();
}

/** Opens a renderer on a new canvas of `width` by `height` pixels: null, or its error. */
async function createOn(width, height) {
  const other = document.createElement("canvas");
  other.width = width;
  other.height = height;
  try {
    (await WebRenderer.create(other)).free();
    return null;
  } catch (error) {
    return { name: error.name, message: error.message };
  }
}

function resize(width, height) {
  renderer.resize(width, height, 1);
}

window.fixture = {
  start,
  drawReference,
  drawAfterGap,
  redraw,
  drawSprites,
  drawError,
  failOnTheDevice,
  loseTheDevice,
  createOn,
  resize,
  pushBothWays,
  answers,
  framesOf,
  badInputs,
  panicked,
};
