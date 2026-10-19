//! Workspace tasks, run with `cargo xtask <task>`.
//!
//! `check` runs the checks CI runs on every platform alike: formatting (the engine and the
//! desktop backend), clippy on the host and on WebAssembly, and the API documentation, with
//! warnings denied. It stops at the first that fails.
//!
//! `wasm-game <package> <out-dir>` builds a game library for the browser and writes its
//! JavaScript glue (`game.js`) and module (`game_bg.wasm`) to `<out-dir>`.
//!
//! `wasm-renderer <out-dir>` builds the viewport renderer (`ludenea-debug-draw` with its
//! `render` feature) and writes `debug_draw.js`, `debug_draw.d.ts`, and `debug_draw_bg.wasm`.

use std::error::Error;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use cargo_metadata::MetadataCommand;
use wasm_bindgen_cli_support::Bindgen;

const USAGE: &str = "usage: cargo xtask check | cargo xtask wasm-game <package> <out-dir> \
                     | cargo xtask wasm-renderer <out-dir>";
const TARGET: &str = "wasm32-unknown-unknown";

/// The cargo commands of `check`, in order, run from the workspace root.
const CHECKS: &[&str] = &[
    "fmt --all --check",
    // The desktop backend, outside the workspace with a lockfile of its own.
    "fmt --manifest-path editor/apps/desktop/src-tauri/Cargo.toml --all --check",
    "clippy --locked --workspace --all-targets -- -D warnings",
    // Warnings are denied through `RUSTDOCFLAGS`, which `check` sets.
    "doc --locked --no-deps --workspace",
    // The browser: the facade's entry, the Web Worker transport, and the viewport renderer.
    "clippy --locked --target wasm32-unknown-unknown -p ludenea -p ludenea-control -p boxes --lib -- -D warnings",
    "clippy --locked --target wasm32-unknown-unknown -p ludenea-debug-draw --features render --lib -- -D warnings",
    // WebAssembly outside the browser: the command line tool that verifies the goldens.
    "clippy --locked --target wasm32-wasip1 -p boxes --bins -- -D warnings",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [task] if task == "check" => check(),
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

fn check() -> Result<(), Box<dyn Error>> {
    for command in CHECKS {
        eprintln!("xtask: cargo {command}");
        let status = Command::new(cargo())
            .args(command.split_whitespace())
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
            .env("RUSTDOCFLAGS", "-D warnings")
            .status()?;
        if !status.success() {
            return Err(format!("`cargo {command}` failed").into());
        }
    }
    Ok(())
}

fn wasm_game(package: &str, out_dir: &str) -> Result<(), Box<dyn Error>> {
    let module = build(package, &["build"], "release")?;
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
        "wasm-renderer",
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

/// Runs `cargo <command>` on `package`'s library for the browser with `profile` and returns the
/// module path.
fn build(package: &str, command: &[&str], profile: &str) -> Result<PathBuf, Box<dyn Error>> {
    let status = Command::new(cargo())
        .args(command)
        .args(["--locked", "--profile", profile, "--target", TARGET])
        .args(["-p", package, "--lib"])
        .status()?;
    if !status.success() {
        return Err(format!("building {package} for {TARGET} failed").into());
    }
    Ok(target_dir()?
        .join(TARGET)
        .join(profile)
        .join(format!("{}.wasm", package.replace('-', "_"))))
}

/// The cargo that runs this task, so that every run uses the same toolchain.
fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// Where cargo puts the workspace's builds, as configured (`CARGO_TARGET_DIR`,
/// `build.target-dir`, or the default).
fn target_dir() -> Result<PathBuf, Box<dyn Error>> {
    let metadata = MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../Cargo.toml"))
        .no_deps()
        .exec()?;
    Ok(metadata.target_directory.into_std_path_buf())
}

#[cfg(test)]
mod tests {
    #[test]
    fn target_dir_is_where_cargo_builds() {
        let this_test = std::env::current_exe().unwrap().canonicalize().unwrap();
        let target = super::target_dir().unwrap().canonicalize().unwrap();
        assert!(this_test.starts_with(&target), "{}", target.display());
    }
}
