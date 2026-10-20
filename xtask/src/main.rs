//! Workspace tasks, run with `cargo xtask <task>`.
//!
//! `check` runs the lint checks CI runs: formatting (the engine and the desktop backend),
//! clippy on the host (with and without the wgpu renderer) and on WebAssembly, and the API documentation, with warnings denied. It
//! stops at the first that fails.
//!
//! `wasm-game <package> <out-dir>` builds a game library for the browser and writes its
//! JavaScript glue (`game.js`) and module (`game_bg.wasm`) to `<out-dir>`.
//!
//! `wasm-renderer <out-dir>` builds the viewport renderer (`ludenea-draw` with its
//! `render` feature) and writes `debug_draw.js`, `debug_draw.d.ts`, and `debug_draw_bg.wasm`.
//!
//! `wasm-render <out-dir>` builds the renderer for the browser (`ludenea-render` with its
//! `web` feature) and writes `ludenea_render.js`, `ludenea_render.d.ts`, and
//! `ludenea_render_bg.wasm`.

use std::error::Error;
use std::path::PathBuf;
use std::process::{Command, ExitCode, ExitStatus};

use cargo_metadata::MetadataCommand;
use wasm_bindgen_cli_support::Bindgen;

const USAGE: &str = "usage: cargo xtask check | cargo xtask wasm-game <package> <out-dir> \
                     | cargo xtask wasm-renderer <out-dir> | cargo xtask wasm-render <out-dir>";
const TARGET: &str = "wasm32-unknown-unknown";

/// The cargo commands of `check`, in order, run from the workspace root.
const CHECKS: &[&str] = &[
    "fmt --all --check",
    // The desktop backend, outside the workspace with a lockfile of its own.
    "fmt --manifest-path editor/apps/desktop/src-tauri/Cargo.toml --all --check",
    "clippy --locked --workspace --all-targets -- -D warnings",
    // The wgpu renderer, off in the workspace build.
    "clippy --locked -p ludenea-render --features gpu --all-targets -- -D warnings",
    // Warnings are denied through `RUSTDOCFLAGS`, which `check` sets.
    "doc --locked --no-deps --workspace",
    // The browser: the facade's entry, the Web Worker transport, the viewport renderer, and
    // the renderer on WebGPU and WebGL2.
    "clippy --locked --target wasm32-unknown-unknown -p ludenea -p ludenea-control -p boxes --lib -- -D warnings",
    "clippy --locked --target wasm32-unknown-unknown -p ludenea-draw --features render --lib -- -D warnings",
    "clippy --locked --target wasm32-unknown-unknown -p ludenea-render --features web --lib -- -D warnings",
    // WebAssembly outside the browser: the command line tool that verifies the goldens.
    "clippy --locked --target wasm32-wasip1 -p boxes --bins -- -D warnings",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [task] if task == "check" => check(),
        [task, package, out_dir] if task == "wasm-game" => wasm_game(package, out_dir),
        [task, out_dir] if task == "wasm-renderer" => {
            browser_module("ludenea-draw", "render", "debug_draw", out_dir)
        }
        [task, out_dir] if task == "wasm-render" => {
            browser_module("ludenea-render", "web", "ludenea_render", out_dir)
        }
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
        let status = run(Command::new(cargo())
            .args(command.split_whitespace())
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
            .env("RUSTDOCFLAGS", "-D warnings"))?;
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

/// Builds `package` with `feature` for the browser, sized down, and writes its JavaScript glue,
/// its TypeScript declarations, and its module to `out_dir` as `<out_name>.*`.
fn browser_module(
    package: &str,
    feature: &str,
    out_name: &str,
    out_dir: &str,
) -> Result<(), Box<dyn Error>> {
    // `cargo rustc` makes the module without declaring a `cdylib` in the crate, which would
    // otherwise link a native library on every workspace build.
    let module = build(
        package,
        &["rustc", "--features", feature, "--crate-type", "cdylib"],
        "wasm-renderer",
    )?;
    Bindgen::new()
        .input_path(module)
        .web(true)?
        .typescript(true)
        .omit_default_module_path(false)
        .out_name(out_name)
        .generate(out_dir)?;
    Ok(())
}

/// Runs `cargo <command>` on `package`'s library for the browser with `profile` and returns the
/// module path.
fn build(package: &str, command: &[&str], profile: &str) -> Result<PathBuf, Box<dyn Error>> {
    let status = run(Command::new(cargo())
        .args(command)
        .args(["--locked", "--profile", profile, "--target", TARGET])
        .args(["-p", package, "--lib"]))?;
    if !status.success() {
        return Err(format!("building {package} for {TARGET} failed").into());
    }
    Ok(target_dir()?
        .join(TARGET)
        .join(profile)
        .join(format!("{}.wasm", package.replace('-', "_"))))
}

/// Runs `command` and waits for it, naming the program and its working directory if it
/// cannot start.
fn run(command: &mut Command) -> Result<ExitStatus, Box<dyn Error>> {
    command.status().map_err(|error| {
        let directory = match command.get_current_dir() {
            Some(directory) => directory.to_path_buf(),
            None => std::env::current_dir().unwrap_or_default(),
        };
        format!(
            "could not run `{}` in {}: {error}",
            command.get_program().display(),
            directory.display()
        )
        .into()
    })
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
    const WORKFLOW: &str = include_str!("../../.github/workflows/determinism.yml");

    /// CI runs each check as a step of its own (the host clippy in the native jobs, the
    /// renderer's clippy in the render jobs, the rest once in the lint job), so a check added here must be added there too.
    #[test]
    fn ci_runs_every_check() {
        assert_eq!(missing_checks(WORKFLOW), Vec::<String>::new());
    }

    #[test]
    fn a_check_commented_out_or_in_another_job_is_missing() {
        // A Windows checkout may end lines with CRLF.
        let workflow = WORKFLOW.replace("\r\n", "\n");
        let doc = "      - run: cargo doc --locked --no-deps --workspace\n";
        let commented_out = workflow.replace(
            doc,
            "      # - run: cargo doc --locked --no-deps --workspace\n",
        );
        let host_clippy =
            "      - run: cargo clippy --locked --workspace --all-targets -- -D warnings\n";
        let elsewhere = workflow
            .replace(doc, "")
            .replace(host_clippy, &format!("{host_clippy}{doc}"));
        assert!(
            elsewhere.contains(doc),
            "the native job has no host clippy step"
        );
        let flags_in_a_comment = workflow.replace(
            "      RUSTDOCFLAGS: -D warnings\n",
            "      # RUSTDOCFLAGS: -D warnings\n",
        );
        for workflow in [commented_out, elsewhere] {
            assert_eq!(
                missing_checks(&workflow),
                ["the lint job has no `run: cargo doc --locked --no-deps --workspace` step"]
            );
        }
        assert_eq!(
            missing_checks(&flags_in_a_comment),
            ["the lint job does not set `RUSTDOCFLAGS: -D warnings`"]
        );
    }

    /// What `workflow` misses of `super::CHECKS`: a step of each, uncommented, in the job
    /// that runs it, and the lint job's `RUSTDOCFLAGS`.
    fn missing_checks(workflow: &str) -> Vec<String> {
        let mut missing = Vec::new();
        for command in super::CHECKS {
            let job = match *command {
                "clippy --locked --workspace --all-targets -- -D warnings" => "native",
                "clippy --locked -p ludenea-render --features gpu --all-targets -- -D warnings" => {
                    "render"
                }
                _ => "lint",
            };
            let step = format!("run: cargo {command}");
            if !job_lines(workflow, job).any(|line| line == format!("- {step}")) {
                missing.push(format!("the {job} job has no `{step}` step"));
            }
        }
        if !job_lines(workflow, "lint").any(|line| line == "RUSTDOCFLAGS: -D warnings") {
            missing.push("the lint job does not set `RUSTDOCFLAGS: -D warnings`".to_string());
        }
        missing
    }

    /// The trimmed lines of the job `name`, from its key to the next line indented as a job
    /// key.
    fn job_lines<'a>(workflow: &'a str, name: &str) -> impl Iterator<Item = &'a str> {
        let key = format!("  {name}:");
        workflow
            .lines()
            .skip_while(move |line| line.trim_end() != key)
            .skip(1)
            .take_while(|line| line.is_empty() || line.starts_with("   "))
            .map(str::trim)
    }

    #[test]
    fn a_command_that_cannot_start_names_the_program_and_its_directory() {
        let directory = std::env::temp_dir();
        let error = super::run(
            std::process::Command::new("ludenea-no-such-program").current_dir(&directory),
        )
        .unwrap_err();
        let expected = format!(
            "could not run `ludenea-no-such-program` in {}: ",
            directory.display()
        );
        assert!(error.to_string().starts_with(&expected), "{error}");
    }

    #[test]
    fn target_dir_is_where_cargo_builds() {
        let this_test = std::env::current_exe().unwrap().canonicalize().unwrap();
        let target = super::target_dir().unwrap().canonicalize().unwrap();
        assert!(this_test.starts_with(&target), "{}", target.display());
    }
}
