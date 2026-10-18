//! Ludenea projects on disk: the manifest, the template, stable asset identifiers, file
//! watching, and editor plugin bundles. It is the base of the project service; nothing here is
//! part of the simulation.

mod build;
mod files;
mod game;
mod manifest;
mod meta;
mod plugins;
pub mod protocol;
pub mod service;
mod template;
mod types;

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ludenea_ecs::AssetId;

pub use build::{BuildArtifact, BuildError, BuildEvent, BuildTarget, Diagnostic};
pub use files::{FileChange, FileEntry};
pub use game::{GameOutput, GameProcess, OutputStream};
pub use manifest::Manifest;
pub use meta::AssetKind;
pub use plugins::{PluginBundle, PluginEvent, PluginSource};

#[derive(thiserror::Error, Debug)]
pub enum ProjectError {
    #[error("invalid project name {0:?}: names must match ^[a-z][a-z0-9_]*$")]
    InvalidName(String),
    #[error("{0:?} is outside the project")]
    OutsideProject(String),
    #[error("{} already exists", .0.display())]
    AlreadyExists(PathBuf),
    #[error("{}: {source}", .path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid manifest {}: {message}", .path.display())]
    Manifest { path: PathBuf, message: String },
    #[error("{0}")]
    Assets(String),
    #[error("cannot watch the project: {0}")]
    Watch(String),
    #[error(
        "no engine path to create the project with: set the editor setting ludenea.engine-path"
    )]
    NoEnginePath,
    #[error("{} is not a Ludenea engine repository (it has no crates/ludenea)", .0.display())]
    NotAnEngine(PathBuf),
}

/// Something worth telling the user that is not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectWarning {
    /// A `.meta` whose file is gone, usually because the file was moved outside the editor.
    /// `path` is where the file used to be.
    OrphanMeta { id: AssetId, path: String },
    /// A `.meta` that cannot be read, parsed, or created. It is left untouched and its file is
    /// listed with the identifier `AssetId(0)` until the meta is repaired.
    InvalidMeta { path: String, reason: String },
}

#[derive(Default, Debug)]
struct Warnings {
    reported: BTreeSet<AssetId>,
    invalid_reported: BTreeSet<String>,
    pending: Vec<ProjectWarning>,
}

#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    manifest: Manifest,
    warnings: Mutex<Warnings>,
}

impl Project {
    /// Opens the project at `root`, which holds a `ludenea.toml`.
    pub fn open(root: &Path) -> Result<Project, ProjectError> {
        let root = root
            .canonicalize()
            .map_err(|source| io_error(root, source))?;
        let path = root.join("ludenea.toml");
        let text = fs::read_to_string(&path).map_err(|source| io_error(&path, source))?;
        let manifest = toml::from_str(&text).map_err(|error| ProjectError::Manifest {
            path,
            message: error.message().to_string(),
        })?;
        Ok(Project {
            root,
            manifest,
            warnings: Mutex::default(),
        })
    }

    /// Creates `parent/name` from the template, with the game depending on the engine at
    /// `engine_path`. On failure nothing is left behind.
    pub fn create(parent: &Path, name: &str, engine_path: &Path) -> Result<Project, ProjectError> {
        if !template::is_valid_name(name) {
            return Err(ProjectError::InvalidName(name.to_string()));
        }
        let root = parent.join(name);
        if root.exists() {
            return Err(ProjectError::AlreadyExists(root));
        }
        let engine_path = engine_path
            .canonicalize()
            .map_err(|source| io_error(engine_path, source))?;
        if !engine_path.join("crates/ludenea/Cargo.toml").is_file() {
            return Err(ProjectError::NotAnEngine(engine_path));
        }
        fs::create_dir_all(&root).map_err(|source| io_error(&root, source))?;
        let created =
            template::write(&root, name, &engine_path).and_then(|()| Project::open(&root));
        if created.is_err() {
            // `root` did not exist before: everything in it is ours.
            let _ = fs::remove_dir_all(&root);
        }
        created
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The absolute path of a project-relative `path`, which must stay inside the project.
    /// A symbolic link at the end is not followed: deleting or renaming acts on the link.
    fn resolve(&self, path: &str) -> Result<PathBuf, ProjectError> {
        self.locate(path).map(|(joined, _)| joined)
    }

    /// Where `path` really is, symbolic links followed, which must be inside the project:
    /// reading and writing act on the target of a link.
    fn resolve_target(&self, path: &str) -> Result<PathBuf, ProjectError> {
        self.locate(path).map(|(_, real)| real)
    }

    /// `path` joined to the root, and the same with every existing part made canonical.
    fn locate(&self, path: &str) -> Result<(PathBuf, PathBuf), ProjectError> {
        let relative = Path::new(path);
        let inside = relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
        if path.is_empty() || !inside {
            return Err(ProjectError::OutsideProject(path.to_string()));
        }
        let joined = self.root.join(relative);
        // Symlinks can lead out of the project: check where the deepest existing part really is.
        let mut existing = joined.as_path();
        while existing.symlink_metadata().is_err() {
            existing = existing.parent().unwrap_or(&self.root);
        }
        match existing.canonicalize() {
            Ok(real) if real.starts_with(&self.root) => {
                let missing = joined
                    .strip_prefix(existing)
                    .expect("an ancestor of the path");
                // Joining an empty path would add a trailing separator.
                let real = if missing.as_os_str().is_empty() {
                    real
                } else {
                    real.join(missing)
                };
                Ok((joined, real))
            }
            _ => Err(ProjectError::OutsideProject(path.to_string())),
        }
    }
}

/// Opens `real`, a path [`Project::resolve_target`] checked, without following a symbolic
/// link in its last part: a link swapped in after the check cannot lead out of the project.
/// ponytail: a directory higher up swapped for a link between the check and the open still
/// leads out; closing that needs a lookup relative to an open handle for every directory.
fn open_unfollowed(real: &Path) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NOFOLLOW);
    options.open(real)
}

/// How often a watcher thread checks whether its [`Watch`] is gone.
const WATCH_POLL: Duration = Duration::from_millis(100);

/// What a background watcher reports, read like a [`Receiver`]. The watcher stops soon after
/// this is dropped, whether or not anything changes.
pub struct Watch<T> {
    receiver: Receiver<T>,
    dropped: Arc<AtomicBool>,
}

impl<T> Watch<T> {
    /// A watch, the sender its watcher reports on, and the flag set when the watch is dropped.
    fn new() -> (Watch<T>, Sender<T>, Arc<AtomicBool>) {
        let (sender, receiver) = mpsc::channel();
        let dropped = Arc::new(AtomicBool::new(false));
        let watch = Watch {
            receiver,
            dropped: Arc::clone(&dropped),
        };
        (watch, sender, dropped)
    }
}

impl<T> Deref for Watch<T> {
    type Target = Receiver<T>;

    fn deref(&self) -> &Receiver<T> {
        &self.receiver
    }
}

impl<T> Drop for Watch<T> {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

/// Waits for the next raw event of a watcher, giving up once its [`Watch`] is dropped.
fn next_raw<T>(raw: &Receiver<T>, dropped: &AtomicBool) -> Option<T> {
    loop {
        if dropped.load(Ordering::SeqCst) {
            return None;
        }
        match raw.recv_timeout(WATCH_POLL) {
            Ok(event) => return Some(event),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn io_error(path: &Path, source: io::Error) -> ProjectError {
    ProjectError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn open_unfollowed_refuses_a_symbolic_link() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, "outside").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_unfollowed(&target).is_ok());
        assert!(open_unfollowed(&link).is_err());
    }
}
