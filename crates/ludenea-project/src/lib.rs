//! Ludenea projects on disk: the manifest, the template, stable asset identifiers, and file
//! watching. It is the base of the project service; nothing here is part of the simulation.

mod files;
mod manifest;
mod meta;
mod template;

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::{fs, io};

use ludenea_ecs::AssetId;

pub use files::{FileChange, FileEntry};
pub use manifest::Manifest;
pub use meta::AssetKind;

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
    #[error("cannot watch the project: {0}")]
    Watch(String),
}

/// Something worth telling the user that is not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectWarning {
    /// A `.meta` whose file is gone, usually because the file was moved outside the editor.
    /// `path` is where the file used to be.
    OrphanMeta { id: AssetId, path: String },
}

#[derive(Default, Debug)]
struct Warnings {
    reported: BTreeSet<AssetId>,
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
    /// `engine_path`.
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
        fs::create_dir_all(&root).map_err(|source| io_error(&root, source))?;
        template::write(&root, name, &engine_path).map_err(|source| io_error(&root, source))?;
        Project::open(&root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The absolute path of a project-relative `path`, which must stay inside the project.
    fn resolve(&self, path: &str) -> Result<PathBuf, ProjectError> {
        let relative = Path::new(path);
        let inside = relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
        if path.is_empty() || !inside {
            return Err(ProjectError::OutsideProject(path.to_string()));
        }
        Ok(self.root.join(relative))
    }
}

fn io_error(path: &Path, source: io::Error) -> ProjectError {
    ProjectError::Io {
        path: path.to_path_buf(),
        source,
    }
}
