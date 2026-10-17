use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use cargo_metadata::{Message, diagnostic::Diagnostic as CargoDiagnostic};
use serde::{Deserialize, Serialize};
use wasm_bindgen_cli_support::Bindgen;
use xxhash_rust::xxh3::xxh3_64;

use crate::Project;

const WASM_TARGET: &str = "wasm32-unknown-unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildTarget {
    Native,
    Wasm,
}

impl BuildTarget {
    fn name(self) -> &'static str {
        match self {
            BuildTarget::Native => "native",
            BuildTarget::Wasm => "wasm",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BuildEvent {
    /// A line of cargo's own output (`Compiling ...`, rendered diagnostics).
    Line {
        text: String,
    },
    /// `total` is the number of artifacts of the previous build of this target, if any.
    Progress {
        done: u32,
        total: Option<u32>,
    },
    Diagnostic(Diagnostic),
    Finished {
        ok: bool,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct Diagnostic {
    pub level: String,
    pub message: String,
    /// Relative to the project root.
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildArtifact {
    Native {
        binary: PathBuf,
    },
    /// Holds `game.js` and `game_bg.wasm`; named after the hash of the module.
    Wasm {
        dir: PathBuf,
    },
}

#[derive(thiserror::Error, Debug)]
pub enum BuildError {
    #[error("cannot run cargo: {0}")]
    Io(#[from] io::Error),
    #[error("the build failed")]
    Failed,
    #[error("cargo produced no {0}")]
    MissingArtifact(&'static str),
    #[error("wasm-bindgen failed: {0}")]
    Bindgen(String),
}

impl Project {
    /// Builds the game on a background thread, reporting on `output`. The last event is always
    /// `Finished`. Builds are outside the simulation: they use wall-clock tools freely.
    pub fn build(
        &self,
        target: BuildTarget,
        output: Sender<BuildEvent>,
    ) -> JoinHandle<Result<BuildArtifact, BuildError>> {
        let root = self.root.clone();
        let package = self.manifest.name.clone();
        thread::spawn(move || {
            let result = run(&root, &package, target, &output);
            let _ = output.send(BuildEvent::Finished { ok: result.is_ok() });
            result
        })
    }
}

fn run(
    root: &Path,
    package: &str,
    target: BuildTarget,
    output: &Sender<BuildEvent>,
) -> Result<BuildArtifact, BuildError> {
    let count_file = root
        .join(".ludenea/build")
        .join(format!("{}.count", target.name()));
    let total = fs::read_to_string(&count_file)
        .ok()
        .and_then(|text| text.trim().parse().ok());

    let mut command = Command::new("cargo");
    // Plain lines for the console, whatever the environment asks for (`tauri dev` asks for
    // colors and progress bars).
    command
        .current_dir(root)
        .args(["build", "--release", "--color", "never"])
        .env("CARGO_TERM_PROGRESS_WHEN", "never");
    match target {
        BuildTarget::Native => command.args(["--bin", package]),
        BuildTarget::Wasm => command.args(["--lib", "--target", WASM_TARGET]),
    };
    let mut child = command
        .arg("--message-format=json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take().expect("stderr is piped");
    let stderr_output = output.clone();
    let lines = thread::spawn(move || {
        for text in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = stderr_output.send(BuildEvent::Line { text });
        }
    });

    let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut done = 0;
    let mut produced: Option<PathBuf> = None;
    for message in Message::parse_stream(stdout).map_while(Result::ok) {
        match message {
            Message::CompilerArtifact(artifact) => {
                done += 1;
                let _ = output.send(BuildEvent::Progress { done, total });
                if artifact.target.name == package {
                    let wanted = match target {
                        BuildTarget::Native => artifact.executable.map(Into::into),
                        BuildTarget::Wasm => artifact
                            .filenames
                            .into_iter()
                            .find(|file| file.extension() == Some("wasm"))
                            .map(Into::into),
                    };
                    produced = wanted.or(produced);
                }
            }
            Message::CompilerMessage(message) => {
                // `json-render-diagnostics` would drop the structured form, so render here.
                if let Some(text) = &message.message.rendered {
                    let _ = output.send(BuildEvent::Line { text: text.clone() });
                }
                let _ = output.send(BuildEvent::Diagnostic(diagnostic(message.message)));
            }
            _ => {}
        }
    }
    let status = child.wait()?;
    let _ = lines.join();
    if !status.success() {
        return Err(BuildError::Failed);
    }

    // Best effort: a missing count only means no total next time.
    if let Some(directory) = count_file.parent() {
        let _ =
            fs::create_dir_all(directory).and_then(|()| fs::write(&count_file, done.to_string()));
    }
    match target {
        BuildTarget::Native => produced
            .map(|binary| BuildArtifact::Native { binary })
            .ok_or(BuildError::MissingArtifact("binary")),
        BuildTarget::Wasm => {
            let module = produced.ok_or(BuildError::MissingArtifact("module"))?;
            bind(root, &module).map(|dir| BuildArtifact::Wasm { dir })
        }
    }
}

fn diagnostic(message: CargoDiagnostic) -> Diagnostic {
    let span = message.spans.iter().find(|span| span.is_primary);
    Diagnostic {
        level: format!("{:?}", message.level).to_lowercase(),
        message: message.message,
        file: span.map(|span| span.file_name.clone()),
        line: span.and_then(|span| u32::try_from(span.line_start).ok()),
        column: span.and_then(|span| u32::try_from(span.column_start).ok()),
    }
}

/// Generates the browser glue for `module` into `.ludenea/build/wasm/<hash of the module>/`.
fn bind(root: &Path, module: &Path) -> Result<PathBuf, BuildError> {
    let parent = root.join(".ludenea/build/wasm");
    let staging = parent.join("staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    Bindgen::new()
        .input_path(module)
        .web(true)
        .and_then(|bindgen| {
            bindgen
                .typescript(false)
                .omit_default_module_path(false)
                .out_name("game")
                .generate(&staging)
        })
        .map_err(|error| BuildError::Bindgen(error.to_string()))?;
    let hash = xxh3_64(&fs::read(staging.join("game_bg.wasm"))?);
    let directory = parent.join(format!("{hash:016x}"));
    if directory.exists() {
        fs::remove_dir_all(&staging)?;
    } else {
        fs::rename(&staging, &directory)?;
    }
    Ok(directory)
}
