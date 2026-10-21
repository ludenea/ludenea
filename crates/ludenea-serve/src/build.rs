//! `ludenea build --web`: builds a project as a standalone web page.

use std::path::Path;
use std::sync::mpsc;

use ludenea_project::{BuildEvent, Project};

/// Builds the project at `root` as a web page into `out`, printing cargo's output.
pub fn web(root: &Path, out: &Path) -> Result<(), String> {
    let project = Project::open(root).map_err(|error| error.to_string())?;
    let (sender, receiver) = mpsc::channel();
    let build = project.build_web(out, sender);
    // The sender drops when the build thread ends, which ends this loop.
    for event in receiver {
        if let BuildEvent::Line { text } = event {
            eprintln!("{}", text.trim_end());
        }
    }
    build
        .join()
        .map_err(|_| "the build thread panicked".to_string())?
        .map(|_| ())
        .map_err(|error| error.to_string())
}
