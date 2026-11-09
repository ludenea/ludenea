//! The run-time builds of the two modules, with Cargo and the pinned
//! toolchain. Throwaway, never product code.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// What one build gave: its duration and the module's bytes, or why it failed.
pub struct Build {
    pub what: &'static str,
    pub seconds: f64,
    pub wasm: Result<Vec<u8>, String>,
}

/// The spikes workspace: the parent of this crate's directory.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the spike lives in the workspace").to_path_buf()
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command.stdin(Stdio::null()).output().map_err(|error| format!("{error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: String = stderr.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
    Err(format!("exit {:?}: {tail}", output.status.code()))
}

/// Installs the WebAssembly target for the pinned toolchain (idempotent) and
/// builds the module crate in `target_dir`, with the `variant` feature for the
/// second module.
pub fn build_module(root: &Path, target_dir: &Path, variant: bool) -> Build {
    let what = if variant { "tick-module, variant feature (wasm32-unknown-unknown)" } else { "tick-module (wasm32-unknown-unknown)" };
    let started = Instant::now();
    let wasm = (|| {
        run(Command::new("rustup").current_dir(root).args(["target", "add", WASM_TARGET])).map_err(|error| format!("rustup target add: {error}"))?;
        let mut cargo = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cargo.current_dir(root).args(["build", "--release", "--locked", "--package", "tick-module", "--target", WASM_TARGET, "--target-dir"]).arg(target_dir);
        if variant {
            cargo.args(["--features", "variant"]);
        }
        run(&mut cargo).map_err(|error| format!("cargo build: {error}"))?;
        let artifact = target_dir.join(WASM_TARGET).join("release").join("tick_module.wasm");
        fs::read(&artifact).map_err(|error| format!("reading {}: {error}", artifact.display()))
    })();
    Build { what, seconds: started.elapsed().as_secs_f64(), wasm }
}

/// A fresh temporary directory for one build, named after the spike.
pub fn scratch(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("scripting-runtime-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("the temporary directory is writable");
    dir
}
