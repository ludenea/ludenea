// Measures the desktop editor's frame times on the swarm, unattended: builds the desktop
// application, opens a copy of `examples/swarm` in it (a project made by `ludenea new` with the
// swarm's sources and assets, as the end-to-end tests make it, so the example itself gains no
// files) with `LUDENEA_FRAME_TIMES`, which plays the scene
// for 10 seconds with the viewport shown through the game camera and writes the time between
// animation frames, then prints their percentiles. Fails when the 95th percentile is above
// 20 ms, when the game pushed no render frames, or when the editor reports an error.
//
//   pnpm -C editor measure:desktop
//
// The window must stay on screen: a hidden window draws no frames.

import { execFileSync, spawn } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { frameTimesVerdict } from "../apps/web/src/frame-times.ts";

// The first launch builds the swarm in release before it can play.
const TIMEOUT_MS = 30 * 60 * 1000;

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const desktop = join(repository, "editor/apps/desktop");
const binary = join(desktop, "src-tauri/target/release", process.platform === "win32" ? "ludenea-desktop.exe" : "ludenea-desktop");

/** Whether the macOS session's screen is locked: a locked screen hides every window. */
function screenLocked() {
  if (process.platform !== "darwin") return false;
  return /<key>CGSSessionScreenIsLocked<\/key>\s*<true\/>/.test(execFileSync("ioreg", ["-n", "Root", "-d1", "-a"], { encoding: "utf8" }));
}

const swarm = join(repository, "examples/swarm");
const ludenea = join(repository, "target/debug", process.platform === "win32" ? "ludenea.exe" : "ludenea");

/** A project named `swarm` in `directory` whose game is the swarm example, with its scene as the default one. */
function swarmProject(directory) {
  const root = join(directory, "swarm");
  execFileSync(ludenea, ["new", root, "--engine-path", repository], { stdio: "inherit" });
  for (const file of ["lib.rs", "assets.rs"]) cpSync(join(swarm, "src", file), join(root, "src", file));
  for (const folder of ["assets", "scenes"]) rmSync(join(root, folder), { recursive: true, force: true });
  mkdirSync(join(root, "scenes"));
  cpSync(join(swarm, "assets"), join(root, "assets"), { recursive: true });
  const manifest = join(root, "ludenea.toml");
  writeFileSync(manifest, readFileSync(manifest, "utf8").replace(/^default_scene = .*$/m, 'default_scene = "assets/swarm.scene.ron"'));
  // The swarm draws its sprite sheet with `png` (its asset generator), at the engine's version.
  const png = /^png = .*$/m.exec(readFileSync(join(repository, "Cargo.toml"), "utf8"))[0];
  const cargo = join(root, "Cargo.toml");
  writeFileSync(cargo, readFileSync(cargo, "utf8").replace("[dependencies]\n", `[dependencies]\n${png}\n`));
  return root;
}

/** Launches the built editor on a copy of the swarm and resolves with the report it wrote. */
function measure() {
  const directory = mkdtempSync(join(tmpdir(), "ludenea-frame-times-"));
  const report = join(directory, "report.json");
  let project;
  try {
    project = swarmProject(directory);
  } catch (error) {
    rmSync(directory, { recursive: true, force: true });
    throw error;
  }
  return new Promise((resolvePromise, reject) => {
    const editor = spawn(binary, [project], {
      // The game builds with the engine's own build artifacts.
      env: { ...process.env, CARGO_TARGET_DIR: join(repository, "target"), LUDENEA_ENGINE_PATH: repository, LUDENEA_FRAME_TIMES: report },
      stdio: "inherit",
    });
    const timer = setTimeout(() => {
      editor.kill();
      reject(new Error(`the editor wrote no frame times within ${TIMEOUT_MS / 60_000} minutes; launch it on a copy of examples/swarm to see why`));
    }, TIMEOUT_MS);
    editor.on("error", (error) => reject(new Error(`cannot launch ${binary}: ${error.message}`)));
    editor.on("exit", (code, signal) => {
      clearTimeout(timer);
      try {
        if (code !== 0) throw new Error(`the editor exited with ${signal ?? `code ${code}`} before writing the frame times`);
        resolvePromise(JSON.parse(readFileSync(report, "utf8")));
      } catch (error) {
        reject(error);
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  });
}

try {
  if (screenLocked()) throw new Error("the screen is locked: unlock it, since the editor window must be on screen to draw frames");
  execFileSync("cargo", ["build", "--locked", "-p", "ludenea-serve"], { cwd: repository, stdio: "inherit" });
  // `tauri build` builds the front end, then the backend in release with the front end inside.
  execFileSync("pnpm", ["exec", "tauri", "build", "--no-bundle", "--", "--locked"], { cwd: desktop, stdio: "inherit" });
  const { line, failed } = frameTimesVerdict(await measure(), "desktop editor, swarm");
  console.log(line);
  process.exitCode = failed ? 1 : 0;
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
