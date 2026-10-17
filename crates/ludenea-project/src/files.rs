use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
use std::{fs, thread};

use ludenea_ecs::AssetId;
use notify::RecursiveMode;
use notify_debouncer_full::new_debouncer;

use crate::meta::{AssetKind, Meta};
use crate::{Project, ProjectError, ProjectWarning, io_error};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(100);
/// The directories whose files get identifiers.
const ASSET_DIRECTORIES: [&str; 2] = ["scenes", "assets"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative to the project root, with forward slashes.
    pub path: String,
    pub id: AssetId,
    pub kind: AssetKind,
}

/// A change on disk, with the path relative to the project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Changed(String),
    Created(String),
    Removed(String),
}

fn meta_path(file: &Path) -> std::path::PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".meta");
    name.into()
}

impl Project {
    /// The files under `scenes/` and `assets/`, sorted by path. Files without a meta get one.
    /// Metas without a file are queued as warnings (see [`Project::take_warnings`]).
    pub fn files(&self) -> Vec<FileEntry> {
        let mut all = Vec::new();
        for directory in ASSET_DIRECTORIES {
            collect(&self.root, directory, &mut all);
        }
        let present: BTreeSet<&str> = all.iter().map(String::as_str).collect();
        let mut entries = Vec::new();
        for path in all.iter().filter(|path| !path.ends_with(".meta")) {
            let kind = AssetKind::of(path);
            let meta_file = meta_path(&self.root.join(path));
            // ponytail: an unreadable meta is replaced, which changes the id; ask the user if it matters.
            let meta = Meta::read(&meta_file).or_else(|| Meta::create(&meta_file, kind).ok());
            if let Some(meta) = meta {
                entries.push(FileEntry {
                    path: path.clone(),
                    id: meta.id,
                    kind,
                });
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut warnings = self.warnings.lock().expect("warnings are not poisoned");
        for meta in all.iter().filter_map(|path| path.strip_suffix(".meta")) {
            if present.contains(meta) {
                continue;
            }
            let Some(found) = Meta::read(&meta_path(&self.root.join(meta))) else {
                continue;
            };
            if warnings.reported.insert(found.id) {
                warnings.pending.push(ProjectWarning::OrphanMeta {
                    id: found.id,
                    path: meta.to_string(),
                });
            }
        }
        entries
    }

    /// The warnings found since the last call.
    pub fn take_warnings(&self) -> Vec<ProjectWarning> {
        let mut warnings = self.warnings.lock().expect("warnings are not poisoned");
        std::mem::take(&mut warnings.pending)
    }

    /// Moves a file with its meta.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), ProjectError> {
        let (source, target) = (self.resolve(from)?, self.resolve(to)?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        // Give the file an identifier first so it keeps it across the move.
        self.files();
        fs::rename(&source, &target).map_err(|error| io_error(&source, error))?;
        let source_meta = meta_path(&source);
        if source_meta.exists() {
            fs::rename(&source_meta, meta_path(&target))
                .map_err(|error| io_error(&source_meta, error))?;
        }
        Ok(())
    }

    /// Deletes a file with its meta.
    pub fn delete(&mut self, path: &str) -> Result<(), ProjectError> {
        let file = self.resolve(path)?;
        fs::remove_file(&file).map_err(|error| io_error(&file, error))?;
        let _ = fs::remove_file(meta_path(&file));
        Ok(())
    }

    pub fn read_text(&self, path: &str) -> Result<String, ProjectError> {
        let file = self.resolve(path)?;
        fs::read_to_string(&file).map_err(|error| io_error(&file, error))
    }

    pub fn write_text(&mut self, path: &str, text: &str) -> Result<(), ProjectError> {
        let file = self.resolve(path)?;
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::write(&file, text).map_err(|error| io_error(&file, error))
    }

    /// Reports changes under the project root, debounced. Watching stops when the receiver is
    /// dropped (noticed at the next change).
    pub fn watch(&self) -> Result<Receiver<FileChange>, ProjectError> {
        let (raw_sender, raw_receiver) = mpsc::channel();
        let mut debouncer = new_debouncer(WATCH_DEBOUNCE, None, raw_sender)
            .map_err(|error| ProjectError::Watch(error.to_string()))?;
        debouncer
            .watch(&self.root, RecursiveMode::Recursive)
            .map_err(|error| ProjectError::Watch(error.to_string()))?;
        let (sender, receiver) = mpsc::channel();
        let root = self.root.clone();
        let mut known = BTreeSet::new();
        list_files(&root, &root, &mut known);
        // The thread owns the debouncer, which stops watching when it is dropped.
        thread::spawn(move || {
            let _debouncer = debouncer;
            for batch in raw_receiver {
                let Ok(events) = batch else { continue };
                let mut touched = BTreeSet::new();
                for path in events.iter().flat_map(|event| &event.paths) {
                    touched.extend(relative_path(&root, path));
                }
                for relative in touched {
                    let file = root.join(&relative);
                    // The event kinds are not reliable across platforms (an atomic save can
                    // look like a creation), so compare with the files known to exist.
                    let change = if file.is_dir() {
                        continue;
                    } else if !file.exists() {
                        if !known.remove(&relative) {
                            continue;
                        }
                        FileChange::Removed(relative)
                    } else if known.insert(relative.clone()) {
                        FileChange::Created(relative)
                    } else {
                        FileChange::Changed(relative)
                    };
                    if sender.send(change).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(receiver)
    }
}

/// Lists the files below `root/directory`, as relative paths with forward slashes.
fn collect(root: &Path, directory: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root.join(directory)) else {
        return;
    };
    let mut names: Vec<_> = entries.flatten().collect();
    names.sort_by_key(|entry| entry.file_name());
    for entry in names {
        let path = format!("{directory}/{}", entry.file_name().to_string_lossy());
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect(root, &path, out),
            Ok(kind) if kind.is_file() => out.push(path),
            _ => {}
        }
    }
}

/// `path` relative to `root` with forward slashes; none outside the root or in `.ludenea/` and `target/`.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy());
    let first = parts.next()?;
    if first == ".ludenea" || first == "target" {
        return None;
    }
    Some(
        std::iter::once(first)
            .chain(parts)
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Adds every file below `directory` to `out`, skipping `.ludenea/` and `target/`.
fn list_files(root: &Path, directory: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(relative) = relative_path(root, &path) else {
            continue;
        };
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => list_files(root, &path, out),
            Ok(kind) if kind.is_file() => {
                out.insert(relative);
            }
            _ => {}
        }
    }
}
