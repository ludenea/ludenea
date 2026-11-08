//! The run-time builds of the plugins the tiers load: the WebAssembly module
//! and the Rust dynamic library with Cargo (the pinned toolchain), the C
//! library with CMake and the platform's default C compiler, twice. Throwaway,
//! never product code.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// What one build gave: its duration, whether it succeeded, the artifact it
/// produced, and for the C builds the compiler and flags CMake recorded.
#[derive(Clone, Debug)]
pub struct BuildOutcome {
    pub what: String,
    pub seconds: f64,
    pub succeeded: bool,
    pub stderr_tail: String,
    pub artifact: Option<PathBuf>,
    pub compiler: String,
    pub flags: String,
}

impl BuildOutcome {
    fn failed(what: &str, seconds: f64, error: String) -> BuildOutcome {
        BuildOutcome { what: what.to_string(), seconds, succeeded: false, stderr_tail: error, artifact: None, compiler: String::new(), flags: String::new() }
    }
}

/// The spikes workspace: the parent of this crate's directory.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the spike lives in the workspace").to_path_buf()
}

fn cargo() -> Command {
    Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn tail(text: &str) -> String {
    text.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect()
}

/// Runs a command to completion; the error of a failed run is its standard
/// error's tail.
fn run(command: &mut Command) -> Result<String, String> {
    let output = command.stdin(Stdio::null()).output().map_err(|error| format!("{error}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!("exit {:?}: {}", output.status.code(), tail(&stderr)))
    }
}

fn cargo_build(root: &Path, target_dir: &Path, package: &str, target: Option<&str>) -> Result<(), String> {
    let mut command = cargo();
    command.current_dir(root).args(["build", "--release", "--locked", "--package", package, "--target-dir"]).arg(target_dir);
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    run(&mut command).map(|_| ())
}

/// Installs the WebAssembly target for the pinned toolchain (idempotent) and
/// builds the module crate; the artifact is the `.wasm` file.
pub fn build_module(root: &Path, target_dir: &Path) -> BuildOutcome {
    let what = "motion-module (wasm32-unknown-unknown, cargo)";
    let started = Instant::now();
    if let Err(error) = run(Command::new("rustup").current_dir(root).args(["target", "add", WASM_TARGET])) {
        return BuildOutcome::failed(what, started.elapsed().as_secs_f64(), format!("rustup target add: {error}"));
    }
    match cargo_build(root, target_dir, "motion-module", Some(WASM_TARGET)) {
        Ok(()) => {
            let artifact = target_dir.join(WASM_TARGET).join("release").join("motion_module.wasm");
            let exists = artifact.is_file();
            BuildOutcome {
                what: what.to_string(),
                seconds: started.elapsed().as_secs_f64(),
                succeeded: exists,
                stderr_tail: if exists { String::new() } else { format!("{} not produced", artifact.display()) },
                artifact: exists.then_some(artifact),
                compiler: "rustc (pinned toolchain)".to_string(),
                flags: "release profile".to_string(),
            }
        }
        Err(error) => BuildOutcome::failed(what, started.elapsed().as_secs_f64(), error),
    }
}

/// Builds the Rust dynamic library; the artifact is the platform's shared library.
pub fn build_native(root: &Path, target_dir: &Path) -> BuildOutcome {
    let what = "motion-native (rust cdylib, cargo)";
    let started = Instant::now();
    match cargo_build(root, target_dir, "motion-native", None) {
        Ok(()) => {
            let artifact = target_dir.join("release").join(format!("{}motion_native{}", env::consts::DLL_PREFIX, env::consts::DLL_SUFFIX));
            let exists = artifact.is_file();
            BuildOutcome {
                what: what.to_string(),
                seconds: started.elapsed().as_secs_f64(),
                succeeded: exists,
                stderr_tail: if exists { String::new() } else { format!("{} not produced", artifact.display()) },
                artifact: exists.then_some(artifact),
                compiler: "rustc (pinned toolchain)".to_string(),
                flags: "release profile".to_string(),
            }
        }
        Err(error) => BuildOutcome::failed(what, started.elapsed().as_secs_f64(), error),
    }
}

/// Builds the C library with CMake in release mode, with the compiler's
/// default flags or the permitted configuration, and installs it under
/// `build_dir/install`; the artifact is the installed shared library.
pub fn build_c(root: &Path, build_dir: &Path, permitted: bool) -> BuildOutcome {
    let what = if permitted { "motion-c (cmake, permitted flags)" } else { "motion-c (cmake, default flags)" };
    let started = Instant::now();
    let source = root.join("plugin-tiers").join("motion-c");
    let option = if permitted { "-DMOTION_PERMITTED=ON" } else { "-DMOTION_PERMITTED=OFF" };
    let configure = run(Command::new("cmake").arg("-S").arg(&source).arg("-B").arg(build_dir).args(["-DCMAKE_BUILD_TYPE=Release", option]));
    if let Err(error) = configure {
        return BuildOutcome::failed(what, started.elapsed().as_secs_f64(), format!("cmake configure: {error}"));
    }
    if let Err(error) = run(Command::new("cmake").arg("--build").arg(build_dir).args(["--config", "Release"])) {
        return BuildOutcome::failed(what, started.elapsed().as_secs_f64(), format!("cmake build: {error}"));
    }
    let prefix = build_dir.join("install");
    if let Err(error) = run(Command::new("cmake").arg("--install").arg(build_dir).args(["--config", "Release", "--prefix"]).arg(&prefix)) {
        return BuildOutcome::failed(what, started.elapsed().as_secs_f64(), format!("cmake install: {error}"));
    }
    let artifact = fs::read_dir(prefix.join("lib"))
        .ok()
        .and_then(|entries| {
            entries.flatten().map(|entry| entry.path()).find(|path| {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                name.contains("motion_c") && name.ends_with(env::consts::DLL_SUFFIX)
            })
        });
    let record = fs::read_to_string(build_dir.join("motion-build.txt")).unwrap_or_default();
    let field = |key: &str| record.lines().find_map(|line| line.strip_prefix(key).map(|v| v.trim().to_string())).unwrap_or_default();
    BuildOutcome {
        what: what.to_string(),
        seconds: started.elapsed().as_secs_f64(),
        succeeded: artifact.is_some(),
        stderr_tail: if artifact.is_some() { String::new() } else { format!("no shared library under {}", prefix.join("lib").display()) },
        artifact,
        compiler: field("compiler="),
        flags: field("flags="),
    }
}

/// A fresh temporary directory for one build, named after the spike.
pub fn scratch(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("plugin-tiers-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("the temporary directory is writable");
    dir
}
