//! Workspace tasks, run with `cargo xtask <task>`.
//!
//! `wasm-game <package> <out-dir>` builds a game library for the browser and writes its
//! JavaScript glue (`game.js`) and module (`game_bg.wasm`) to `<out-dir>`.
//!
//! `wasm-renderer <out-dir>` builds the viewport renderer (`ludenea-debug-draw` with its
//! `render` feature) and writes `debug_draw.js`, `debug_draw.d.ts`, and `debug_draw_bg.wasm`.

use std::error::Error;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use wasm_bindgen_cli_support::Bindgen;

const USAGE: &str =
    "usage: cargo xtask wasm-game <package> <out-dir> | cargo xtask wasm-renderer <out-dir>";
const TARGET: &str = "wasm32-unknown-unknown";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [task, package, out_dir] if task == "wasm-game" => wasm_game(package, out_dir),
        [task, out_dir] if task == "wasm-renderer" => wasm_renderer(out_dir),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn wasm_game(package: &str, out_dir: &str) -> Result<(), Box<dyn Error>> {
    let module = build(package, &["build"])?;
    Bindgen::new()
        .input_path(module)
        .web(true)?
        .typescript(false)
        .omit_default_module_path(false)
        .out_name("game")
        .generate(out_dir)?;
    Ok(())
}

fn wasm_renderer(out_dir: &str) -> Result<(), Box<dyn Error>> {
    // `cargo rustc` makes the module without declaring a `cdylib` in the crate, which would
    // otherwise link a native library on every workspace build.
    let package = "ludenea-debug-draw";
    let module = build(
        package,
        &["rustc", "--features", "render", "--crate-type", "cdylib"],
    )?;
    Bindgen::new()
        .input_path(module)
        .web(true)?
        .typescript(true)
        .omit_default_module_path(false)
        .out_name("debug_draw")
        .generate(out_dir)?;
    Ok(())
}

/// Runs `cargo <command>` on `package`'s library for the browser in release and returns the
/// module path.
fn build(package: &str, command: &[&str]) -> Result<PathBuf, Box<dyn Error>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(command)
        .args(["--locked", "--release", "--target", TARGET])
        .args(["-p", package, "--lib"])
        .status()?;
    if !status.success() {
        return Err(format!("building {package} for {TARGET} failed").into());
    }
    Ok(target_dir()
        .join(TARGET)
        .join("release")
        .join(format!("{}.wasm", package.replace('-', "_"))))
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"))
}
