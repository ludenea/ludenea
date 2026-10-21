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

/** Keys the browser keeps for itself even during a match. */
const BROWSER_KEYS = /^F\d+$/;
/** A gamepad button counts as pressed above this value, as the game's input reads it. */
const PRESSED = 0.5;

let failed = false;
let name = "game";
let tickRate = 60;
let renderer;
let lobby;
let worker;
/** The JSON of the frames of the last two ticks, and whether the renderer has them. */
let previousFrame;
let currentFrame;
let framesSet = true;
/** The host's progress into the next tick when the current frame arrived, and when it did. */
let progress = 0;
let arrival = 0;
let playing = false;
const held = new Set();
let sentDevices = "";
/** The buttons each gamepad slot held at the last look, for the lobby's presses. */
let lobbyButtons = [];

function say(message) {
  if (!failed) status.textContent = message;
}

function fail(message) {
  if (failed) return;
  failed = true;
  playing = false;
  status.textContent = message;
  document.body.dataset.failed = "true";
  worker?.terminate();
}

/** `claim`'s devices in words, as the native window's title names them. */
function devices(claim) {
  const keyboard = { Whole: "keyboard", Left: "left keyboard half", Right: "right keyboard half" }[claim.keyboard];
  const gamepad = claim.gamepad === null ? undefined : `gamepad ${claim.gamepad + 1}`;
  return [keyboard, gamepad].filter((part) => part !== undefined).join(" and ");
}

function showLobby() {
  const claims = JSON.parse(lobby.claimsJson());
  for (const [index, slot] of [...slots.children].entries()) {
    const claim = claims[index];
    slot.textContent = claim === undefined ? "Free" : `Player ${index + 1}: ${devices(claim)}`;
    slot.toggleAttribute("data-claimed", claim !== undefined);
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
    if (!framesSet && currentFrame !== undefined) {
      framesSet = true;
      const report = JSON.parse(renderer.set_frames(previousFrame, currentFrame, "{}"));
      if (report.missing.length > 0) console.warn(`${report.missing.length} sprites or tiles have no texture`, report.missing);
    }
    if (currentFrame !== undefined) {
      const alpha = Math.min(Math.max(progress + ((performance.now() - arrival) * tickRate) / 1000, 0), 1);
      renderer.draw(alpha, undefined);
    }
  } catch (error) {
    // A canvas without a texture this frame draws again on the next one; it says so.
    console.error(String(error));
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
      previousFrame = currentFrame;
      currentFrame = data.frame;
      framesSet = false;
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
    case "failed":
      fail(data.message);
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
  URL.revokeObjectURL(link.href);
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
    if (BROWSER_KEYS.test(event.code) || event.ctrlKey || event.metaKey) return;
    event.preventDefault();
    held.add(event.code);
    if (lobby && !lobby.started && !event.repeat) lobbyPress(lobby.pressKey(event.code));
  });
  addEventListener("keyup", (event) => held.delete(event.code));
  // Keys released while the page had no focus never report it.
  addEventListener("blur", () => held.clear());
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
