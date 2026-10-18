use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ludenea_ecs::AssetId;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{Debouncer, RecommendedCache, new_debouncer};
use serde::{Deserialize, Serialize};

use crate::meta::{AssetKind, Meta};
use crate::{Project, ProjectError, ProjectWarning, Watch, io_error, next_raw, open_unfollowed};

pub(crate) const WATCH_DEBOUNCE: Duration = Duration::from_millis(100);
/// The directories whose files get identifiers.
const ASSET_DIRECTORIES: [&str; 2] = ["scenes", "assets"];
/// Top-level directories that hold no file of the user's: the editor's own, version
/// control, and build outputs.
const UNWATCHED_DIRECTORIES: [&str; 3] = [".ludenea", ".git", "target"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct FileEntry {
    /// Relative to the project root, with forward slashes.
    pub path: String,
    /// `AssetId(0)` while the file's meta is unreadable (reported once as an
    /// `InvalidMeta` warning): every such file shares it, so it names none of them.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub id: AssetId,
    pub kind: AssetKind,
}

/// A change on disk, with the path relative to the project root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
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
        let mut invalid = Vec::new();
        let mut valid = Vec::new();
        for path in all.iter().filter(|path| !path.ends_with(".meta")) {
            let kind = AssetKind::of(path);
            let meta_file = meta_path(&self.root.join(path));
            let id = match Meta::load_or_create(&meta_file, kind) {
                Ok(meta) => {
                    valid.push(format!("{path}.meta"));
                    meta.id
                }
                Err(reason) => {
                    invalid.push((format!("{path}.meta"), reason));
                    AssetId(0)
                }
            };
            entries.push(FileEntry {
                path: path.clone(),
                id,
                kind,
            });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut warnings = self.warnings.lock().expect("warnings are not poisoned");
        // A repaired meta that breaks again is news again.
        for path in &valid {
            warnings.invalid_reported.remove(path);
        }
        for (path, reason) in invalid {
            if warnings.invalid_reported.insert(path.clone()) {
                warnings
                    .pending
                    .push(ProjectWarning::InvalidMeta { path, reason });
            }
        }
        for meta in all.iter().filter_map(|path| path.strip_suffix(".meta")) {
            if present.contains(meta) {
                continue;
            }
            let Ok(found) = Meta::read(&meta_path(&self.root.join(meta))) else {
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
        let target_meta = meta_path(&target);
        for existing in [&target, &target_meta] {
            if existing.symlink_metadata().is_ok() {
                return Err(ProjectError::AlreadyExists(existing.clone()));
            }
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

    /// Reads a file, or the target of a symbolic link inside the project.
    pub fn read_text(&self, path: &str) -> Result<String, ProjectError> {
        let file = self.resolve_target(path)?;
        let mut text = String::new();
        open_unfollowed(&file)
            .and_then(|mut opened| opened.read_to_string(&mut text))
            .map_err(|error| io_error(&file, error))?;
        Ok(text)
    }

    /// Writes `text` to a temporary file in `.ludenea/` (unwatched, on the same file system),
    /// synced to disk, then renames it over `path`: neither a crash mid-write nor a power loss
    /// leaves a truncated file. The file keeps its permissions; through a symbolic link inside
    /// the project, the link's target is replaced and the link kept.
    pub fn write_text(&mut self, path: &str, text: &str) -> Result<(), ProjectError> {
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let file = self.resolve_target(path)?;
        let parent = file.parent().expect("a project file has a parent");
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        let staging = self.root.join(".ludenea");
        fs::create_dir_all(&staging).map_err(|error| io_error(&staging, error))?;
        let write = WRITES.fetch_add(1, Ordering::Relaxed);
        let temporary = staging.join(format!("write-{}-{write}.tmp", std::process::id()));
        let staged = File::create(&temporary).and_then(|mut staged| {
            staged.write_all(text.as_bytes())?;
            if let Ok(existing) = fs::metadata(&file) {
                staged.set_permissions(existing.permissions())?;
            }
            staged.sync_all()
        });
        let written = staged
            .map_err(|error| io_error(&temporary, error))
            .and_then(|()| fs::rename(&temporary, &file).map_err(|error| io_error(&file, error)));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written?;
        // Best effort: the rename itself reaches the disk with its directory.
        #[cfg(unix)]
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
        Ok(())
    }

    /// Reports changes under the project root, debounced, except in the editor's own
    /// directory, version control, build outputs, installed packages, and metas.
    pub fn watch(&self) -> Result<Watch<FileChange>, ProjectError> {
        let (raw_sender, raw_receiver) = mpsc::channel();
        let mut debouncer = new_debouncer(WATCH_DEBOUNCE, None, raw_sender)
            .map_err(|error| ProjectError::Watch(error.to_string()))?;
        // The root alone, then each directory in it that may hold the user's files: a
        // recursive watch of the root would walk `target/` and `.git/`.
        debouncer
            .watch(&self.root, RecursiveMode::NonRecursive)
            .map_err(|error| ProjectError::Watch(error.to_string()))?;
        let mut watched = BTreeSet::new();
        for entry in fs::read_dir(&self.root).map_err(|error| io_error(&self.root, error))? {
            let path = entry.map_err(|error| io_error(&self.root, error))?.path();
            if path.is_dir() && relative_path(&self.root, &path).is_some() {
                watch_except_packages(&mut debouncer, &path, &mut watched)
                    .map_err(|error| ProjectError::Watch(error.to_string()))?;
            }
        }
        let (watch, sender, dropped) = Watch::new();
        let root = self.root.clone();
        let mut known = BTreeSet::new();
        list_files(&root, &root, &mut known);
        // The thread owns the debouncer, which stops watching when it is dropped.
        thread::spawn(move || {
            let mut debouncer = debouncer;
            while let Some(batch) = next_raw(&raw_receiver, &dropped) {
                let Ok(events) = batch else { continue };
                let mut touched = BTreeSet::new();
                for path in events.iter().flat_map(|event| &event.paths) {
                    touched.extend(relative_path(&root, path));
                }
                let mut changes = Vec::new();
                // Created in this batch: a file found in a new directory may have its own
                // event in the same batch, which is not a change.
                let mut created = BTreeSet::new();
                for relative in touched {
                    let file = root.join(&relative);
                    // Top-level directories are watched alone, and the directories in them
                    // recursively (see `watch_except_packages`).
                    let depth = relative.split('/').count();
                    if depth <= 2 && file.is_dir() && !watched.contains(&file) {
                        // A new directory: watch it, and report what it already holds.
                        let watching = if depth == 1 {
                            watch_except_packages(&mut debouncer, &file, &mut watched)
                        } else {
                            watched.insert(file.clone());
                            debouncer.watch(&file, RecursiveMode::Recursive)
                        };
                        if watching.is_ok() {
                            let mut inside = BTreeSet::new();
                            list_files(&root, &file, &mut inside);
                            for relative in inside {
                                if known.insert(relative.clone()) {
                                    created.insert(relative.clone());
                                    changes.push(FileChange::Created(relative));
                                }
                            }
                        }
                        continue;
                    }
                    // The event kinds are not reliable across platforms (an atomic save can
                    // look like a creation), so compare with the files known to exist.
                    let change = if file.is_dir() {
                        continue;
                    } else if !file.exists() {
                        unwatch_below(&mut debouncer, &file, &mut watched);
                        if !known.remove(&relative) {
                            continue;
                        }
                        FileChange::Removed(relative)
                    } else if known.insert(relative.clone()) {
                        FileChange::Created(relative)
                    } else if created.contains(&relative) {
                        continue;
                    } else {
                        FileChange::Changed(relative)
                    };
                    changes.push(change);
                }
                for change in changes {
                    if sender.send(change).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(watch)
    }
}

/// Watches `directory` and what is below it, except a `node_modules` directly in it: installed
/// packages hold thousands of directories (one inotify watch each, all walked by the debouncer)
/// that no watcher reports. `directory` is watched alone and each other directory in it
/// recursively, all added to `watched`; a directory created in it later is the caller's to watch.
pub(crate) fn watch_except_packages(
    debouncer: &mut Debouncer<RecommendedWatcher, RecommendedCache>,
    directory: &Path,
    watched: &mut BTreeSet<PathBuf>,
) -> notify::Result<()> {
    debouncer.watch(directory, RecursiveMode::NonRecursive)?;
    watched.insert(directory.to_path_buf());
    for entry in fs::read_dir(directory)
        .map_err(notify::Error::io)?
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() && entry.file_name() != "node_modules" && watched.insert(path.clone()) {
            debouncer.watch(&path, RecursiveMode::Recursive)?;
        }
    }
    Ok(())
}

/// Stops watching `directory` and every watched directory below it (it was removed).
pub(crate) fn unwatch_below(
    debouncer: &mut Debouncer<RecommendedWatcher, RecommendedCache>,
    directory: &Path,
    watched: &mut BTreeSet<PathBuf>,
) {
    for path in watched.extract_if(.., |path| path.starts_with(directory)) {
        let _ = debouncer.unwatch(&path);
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

/// `path` relative to `root` with forward slashes; none outside the root, in an unwatched
/// directory, in `node_modules/`, or for a meta.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<_> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    let first = parts.first()?;
    if UNWATCHED_DIRECTORIES.contains(&first.as_ref())
        || parts.iter().any(|part| part == "node_modules")
        || parts.last()?.ends_with(".meta")
    {
        return None;
    }
    Some(parts.join("/"))
}

/// Adds every file below `directory` to `out`, skipping what [`relative_path`] ignores.
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;

    #[test]
    fn installed_packages_are_not_watched() {
        let dir = tempfile::TempDir::new().unwrap();
        let editor = dir.path().join("editor");
        for directory in ["node_modules/react", "panels/deep"] {
            fs::create_dir_all(editor.join(directory)).unwrap();
        }
        fs::write(editor.join("index.ts"), "").unwrap();
        let mut debouncer = new_debouncer(WATCH_DEBOUNCE, None, |_| {}).unwrap();
        let mut watched = BTreeSet::new();
        watch_except_packages(&mut debouncer, &editor, &mut watched).unwrap();
        assert_eq!(
            watched,
            BTreeSet::from([editor.clone(), editor.join("panels")])
        );
        unwatch_below(&mut debouncer, &editor, &mut watched);
        assert!(watched.is_empty());
    }

    /// The watcher thread holds the dropped flag until it exits.
    #[test]
    #[allow(clippy::disallowed_methods)]
    fn the_watcher_stops_soon_after_its_watch_is_dropped() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("ludenea.toml"),
            "name = \"demo\"\nengine = \"0.1.0\"\n",
        )
        .unwrap();
        let project = Project::open(dir.path()).unwrap();
        let watch = project.watch().unwrap();
        let dropped = Arc::clone(&watch.dropped);
        assert_eq!(
            Arc::strong_count(&dropped),
            3,
            "the watch, the thread, and this test"
        );
        drop(watch);
        let start = Instant::now();
        while Arc::strong_count(&dropped) > 1 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the watcher still runs"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
