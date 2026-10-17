//! Workspace tasks, run with `cargo xtask <task>`.
//!
//! `wasm-game <package> <out-dir>` builds a game library for the browser and writes its
//! JavaScript glue (`game.js`) and module (`game_bg.wasm`) to `<out-dir>`.

use std::error::Error;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use wasm_bindgen_cli_support::Bindgen;

const USAGE: &str = "usage: cargo xtask wasm-game <package> <out-dir>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [task, package, out_dir] if task == "wasm-game" => wasm_game(package, out_dir),
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
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let target = "wasm32-unknown-unknown";
    let status = Command::new(cargo)
        .args(["build", "--locked", "--release", "--target", target])
        .args(["-p", package, "--lib"])
        .status()?;
    if !status.success() {
        return Err(format!("building {package} for {target} failed").into());
    }
    let module = target_dir()
        .join(target)
        .join("release")
        .join(format!("{}.wasm", package.replace('-', "_")));
    Bindgen::new()
        .input_path(module)
        .web(true)?
        .typescript(false)
        .omit_default_module_path(false)
        .out_name("game")
        .generate(out_dir)?;
    Ok(())
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"))
}
