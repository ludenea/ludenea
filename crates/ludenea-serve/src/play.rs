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
    exit_code(&binary, status.code())
}

/// The game's exit `code` as this tool's own, or why it cannot be passed on.
fn exit_code(binary: &Path, code: Option<i32>) -> Result<u8, String> {
    match code {
        Some(code) => u8::try_from(code).map_err(|_| {
            format!(
                "{} exited with the status {code}, which is not an exit code",
                binary.display()
            )
        }),
        None => Err(format!(
            "{} ended without an exit code (killed by a signal)",
            binary.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_passes_through_as_is() {
        assert_eq!(exit_code(Path::new("game"), Some(3)), Ok(3));
        assert_eq!(exit_code(Path::new("game"), Some(0)), Ok(0));
    }

    #[test]
    fn an_out_of_range_code_is_an_error_naming_it() {
        let error = exit_code(Path::new("game"), Some(-1073741819)).unwrap_err();
        assert!(
            error.contains("game") && error.contains("-1073741819"),
            "{error}"
        );
        assert!(exit_code(Path::new("game"), Some(256)).is_err());
    }

    #[test]
    fn no_code_is_an_error() {
        let error = exit_code(Path::new("game"), None).unwrap_err();
        assert!(error.contains("signal"), "{error}");
    }
}
