// The simulation of a Ludenea web build, in a module Web Worker: it creates the game's control
// `WorkerHost`, loads `game.json`, and on `start` sets the players and plays, or on `join`
// joins the online match the page's room agreed on, letting time pass every 4 ms. After the
// ticks of each step it posts the render frame of the newest tick, and the packets the match
// sends. Every change to the game goes through host commands, so a match records the same
// session as the editor. The page sends:
//   { type: "init", ticks }            ticks: the tick to stop at (to leave at, online), or null
//   { type: "start", claims }          one claim per player
//   { type: "join", settings, claims } the match's `OnlineSettings` and this peer's claims
//   { type: "net", packets }           [{ from, packet: ArrayBuffer }] from the other peers
//   { type: "devices", devices }       what is held, as `set_live_input` takes it
//   { type: "leave" }                  leave the online match
//   { type: "export_replay" }
//   { type: "snapshot", tick }         the session's snapshot of `tick`, after an online match
//   { type: "stop" }                   the page failed: stop playing for good (online, leave)
// and receives `loaded`, `frame`, `net` ({ packets: [{ to, packet: ArrayBuffer }] },
// transferred), `online` ({ state }), `silent` ({ peer, ms }), `back` ({ peer }), `replay`,
// `replay_failed`, `snapshot` ({ tick, bytes: ArrayBuffer }), `snapshot_failed`, `console`,
// `failed`, and the panic hook's `crashed`. After `failed` the game no longer plays, but its
// replay can still be saved; an online match goes on carrying packets, so the others hear
// how it ended.

import init, { createHost } from "./game.js";

/** The protocol hello of the engine this page was built with, written in by the build. */
const HELLO = "__LUDENEA_HELLO__";
/** How often time passes, in milliseconds. */
const STEP_MS = 4;
/** The `game.json` version this page reads. */
const GAME_VERSION = 1;
/** The ticks the session resimulates per advance after an online match (the host's
 * `RESIMULATION_BUDGET`). */
const RESIMULATION_BUDGET = 240;

let host;
let nextId = 1;
let timer;
let failed = false;
let stopAt = null;
let tickRate = 60;
let tick = 0;
/** The page joined an online match; it left it; the match ended. */
let online = false;
let left = false;
let ended = false;
/** The host's refusal of the devices last warned about during an online match. */
let refusedInput;

function fail(message) {
  // Online, the match goes on: the others are told how it ended while it lingers.
  if (!online) clearInterval(timer);
  if (failed) return;
  failed = true;
  self.postMessage({ type: "failed", message });
}

/** Sends `command` to the host and returns its reply; a refusal throws its message, with the
 * error's `code`. */
function request(command) {
  const id = nextId++;
  const messages = JSON.parse(host.handle(JSON.stringify({ type: "request", id, command })));
  const response = messages.find((message) => message.type === "response" && message.id === id);
  handleEvents(messages.filter((message) => message.type === "event").map((message) => message.event));
  if (response === undefined) throw new Error(`the game did not answer ${command.type}`);
  if ("error" in response.result) {
    const error = new Error(`the game refused ${command.type}: ${response.result.error.message}`);
    error.code = response.result.error.code;
    throw error;
  }
  return response.result.ok;
}

/** Reports a failure, passes the console and the online match's news on, and posts the
 * frame of the newest tick. Once an online match ended, the session resimulates it from
 * tick 0: those ticks are not posted, so the page keeps showing the match's last tick. */
function handleEvents(events) {
  let ticked;
  let endedNow = false;
  for (const event of events) {
    if (event.type === "ticked") {
      if (!ended) ticked = event;
    } else if (event.type === "console") {
      self.postMessage({ type: "console", level: event.level, text: event.text });
    } else if (event.type === "paused" && (event.reason.type === "failed" || event.reason.type === "panicked")) {
      fail(`the game failed at tick ${event.reason.tick}: ${event.reason.message}`);
    } else if (event.type === "online") {
      if (event.state.type === "ended" && !ended) {
        ended = true;
        endedNow = true;
      }
      self.postMessage({ type: "online", state: event.state });
    } else if (event.type === "peer_silent") {
      self.postMessage({ type: "silent", peer: event.peer, ms: event.for_ms });
    } else if (event.type === "peer_back") {
      self.postMessage({ type: "back", peer: event.peer });
    }
  }
  if (ticked === undefined || (failed && !online)) return;
  tick = ticked.tick;
  // The frame's text as the host writes it, forwarded without parsing it here. When a step
  // ran several ticks, only the newest is posted; the page sees the gap in `tick`. The
  // landing of an online match comes with its end: the host already resimulates from tick
  // 0, so the page keeps the frame it has.
  self.postMessage({
    type: "frame",
    frame: endedNow ? null : host.renderFrameJson([]),
    tick: ticked.tick,
    hash: ticked.hash,
    progress: host.tickProgress(),
  });
}

/** Posts the packets the online match sent since the last call, transferring their bytes. */
function postPackets() {
  const packets = host.takeNetPackets();
  if (packets !== undefined) self.postMessage({ type: "net", packets }, packets.map((sent) => sent.packet));
}

/** Lets `elapsed` milliseconds pass, and posts what happened. */
function step(elapsed) {
  handleEvents(JSON.parse(host.advance(elapsed)).map((message) => message.event));
  postPackets();
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
  play();
}

/** Joins the online match: it connects, plays and lingers as time passes. */
function join(settings, claims) {
  request({ type: "join_match", settings, claims });
  online = true;
  play();
}

/** Lets time pass every 4 ms: locally up to `stopAt`; online until the page goes, as the
 * match lingers after its end and the session resimulates it. */
function play() {
  let last = performance.now();
  timer = setInterval(() => {
    const now = performance.now();
    let elapsed = now - last;
    last = now;
    if (stopAt !== null && !online) {
      if (tick >= stopAt) return clearInterval(timer);
      // At most half a tick at a time, so a step runs one tick at most and play stops exactly
      // at `stopAt`; the rest of a stall is dropped.
      elapsed = Math.min(elapsed, 500 / tickRate);
    }
    if (failed && !online) return;
    try {
      step(elapsed);
      // Online, `?ticks=N` leaves the match at tick N, as the native window's `--ticks`.
      if (online && stopAt !== null && tick >= stopAt) leave();
    } catch (error) {
      clearInterval(timer);
      fail(error instanceof Error ? error.message : String(error));
    }
  }, STEP_MS);
}

/** Gives the online match the packets the other peers sent. */
function receive(packets) {
  for (const { from, packet } of packets) {
    try {
      host.netReceive(from, new Uint8Array(packet));
    } catch (error) {
      self.postMessage({ type: "console", level: "error", text: `a packet from peer ${from} was refused: ${error}` });
    }
  }
}

/** Sends the devices held. During an online match, devices whose input is longer than the
 * match carries are a warning, once until they change, and the match goes on with the
 * previous input; after its end they are no longer sent. */
function setDevices(devices) {
  if (ended) return;
  try {
    request({ type: "set_live_input", devices });
    refusedInput = undefined;
  } catch (error) {
    if (!(online && error.code === "input_too_large")) throw error;
    if (error.message !== refusedInput) {
      self.postMessage({ type: "console", level: "warning", text: `the devices held are not played, the previous input stays: ${error.message}` });
    }
    refusedInput = error.message;
  }
}

/** Leaves the online match, once, unless it ended. */
function leave() {
  if (!online || left || ended) return;
  left = true;
  try {
    request({ type: "leave_match" });
    postPackets();
  } catch (error) {
    self.postMessage({ type: "console", level: "warning", text: error instanceof Error ? error.message : String(error) });
  }
}

let loaded = Promise.resolve();

self.addEventListener("message", ({ data }) => {
  // Messages wait for the game to load and keep their order.
  loaded = loaded.then(async () => {
    if (data.type === "export_replay") return exportReplay();
    if (data.type === "snapshot") return snapshot(data.tick);
    if (host !== undefined && data.type === "net") return receive(data.packets);
    if (data.type === "leave") return leave();
    if (failed) return;
    try {
      if (data.type === "init") await load(data.ticks);
      else if (data.type === "start") start(data.claims);
      else if (data.type === "join") join(data.settings, data.claims);
      else if (data.type === "devices") setDevices(data.devices);
      else if (data.type === "stop") stop();
    } catch (error) {
      fail(error instanceof Error ? error.message : String(error));
    }
  });
});

/** Stops play for good, the page having failed: no tick runs after it, so the replay ends
 * where the page stopped. Only `export_replay` is answered afterwards. Online it leaves the
 * match instead, which goes on until every peer ends at the same tick. */
function stop() {
  failed = true;
  if (online) return leave();
  clearInterval(timer);
  request({ type: "pause" });
}

/** Runs `call` again while the host answers `resimulating`, as after an online match, letting
 * no time pass between the tries: each advance resimulates up to `RESIMULATION_BUDGET` ticks,
 * so at most `tick / RESIMULATION_BUDGET + 2` advances are needed. */
function settled(call) {
  const advances = Math.floor(tick / RESIMULATION_BUDGET) + 2;
  for (let tries = 0; ; tries++) {
    try {
      return call();
    } catch (error) {
      if (error.code !== "resimulating" || tries >= advances) throw error;
      step(0);
    }
  }
}

/** Posts the replay of the match, after a failure of the game too: it holds the ticks that
 * ran, as the native window's `--record` does. After an online match it holds the match's
 * confirmed inputs. A replay that cannot be made is reported on its own and leaves the game as
 * it is. */
function exportReplay() {
  try {
    if (host === undefined) throw new Error("the game is not loaded");
    const { replay } = settled(() => request({ type: "export_replay", branch: null }));
    self.postMessage({ type: "replay", replay });
  } catch (error) {
    self.postMessage({ type: "replay_failed", message: error instanceof Error ? error.message : String(error) });
  }
}

/** Posts the session's snapshot of `tick`, as the native runtimes write it for a desync. */
function snapshot(at) {
  try {
    if (host === undefined) throw new Error("the game is not loaded");
    settled(() => request({ type: "seek", tick: at }));
    const { snapshot } = settled(() => request({ type: "take_snapshot" }));
    const bytes = Uint8Array.from(atob(snapshot), (char) => char.charCodeAt(0)).buffer;
    self.postMessage({ type: "snapshot", tick: at, bytes }, [bytes]);
  } catch (error) {
    self.postMessage({ type: "snapshot_failed", message: error instanceof Error ? error.message : String(error) });
  }
}
