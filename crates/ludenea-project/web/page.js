// The page of a Ludenea web build. The simulation runs in `worker.js`; this page draws its
// frames with the game's `WebRenderer`, between the last two ticks, runs the lobby with the
// game's `WebLobby` (or skips it on `?players=N`), sends the keys and gamepads held, and saves
// the replay. `?ticks=N` stops play at tick N. Runtime code: it reads the clock to draw and
// changes the game only through the worker's host commands.

import init, { WebLobby, WebRenderer, automaticClaimsJson } from "./game.js";

const canvas = document.getElementById("game");
const status = document.getElementById("status");
const lobbyView = document.getElementById("lobby");
const slots = document.getElementById("slots");
const save = document.getElementById("save");

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
    lobbyView.hidden = true;
    startMatch(pressed.claims);
  } else if (pressed.type !== "ignored") {
    showLobby();
  }
}

function startMatch(claims) {
  for (const code of held) heldAtStart.add(code);
  held.clear();
  playing = true;
  save.disabled = false;
  say(`Playing with ${claims.length} ${claims.length === 1 ? "player" : "players"}`);
  worker.postMessage({ type: "start", claims });
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
      pendingFrame = data.frame;
      progress = data.progress;
      arrival = performance.now();
      document.body.dataset.tick = String(data.tick);
      document.body.dataset.hash = data.hash;
      break;
    case "replay":
      download(data.replay);
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

function download(base64) {
  const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
  const link = document.createElement("a");
  link.href = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
  link.download = `${name}.replay`;
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
  await init();
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
  requestAnimationFrame(animate);
}

main().catch((error) => fail(`the game cannot start: ${error instanceof Error ? error.message : String(error)}`));
