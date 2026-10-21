//! `ludenea play`: builds a project natively and runs its binary's `play`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;

use ludenea_project::{BuildArtifact, BuildEvent, BuildTarget, Project};

/// Builds the project at `root`, plays `scene` (else its `default_scene`) with `args`, and
/// returns the game's exit code.
pub fn run(root: &Path, scene: Option<&Path>, args: &[String]) -> Result<u8, String> {
    let project = Project::open(root).map_err(|error| error.to_string())?;
    let scene: PathBuf = match (scene, &project.manifest().default_scene) {
        (Some(scene), _) => scene.to_path_buf(),
        (None, Some(default)) => project.root().join(default),
        (None, None) => {
            return Err(
                "no scene to play: pass --scene or set default_scene in ludenea.toml".into(),
            );
        }
    };

    let (sender, receiver) = mpsc::channel();
    let build = project.build(BuildTarget::Native, sender);
    // The sender drops when the build thread ends, which ends this loop.
    for event in receiver {
        if let BuildEvent::Line { text } = event {
            eprintln!("{}", text.trim_end());
        }
    }
    let artifact = build
        .join()
        .map_err(|_| "the build thread panicked".to_string())?
        .map_err(|error| error.to_string())?;
    let BuildArtifact::Native { binary } = artifact else {
        return Err("the native build produced no binary".into());
    };
    if !binary.is_file() {
        return Err(format!("the built binary {} is missing", binary.display()));
    }

    let status = Command::new(&binary)
        .arg("play")
        .arg(&scene)
        .arg("--project")
        .arg(project.root())
        .args(args)
        .status()
        .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    match status.code() {
        Some(code) => Ok(u8::try_from(code).unwrap_or(2)),
        None => Err(format!(
            "{} ended without an exit code (killed by a signal)",
            binary.display()
        )),
    }
}
