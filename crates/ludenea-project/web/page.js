// The page of a Ludenea web build. The simulation runs in `worker.js`; this page draws its
// frames with the game's `WebRenderer`, between the last two ticks, runs the lobby with the
// game's `WebLobby` (or skips it on `?players=N`), sends the keys and gamepads held, and saves
// the replay. `?ticks=N` stops play at tick N (online, leaves the match there).
//
// Online, `?host=N` (2 to 4 peers) opens a room and `?join=CODE` joins one, through the
// signaling listener of `?signaling=URL` (the page's own host, `/rooms`, by default), with
// the STUN servers of `?stun=URL` (repeatable; `none` for none) and the conditions of
// `?latency=MS&jitter=MS&loss=PERCENT` on this peer's sends. The page holds the room and the
// WebRTC link (the game's `WebOnline`), which the worker cannot reach, and carries the
// match's packets between it and the worker's host: whenever the worker posts, and every 4 ms.
// The worker's own timer drives its posts, which a hidden tab still receives, so a hidden tab
// keeps its match going. Runtime code: it reads the clock to draw and to pace the link, and
// changes the game only through the worker's host commands.

import init, { WebLobby, WebOnline, WebRenderer, automaticClaimsJson, defaultStun, endLingerMs } from "./game.js";

const canvas = document.getElementById("game");
const status = document.getElementById("status");
const lobbyView = document.getElementById("lobby");
const slots = document.getElementById("slots");
const lobbyHelp = document.getElementById("lobby-help");
const roomView = document.getElementById("room");
const save = document.getElementById("save");
const leaveButton = document.getElementById("leave");
const snapshotButton = document.getElementById("snapshot");

/** Function keys reload, toggle fullscreen and open the developer tools: the browser keeps
 * them (they still count as held for the game). */
const FUNCTION_KEY = /^F\d+$/;
/** With Control or Meta, the shortcuts a player must keep: reload (R), close the tab or the
 * window (W), the address bar (L), a new tab (T), and quit (Q). Every other combination, such
 * as Control with WASD, reaches the game; zoom (Control with Plus, Minus or 0) is deliberately
 * left to the game too, so a page that plays never changes size under the player. */
const SHORTCUTS = new Set(["KeyR", "KeyW", "KeyL", "KeyT", "KeyQ"]);
const META = new Set(["MetaLeft", "MetaRight"]);
/** The modifier keys, which report their own keyup. */
const MODIFIER_KEYS = new Set([...META, "ShiftLeft", "ShiftRight", "ControlLeft", "ControlRight", "AltLeft", "AltRight"]);
/** The name of the renderer's error for a canvas without a texture this frame, which passes. */
const TRANSIENT_DRAW = "NoSurfaceTexture";
/** Consecutive transient draw failures, about two seconds of frames, after which the page
 * gives up and says why. */
const MAX_TRANSIENT_DRAWS = 120;
/** A gamepad button counts as pressed above this value, as the game's input reads it. */
const PRESSED = 0.5;
/** How often the page pumps the online link when no worker message arrives, in milliseconds. */
const PUMP_MS = 4;
/** The parameters that only an online page takes, besides `?host` and `?join`. */
const ONLINE_PARAMETERS = ["signaling", "stun", "latency", "jitter", "loss"];
/** The parameter each field of `WebOnline`'s options comes from, to name it in a refusal. */
const PARAMETER_OF = {
  "role.host": "host",
  "role.join": "join",
  signaling: "signaling",
  stun: "stun",
  "conditions.latency_ms": "latency",
  "conditions.jitter_ms": "jitter",
  "conditions.loss_percent": "loss",
};
/** What the status says in each state of the online match. */
const ONLINE_WORDS = {
  room: () => "Waiting for the room",
  connecting: () => "Connecting to the other peers",
  running: () => `Playing online as peer ${local}`,
  leaving: () => "Leaving the match",
};

let failed = false;
let name = "game";
let tickRate = 60;
let renderer;
let lobby;
let worker;
/** The JSON of the newest frame until the renderer has it, and whether it has any. */
let pendingFrame;
let pushed = false;
let transientDraws = 0;
let draws = 0;
/** The host's progress into the next tick when the current frame arrived, and when it did. */
let progress = 0;
let arrival = 0;
let playing = false;
const held = new Set();
/** The keys held when the match started, such as the Enter that started it: they reach the
 * game only once pressed again, not through the keyboard's repeats. */
const heldAtStart = new Set();
let sentDevices = "";
/** The buttons each gamepad slot held at the last look, for the lobby's presses. */
let lobbyButtons = [];
/** The room and link of an online page (`WebOnline`), the timer that pumps it, and whether
 * it is pumped still. */
let online;
let pumpTimer;
let pumping = false;
/** The claims the room was readied with, once it was; whether the worker joined the match;
 * this peer's number in it; the match's state, its end, and the peers silent, with the
 * silence announced in milliseconds. */
let readyClaims;
let joined = false;
let local;
let onlineState = "room";
let end;
/** When the page heard of the end, on its monotonic clock. */
let endedAt;
const silent = new Map();

function say(message) {
  if (!failed) status.textContent = message;
}

/**
 * Stops the page and shows `message`. A game that failed keeps its worker, so "Save replay"
 * still saves the ticks that ran; `keepWorker` false ends the worker (a crash, a page error).
 */
function fail(message, { keepWorker = false } = {}) {
  if (failed) return;
  failed = true;
  playing = false;
  status.textContent = message;
  document.body.dataset.failed = "true";
  // Without its worker the match cannot go on, and before the match the page leaves the room;
  // a match that started goes on until every peer ends it at the same tick.
  if (!keepWorker || !joined) closeLink();
  if (!keepWorker) {
    worker?.terminate();
    save.disabled = true;
  } else if (worker !== undefined) {
    // The game keeps nothing held and stops playing: its replay ends here, as it was.
    worker.postMessage({ type: "devices", devices: { keys: [], gamepads: [] } });
    worker.postMessage({ type: "stop" });
  }
}

function showLobby() {
  // The engine names each claim's devices, as the native window's title does.
  const described = JSON.parse(lobby.describeClaims());
  for (const [index, slot] of [...slots.children].entries()) {
    const devices = described[index];
    slot.textContent = devices === undefined ? "Free" : `Player ${index + 1}: ${devices}`;
    slot.toggleAttribute("data-claimed", devices !== undefined);
  }
}

/** Passes a press to the lobby, and starts the match when it says so. */
function lobbyPress(event) {
  const pressed = JSON.parse(event);
  if (pressed.type === "started") {
    startMatch(pressed.claims);
  } else if (pressed.type !== "ignored") {
    showLobby();
  }
}

/** Starts the match with `claims`; online, readies the room with them instead, and the match
 * starts once the room agreed. */
function startMatch(claims) {
  if (online !== undefined) return readyRoom(claims);
  lobbyView.hidden = true;
  holdNothing();
  save.disabled = false;
  say(`Playing with ${claims.length} ${claims.length === 1 ? "player" : "players"}`);
  worker.postMessage({ type: "start", claims });
}

/** The keys held now, such as the Enter that started the match, reach the game only once
 * pressed again; the devices go to the game from now on. */
function holdNothing() {
  for (const code of held) heldAtStart.add(code);
  held.clear();
  playing = true;
}

/** Readies the room with one local player per claim; the lobby keeps showing the room. */
function readyRoom(claims) {
  try {
    online.setReady(claims.length);
  } catch (error) {
    return fail(String(error));
  }
  readyClaims = claims;
  slots.hidden = true;
  lobbyHelp.hidden = true;
}

/** The page's online parameters as the options of `WebOnline`, `null` to play locally. A
 * malformed one throws its message, naming it; `WebOnline` checks the ranges. The default
 * STUN server (`stun: null`) is the game's, known once it loaded. */
function onlineOptions() {
  const parameters = new URLSearchParams(location.search);
  const host = parameters.get("host");
  const join = parameters.get("join");
  if (host === null && join === null) {
    const given = ONLINE_PARAMETERS.find((name) => parameters.has(name));
    if (given !== undefined) throw new Error(`?${given} is an online parameter: give ?host=N or ?join=CODE with it`);
    return null;
  }
  if (host !== null && join !== null) throw new Error("?host and ?join cannot both be given: host a room or join one");
  const whole = (name, what) => {
    const value = parameters.get(name) ?? "0";
    if (!/^\d+$/.test(value)) throw new Error(`?${name}=${value} is not ${what}`);
    return Number(value);
  };
  const stun = parameters.getAll("stun");
  if (stun.includes("none") && stun.length > 1) throw new Error("?stun=none uses no STUN server: give it alone");
  return {
    role: host !== null ? { host: whole("host", "a number of peers") } : { join },
    signaling: parameters.get("signaling") ?? `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/rooms`,
    stun: stun.length === 0 ? null : stun[0] === "none" ? [] : stun,
    conditions: {
      latency_ms: whole("latency", "a number of milliseconds"),
      jitter_ms: whole("jitter", "a number of milliseconds"),
      loss_percent: whole("loss", "a percentage"),
    },
  };
}

/** Opens the room and the link of `options`, and pumps them from now on. A refusal throws,
 * naming the parameter of the field `WebOnline` refused. */
function goOnline(options) {
  options.stun ??= [defaultStun()];
  try {
    online = new WebOnline(JSON.stringify(options), crypto.getRandomValues(new Uint32Array(4)));
  } catch (error) {
    const message = String(error);
    const field = message.slice(0, message.indexOf(": "));
    const parameter = PARAMETER_OF[field];
    throw new Error(parameter === undefined ? `the online options are refused: ${message}` : `?${parameter}: ${message.slice(field.length + 2)}`);
  }
  document.body.dataset.online = onlineState;
  roomView.hidden = false;
  lobbyView.hidden = false;
  leaveButton.hidden = false;
  showOnline();
  pumping = true;
  pumpTimer = setInterval(pump, PUMP_MS);
}

/** Stops pumping the link and closes it. */
function closeLink() {
  pumping = false;
  clearInterval(pumpTimer);
  online?.free();
  online = undefined;
  leaveButton.disabled = true;
  document.body.dataset.link = "closed";
}

/** Polls the room, starts the match once it agreed, and passes the packets received on to
 * the worker. */
function pump() {
  if (!pumping) return;
  const now = performance.now();
  const room = JSON.parse(online.poll(now));
  for (const message of online.messages()) console.warn(message);
  if (!joined) {
    if (room.state === "failed") return fail(room.message);
    if (room.state === "room") showRoom(room);
    else joinMatch(room);
    // A page that could not join closed its link.
    if (!pumping) return;
  }
  const packets = online.receive(now);
  if (packets.length > 0) {
    worker.postMessage(
      { type: "net", packets },
      packets.map((received) => received.packet),
    );
  }
  // Once the others were told how the match ended, as the host lingers to tell them, the
  // link closes: the room leaves the listener, and counts no more against its limits.
  if (endedAt !== undefined && now - endedAt >= endLingerMs()) closeLink();
}

/** Shows the room's code once it exists on the listener, and what it waits for. */
function showRoom({ assigned, describe }) {
  if (assigned && document.body.dataset.room === undefined) document.body.dataset.room = online.code;
  roomView.textContent = assigned ? `Room ${online.code}: ${describe}` : describe;
}

/** The room agreed on the match: the worker joins it. */
function joinMatch({ settings, local: peer }) {
  if (readyClaims === undefined) return fail("the room agreed on a match before this page was ready");
  joined = true;
  local = peer;
  lobbyView.hidden = true;
  holdNothing();
  worker.postMessage({ type: "join", settings, claims: readyClaims });
}

/** The online match is in `state`, as the worker's host says. */
function setOnline(state) {
  onlineState = state.type;
  document.body.dataset.online = onlineState;
  leaveButton.disabled = !(onlineState === "connecting" || onlineState === "running");
  if (onlineState === "ended") {
    end = state.end;
    // The worker tells every state it reports: the linger counts from the first end.
    endedAt ??= performance.now();
    document.body.dataset.end = end.kind;
    playing = false;
    silent.clear();
    save.disabled = false;
    snapshotButton.hidden = end.kind !== "desync";
  }
  showOnline();
}

/** Says where the online match stands: the peers silent while it plays, its end after it. */
function showOnline() {
  if (end !== undefined) return say(`The match ended: ${end.message}`);
  const silences = [...silent].map(([peer, ms]) => `peer ${peer} silent for ${ms / 1000} s`);
  say(silences.length > 0 ? silences.join(", ") : ONLINE_WORDS[onlineState]());
}

/** The gamepads in their slots, as the game's input reads them; `null` is an empty slot. */
function gamepads() {
  const pads = [...(navigator.getGamepads?.() ?? [])].map((pad) =>
    pad?.connected ? { buttons: pad.buttons.map((button) => button.value), axes: [...pad.axes] } : null,
  );
  while (pads.length > 0 && pads[pads.length - 1] === null) pads.pop();
  return pads;
}

/** The lobby's gamepad presses: each button that went down since the last look. */
function pollLobbyButtons(pads) {
  pads.forEach((pad, slot) => {
    const before = lobbyButtons[slot] ?? [];
    pad?.buttons.forEach((value, button) => {
      if (value > PRESSED && !(before[button] > PRESSED)) lobbyPress(lobby.pressButton(slot, button));
    });
  });
  lobbyButtons = pads.map((pad) => pad?.buttons ?? []);
}

/** Releases every key (the page lost the keyboard), and the gamepads too when the tab is
 * hidden (the page cannot read them then), and tells the game at once: on a hidden tab no
 * animation frame runs, but the worker keeps playing. The next frame shown sends them again. */
function releaseDevices({ gamepadsToo }) {
  held.clear();
  if (playing) sendDevices(gamepadsToo ? [] : gamepads());
}

function sendDevices(pads) {
  const devices = JSON.stringify({ keys: [...held].sort(), gamepads: pads });
  if (devices === sentDevices) return;
  sentDevices = devices;
  worker.postMessage({ type: "devices", devices: JSON.parse(devices) });
}

function resize() {
  renderer.resize(canvas.clientWidth, canvas.clientHeight, devicePixelRatio);
}

function animate() {
  if (failed) return;
  const pads = gamepads();
  if (playing) sendDevices(pads);
  else if (lobby && !lobby.started) pollLobbyButtons(pads);
  try {
    if (pendingFrame !== undefined) {
      const frame = pendingFrame;
      pendingFrame = undefined;
      pushed = true;
      // The renderer blends it from the frame pushed last only when that is the tick before:
      // after a step of several ticks, a seek, or frames this page had no time to draw, it
      // shows as is.
      const report = JSON.parse(renderer.push_frame(frame, "{}"));
      if (report.missing.length > 0) console.warn(`${report.missing.length} sprites or tiles have no texture`, report.missing);
    }
    if (pushed) {
      const alpha = Math.min(Math.max(progress + ((performance.now() - arrival) * tickRate) / 1000, 0), 1);
      renderer.draw(alpha, undefined);
      transientDraws = 0;
      document.body.dataset.draws = String(++draws);
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    // A canvas without a texture this frame draws again on the next one; the renderer names
    // that error. Any other error, or one that never passes, stops the page with its cause.
    if (error?.name !== TRANSIENT_DRAW || ++transientDraws > MAX_TRANSIENT_DRAWS) {
      return fail(`the game cannot be drawn: ${message}`, { keepWorker: true });
    }
    console.warn(message);
  }
  requestAnimationFrame(animate);
}

function onWorkerMessage({ data }) {
  switch (data.type) {
    case "loaded":
      name = data.name;
      tickRate = data.tickRate;
      document.title = name;
      document.getElementById("game-name").textContent = name;
      chooseClaims();
      break;
    case "frame":
      // No frame with the end of an online match: the page keeps drawing the one it has.
      if (data.frame !== null) {
        pendingFrame = data.frame;
        progress = data.progress;
        arrival = performance.now();
      }
      document.body.dataset.tick = String(data.tick);
      document.body.dataset.hash = data.hash;
      break;
    case "net":
      for (const { to, packet } of data.packets) online?.send(to, new Uint8Array(packet), performance.now());
      break;
    case "online":
      setOnline(data.state);
      break;
    case "silent":
      silent.set(data.peer, data.ms);
      showOnline();
      break;
    case "back":
      silent.delete(data.peer);
      showOnline();
      break;
    case "replay":
      download(Uint8Array.from(atob(data.replay), (char) => char.charCodeAt(0)), `${name}.replay`);
      break;
    case "snapshot":
      download(new Uint8Array(data.bytes), `desync-tick-${data.tick}-peer-${local}.snap`);
      break;
    case "snapshot_failed":
      status.textContent = `${status.textContent}; the snapshot cannot be saved: ${data.message}`;
      break;
    case "console":
      (data.level === "error" ? console.error : data.level === "warning" ? console.warn : console.info)(data.text);
      break;
    case "replay_failed":
      // Beside a failure already shown, not in place of it.
      status.textContent = `${failed ? `${status.textContent}; ` : ""}the replay cannot be saved: ${data.message}`;
      break;
    case "failed":
      fail(data.message, { keepWorker: true });
      break;
    case "crashed":
      fail(`the game crashed at tick ${data.tick}: ${data.message}`);
      break;
  }
  // The link is pumped on every message too: the worker's timer runs on a hidden tab.
  pump();
}

/** Starts the match on `?players=N`, else opens the lobby. */
function chooseClaims() {
  const players = new URLSearchParams(location.search).get("players");
  if (players === null) {
    lobby = new WebLobby();
    lobbyView.hidden = false;
    showLobby();
    say("Waiting for players");
    return;
  }
  if (!/^\d+$/.test(players)) return fail(`?players=${players} is not a number of players`);
  try {
    startMatch(JSON.parse(automaticClaimsJson(Math.min(Number(players), 0xffffffff))));
  } catch (error) {
    fail(String(error));
  }
}

function download(bytes, file) {
  const link = document.createElement("a");
  link.href = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
  link.download = file;
  link.click();
  // After the click has started the download.
  setTimeout(() => URL.revokeObjectURL(link.href), 60_000);
}

async function fetchBytes(file) {
  const response = await fetch(file);
  if (!response.ok) throw new Error(`cannot fetch ${file}: ${response.status} ${response.statusText}`);
  return new Uint8Array(await response.arrayBuffer());
}

async function loadAtlas() {
  const manifest = new TextDecoder().decode(await fetchBytes("atlas/atlas.json"));
  const { pages } = JSON.parse(manifest);
  const pngs = await Promise.all(pages.map((page) => fetchBytes(`atlas/${page.file}`)));
  renderer.set_atlas(manifest, pngs);
}

function captureKeys() {
  addEventListener("keydown", (event) => {
    // The browser keeps its shortcuts; every key, those included, counts as held.
    const shortcut = FUNCTION_KEY.test(event.code) || ((event.ctrlKey || event.metaKey) && SHORTCUTS.has(event.code));
    if (!shortcut) event.preventDefault();
    if (event.repeat && heldAtStart.has(event.code)) return;
    heldAtStart.delete(event.code);
    held.add(event.code);
    if (lobby && !lobby.started && !event.repeat) lobbyPress(lobby.pressKey(event.code));
  });
  addEventListener("keyup", (event) => {
    heldAtStart.delete(event.code);
    held.delete(event.code);
    // macOS sends no keyup for a key released while Meta is down: when Meta goes up, every
    // key but the modifiers counts as released, and the game hears it now.
    if (META.has(event.code) && ![...META].some((code) => held.has(code))) {
      for (const code of held) if (!MODIFIER_KEYS.has(code)) held.delete(code);
      if (playing) sendDevices(gamepads());
    }
  });
  // Keys released while the page had no focus never report it.
  addEventListener("blur", () => releaseDevices({ gamepadsToo: false }));
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) releaseDevices({ gamepadsToo: true });
  });
}

async function main() {
  // index.html says why the page cannot run from a file.
  if (location.protocol === "file:") return;
  const ticks = new URLSearchParams(location.search).get("ticks");
  if (ticks !== null && !/^\d+$/.test(ticks)) return fail(`?ticks=${ticks} is not a tick number`);
  let options;
  try {
    options = onlineOptions();
  } catch (error) {
    return fail(error.message);
  }
  await init();
  if (options !== null) {
    try {
      goOnline(options);
    } catch (error) {
      return fail(error.message);
    }
  }
  renderer = await WebRenderer.create(canvas);
  resize();
  addEventListener("resize", resize);
  await loadAtlas();
  captureKeys();
  worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
  worker.addEventListener("message", onWorkerMessage);
  worker.addEventListener("error", (event) => fail(`the game's worker failed: ${event.message ?? "it could not start"}`));
  worker.postMessage({ type: "init", ticks: ticks === null ? null : Number(ticks) });
  save.addEventListener("click", () => worker.postMessage({ type: "export_replay" }));
  leaveButton.addEventListener("click", () => {
    leaveButton.disabled = true;
    worker.postMessage({ type: "leave" });
  });
  snapshotButton.addEventListener("click", () => worker.postMessage({ type: "snapshot", tick: end.tick }));
  // Best effort: the page goes away with its link right after, so the others may only hear
  // the leave if its packets left before (the README says so).
  addEventListener("pagehide", () => {
    if (joined && end === undefined) worker.postMessage({ type: "leave" });
  });
  requestAnimationFrame(animate);
}

main().catch((error) => fail(`the game cannot start: ${error instanceof Error ? error.message : String(error)}`));
