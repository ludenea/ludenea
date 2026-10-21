// The simulation of a Ludenea web build, in a module Web Worker: it creates the game's control
// `WorkerHost`, loads `game.json`, and on `start` sets the players and plays, letting time pass
// every 4 ms. After the ticks of each step it posts the render frame of the newest tick. Every
// change to the game goes through host commands, so a match records the same session as the
// editor. The page sends:
//   { type: "init", ticks }            ticks: the tick to stop at, or null
//   { type: "start", claims }          one claim per player
//   { type: "devices", devices }       what is held, as `set_live_input` takes it
//   { type: "export_replay" }
// and receives `loaded`, `frame`, `replay`, `replay_failed`, `console`, `failed`, and the panic
// hook's `crashed`. After `failed` the game no longer plays, but its replay can still be saved.

import init, { createHost } from "./game.js";

/** The protocol hello of the engine this page was built with, written in by the build. */
const HELLO = "__LUDENEA_HELLO__";
/** How often time passes, in milliseconds. */
const STEP_MS = 4;
/** The `game.json` version this page reads. */
const GAME_VERSION = 1;

let host;
let nextId = 1;
let timer;
let failed = false;
let stopAt = null;
let tickRate = 60;
let tick = 0;

function fail(message) {
  clearInterval(timer);
  if (failed) return;
  failed = true;
  self.postMessage({ type: "failed", message });
}

/** Sends `command` to the host and returns its reply; a refusal throws its message. */
function request(command) {
  const id = nextId++;
  const messages = JSON.parse(host.handle(JSON.stringify({ type: "request", id, command })));
  const response = messages.find((message) => message.type === "response" && message.id === id);
  handleEvents(messages.filter((message) => message.type === "event").map((message) => message.event));
  if (response === undefined) throw new Error(`the game did not answer ${command.type}`);
  if ("error" in response.result) throw new Error(`the game refused ${command.type}: ${response.result.error.message}`);
  return response.result.ok;
}

/** Reports a failure, passes the console on, and posts the frame of the newest tick. */
function handleEvents(events) {
  let ticked;
  for (const event of events) {
    if (event.type === "ticked") ticked = event;
    else if (event.type === "console") self.postMessage({ type: "console", level: event.level, text: event.text });
    else if (event.type === "paused" && (event.reason.type === "failed" || event.reason.type === "panicked")) {
      fail(`the game failed at tick ${event.reason.tick}: ${event.reason.message}`);
    }
  }
  if (ticked === undefined || failed) return;
  tick = ticked.tick;
  // The frame's text as the host writes it, forwarded without parsing it here. When a step
  // ran several ticks, only the newest is posted; the page sees the gap in `tick`.
  self.postMessage({
    type: "frame",
    frame: host.renderFrameJson([]),
    tick: ticked.tick,
    hash: ticked.hash,
    progress: host.tickProgress(),
  });
}

async function fetchJson(name) {
  const response = await fetch(new URL(name, import.meta.url));
  if (!response.ok) throw new Error(`cannot fetch ${name}: ${response.status} ${response.statusText}`);
  return response.json();
}

async function load(ticks) {
  stopAt = ticks;
  await init();
  host = createHost();
  const [welcome] = JSON.parse(host.handle(JSON.stringify(HELLO)));
  if (welcome?.type !== "welcome") {
    throw new Error(`the game refused this page: ${welcome?.error?.message ?? "no welcome"}; build the page again`);
  }
  tickRate = welcome.game.tick_rate;
  const game = await fetchJson("game.json");
  if (game.version !== GAME_VERSION) {
    throw new Error(`game.json has version ${game.version}, but this page reads version ${GAME_VERSION}: build the page again`);
  }
  request({ type: "load_scene", scene: game.scene, assets: game.assets, document: "Scene" });
  self.postMessage({ type: "loaded", name: game.name, tickRate });
}

function start(claims) {
  request({ type: "set_players", players: claims.length, claims });
  request({ type: "play" });
  let last = performance.now();
  timer = setInterval(() => {
    const now = performance.now();
    let elapsed = now - last;
    last = now;
    if (stopAt !== null) {
      if (tick >= stopAt) return clearInterval(timer);
      // At most half a tick at a time, so a step runs one tick at most and play stops exactly
      // at `stopAt`; the rest of a stall is dropped.
      elapsed = Math.min(elapsed, 500 / tickRate);
    }
    guard(() => handleEvents(JSON.parse(host.advance(elapsed)).map((message) => message.event)));
  }, STEP_MS);
}

/** Runs `call`, turning what it throws into a failure; a panic of the game ends the instance. */
function guard(call) {
  if (failed) return;
  try {
    call();
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
  }
}

let loaded = Promise.resolve();

self.addEventListener("message", ({ data }) => {
  // Messages wait for the game to load and keep their order.
  loaded = loaded.then(async () => {
    if (data.type === "export_replay") return exportReplay();
    if (failed) return;
    try {
      if (data.type === "init") await load(data.ticks);
      else if (data.type === "start") start(data.claims);
      else if (data.type === "devices") request({ type: "set_live_input", devices: data.devices });
    } catch (error) {
      fail(error instanceof Error ? error.message : String(error));
    }
  });
});

/** Posts the replay of the match, after a failure of the game too: it holds the ticks that
 * ran, as the native window's `--record` does. A replay that cannot be made is reported on its
 * own and leaves the game as it is. */
function exportReplay() {
  try {
    if (host === undefined) throw new Error("the game is not loaded");
    const { replay } = request({ type: "export_replay", branch: null });
    self.postMessage({ type: "replay", replay });
  } catch (error) {
    self.postMessage({ type: "replay_failed", message: error instanceof Error ? error.message : String(error) });
  }
}
