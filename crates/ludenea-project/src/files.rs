use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ludenea_ecs::AssetId;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{Debouncer, RecommendedCache, new_debouncer};
use serde::{Deserialize, Serialize};

use crate::meta::{AssetKind, FoundAsset, Meta, read_assets};
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

/// A change on disk, with the path relative to the project root and the file's kind (a
/// removed file's kind as it was last seen).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub enum FileChange {
    Changed { path: String, kind: AssetKind },
    Created { path: String, kind: AssetKind },
    Removed { path: String, kind: AssetKind },
}

fn meta_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".meta");
    name.into()
}

/// The kind of the file `path`: its extension decides, except that only a meta makes a file
/// a data asset.
fn kind_of(path: &str, meta: Option<&Meta>) -> AssetKind {
    match meta.map(|meta| &meta.kind) {
        Some(data @ AssetKind::Data { .. }) => data.clone(),
        _ => AssetKind::of(path),
    }
}

/// The kind of the project file `relative`, reading its meta when it has one.
fn kind_on_disk(root: &Path, relative: &str) -> AssetKind {
    let meta = meta_path(&root.join(relative));
    let meta = meta.is_file().then(|| Meta::read(&meta).ok()).flatten();
    kind_of(relative, meta.as_ref())
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
            let (id, kind) = match Meta::load_or_create(&meta_file, kind.clone()) {
                Ok(meta) => {
                    valid.push(format!("{path}.meta"));
                    (meta.id, kind_of(path, Some(&meta)))
                }
                Err(reason) => {
                    invalid.push((format!("{path}.meta"), reason));
                    (AssetId(0), kind)
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

    /// Like [`Project::read_text`], for any bytes.
    pub fn read_binary(&self, path: &str) -> Result<Vec<u8>, ProjectError> {
        let file = self.resolve_target(path)?;
        let mut bytes = Vec::new();
        open_unfollowed(&file)
            .and_then(|mut opened| opened.read_to_end(&mut bytes))
            .map_err(|error| io_error(&file, error))?;
        Ok(bytes)
    }

    /// Writes `text` to the new file `path` and a meta of `kind` next to it, both or neither;
    /// either existing is refused, and so is a kind the path's extension contradicts. The
    /// file comes first: a crash in between leaves a file without a meta, which the next
    /// listing gives one, never a meta without a file.
    pub fn create_asset(
        &mut self,
        path: &str,
        kind: AssetKind,
        text: &str,
    ) -> Result<FileEntry, ProjectError> {
        let fits = match &kind {
            AssetKind::Data { .. } => {
                path.ends_with(".ron") && AssetKind::of(path) == AssetKind::Other
            }
            other => AssetKind::of(path) == *other,
        };
        if !fits {
            return Err(ProjectError::KindMismatch {
                path: path.to_string(),
                kind,
            });
        }
        let file = self.resolve(path)?;
        let meta_file = meta_path(&file);
        if meta_file.symlink_metadata().is_ok() {
            return Err(ProjectError::AlreadyExists(meta_file));
        }
        let meta = Meta::new(kind);
        let meta_text = ron::to_string(&meta).expect("a meta always encodes") + "\n";
        self.place(&file, text.as_bytes(), false)?;
        if let Err(error) = self.place(&meta_file, meta_text.as_bytes(), false) {
            // Only the file is this call's to remove.
            let _ = fs::remove_file(&file);
            return Err(error);
        }
        Ok(FileEntry {
            path: path.to_string(),
            id: meta.id,
            kind: kind_of(path, Some(&meta)),
        })
    }

    /// Every prefab and data asset under `scenes/` and `assets/`, read like the CLI reads a
    /// project (see [`read_assets`]).
    pub fn asset_texts(&self) -> Result<Vec<FoundAsset>, ProjectError> {
        let mut files = Vec::new();
        for directory in ASSET_DIRECTORIES {
            collect(&self.root, directory, &mut files);
        }
        let files = files
            .into_iter()
            .map(|path| {
                let file = self.root.join(&path);
                (path, file)
            })
            .collect();
        read_assets(files).map_err(ProjectError::Assets)
    }

    /// Keeps the exported session of `document` in `.ludenea/sessions/`.
    pub fn store_session(&mut self, document: AssetId, bytes: &[u8]) -> Result<(), ProjectError> {
        self.write_bytes(&session_path(document), bytes)
    }

    /// The session [`Project::store_session`] kept for `document`, if any.
    pub fn load_stored_session(&self, document: AssetId) -> Result<Option<Vec<u8>>, ProjectError> {
        match self.read_binary(&session_path(document)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(ProjectError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Writes `text` to a temporary file in `.ludenea/` (unwatched, on the same file system),
    /// synced to disk, then renames it over `path`: neither a crash mid-write nor a power loss
    /// leaves a truncated file. The file keeps its permissions; through a symbolic link inside
    /// the project, the link's target is replaced and the link kept.
    pub fn write_text(&mut self, path: &str, text: &str) -> Result<(), ProjectError> {
        self.write_bytes(path, text.as_bytes())
    }

    /// Like [`Project::write_text`], for any bytes.
    pub fn write_bytes(&mut self, path: &str, bytes: &[u8]) -> Result<(), ProjectError> {
        let file = self.resolve_target(path)?;
        self.place(&file, bytes, true)
    }

    /// Writes `bytes` to a synced temporary file in `.ludenea/` (unwatched, on the same file
    /// system), then puts it at `file` in one step: renamed over it when `replace`, else
    /// linked there, which fails with `AlreadyExists` if `file` exists.
    fn place(&self, file: &Path, bytes: &[u8], replace: bool) -> Result<(), ProjectError> {
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let parent = file.parent().expect("a project file has a parent");
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        let staging = self.root.join(".ludenea");
        fs::create_dir_all(&staging).map_err(|error| io_error(&staging, error))?;
        let write = WRITES.fetch_add(1, Ordering::Relaxed);
        let temporary = staging.join(format!("write-{}-{write}.tmp", std::process::id()));
        let staged = File::create(&temporary).and_then(|mut staged| {
            staged.write_all(bytes)?;
            if let Ok(existing) = fs::metadata(file) {
                staged.set_permissions(existing.permissions())?;
            }
            staged.sync_all()
        });
        let placed = staged
            .map_err(|error| io_error(&temporary, error))
            .and_then(|()| {
                let placed = if replace {
                    fs::rename(&temporary, file)
                } else {
                    fs::hard_link(&temporary, file)
                };
                placed.map_err(|error| match error.kind() {
                    io::ErrorKind::AlreadyExists => ProjectError::AlreadyExists(file.to_path_buf()),
                    _ => io_error(file, error),
                })
            });
        if placed.is_err() || !replace {
            let _ = fs::remove_file(&temporary);
        }
        placed?;
        // Best effort: the new name itself reaches the disk with its directory.
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
        let mut known = BTreeMap::new();
        list_files(&self.root, &self.root, &mut known);
        let mut tracked = Tracked {
            root: self.root.clone(),
            known,
            reported: BTreeMap::new(),
            watched,
            debouncer,
        };
        // The thread owns the debouncer, which stops watching when it is dropped.
        thread::spawn(move || {
            while let Some(batch) = next_raw(&raw_receiver, &dropped) {
                let Ok(events) = batch else { continue };
                for change in tracked.changes(events.iter().map(|event| &event.event)) {
                    if sender.send(change).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(watch)
    }
}

/// What the file watcher knows: the files that exist with their kinds, and the directories
/// it watches.
struct Tracked {
    root: PathBuf,
    known: BTreeMap<String, AssetKind>,
    /// The kind and stamp of each file when its creation or change was last reported: one
    /// write whose events arrive in two batches is reported once.
    /// ponytail: two writes of the same length within one modification time tick look like
    /// one; fine with the nanosecond times of current file systems.
    reported: BTreeMap<String, (AssetKind, Stamp)>,
    watched: BTreeSet<PathBuf>,
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl Tracked {
    /// The changes a batch of events makes, watching the directories it creates.
    fn changes<'a>(&mut self, events: impl IntoIterator<Item = &'a Event>) -> Vec<FileChange> {
        let mut touched = BTreeSet::new();
        // Directories created in this batch, maybe after being removed in it too: their old
        // watch is gone even though they are still listed as watched.
        let mut recreated = BTreeSet::new();
        for event in events {
            for path in &event.paths {
                touched.extend(relative_path(&self.root, path));
                if matches!(event.kind, EventKind::Create(_)) {
                    recreated.insert(path.clone());
                }
            }
        }
        let mut changes = Vec::new();
        // Created in this batch: a file found in a new directory may have its own event in
        // the same batch, which is not a change.
        let mut created = BTreeSet::new();
        for relative in touched {
            let file = self.root.join(&relative);
            // Top-level directories are watched alone, and the directories in them
            // recursively (see `watch_except_packages`).
            let depth = relative.split('/').count();
            if depth <= 2
                && file.is_dir()
                && (!self.watched.contains(&file) || recreated.contains(&file))
            {
                // A new directory: watch it, and report what it already holds.
                unwatch_below(&mut self.debouncer, &file, &mut self.watched);
                let watching = if depth == 1 {
                    watch_except_packages(&mut self.debouncer, &file, &mut self.watched)
                } else {
                    let watching = self.debouncer.watch(&file, RecursiveMode::Recursive);
                    if watching.is_ok() {
                        self.watched.insert(file.clone());
                    }
                    watching
                };
                if watching.is_ok() {
                    let mut inside = BTreeMap::new();
                    list_files(&self.root, &file, &mut inside);
                    for (path, kind) in inside {
                        if self.known.insert(path.clone(), kind.clone()).is_none() {
                            created.insert(path.clone());
                            changes.push(FileChange::Created { path, kind });
                        }
                    }
                }
                continue;
            }
            // The event kinds are not reliable across platforms (an atomic save can look like
            // a creation), so compare with the files known to exist.
            let change = if file.is_dir() {
                continue;
            } else if !file.exists() {
                unwatch_below(&mut self.debouncer, &file, &mut self.watched);
                self.reported.remove(&relative);
                let Some(kind) = self.known.remove(&relative) else {
                    continue;
                };
                FileChange::Removed {
                    path: relative,
                    kind,
                }
            } else {
                let kind = kind_on_disk(&self.root, &relative);
                let seen = (kind.clone(), stamp(&file));
                if self.reported.insert(relative.clone(), seen.clone()) == Some(seen) {
                    continue;
                }
                if self.known.insert(relative.clone(), kind.clone()).is_none() {
                    FileChange::Created {
                        path: relative,
                        kind,
                    }
                } else if created.contains(&relative) {
                    continue;
                } else {
                    FileChange::Changed {
                        path: relative,
                        kind,
                    }
                }
            };
            changes.push(change);
        }
        changes
    }
}

/// A file's modification time and length, `None` when unreadable.
type Stamp = Option<(std::time::SystemTime, u64)>;

fn stamp(file: &Path) -> Stamp {
    let metadata = fs::metadata(file).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
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
        if path.is_dir() && entry.file_name() != "node_modules" && !watched.contains(&path) {
            debouncer.watch(&path, RecursiveMode::Recursive)?;
            watched.insert(path);
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

fn session_path(document: AssetId) -> String {
    format!(".ludenea/sessions/{document}.session")
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

/// Adds every file below `directory` to `out` with its kind, skipping what [`relative_path`]
/// ignores.
fn list_files(root: &Path, directory: &Path, out: &mut BTreeMap<String, AssetKind>) {
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
                let kind = kind_on_disk(root, &relative);
                out.insert(relative, kind);
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

    fn tracked(root: &Path) -> Tracked {
        Tracked {
            root: root.to_path_buf(),
            known: BTreeMap::new(),
            reported: BTreeMap::new(),
            watched: BTreeSet::new(),
            debouncer: new_debouncer(WATCH_DEBOUNCE, None, |_| {}).unwrap(),
        }
    }

    fn created(path: PathBuf) -> Event {
        Event::new(EventKind::Create(notify::event::CreateKind::Any)).add_path(path)
    }

    /// A new directory and a file written in it, in one batch: one creation, not a creation
    /// followed by a change.
    #[test]
    fn a_new_directory_with_a_file_in_one_batch_reports_the_file_once() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir(dir.path().join("levels")).unwrap();
        fs::write(dir.path().join("levels/a.txt"), "a").unwrap();
        let mut tracked = tracked(dir.path());
        let batch = [
            created(dir.path().join("levels")),
            created(dir.path().join("levels/a.txt")),
        ];
        assert_eq!(
            tracked.changes(&batch),
            [FileChange::Created {
                path: "levels/a.txt".into(),
                kind: AssetKind::Other
            }]
        );
        assert!(tracked.watched.contains(&dir.path().join("levels")));
    }

    /// A watched directory removed and created again in one batch is watched again: its old
    /// watch went with the removed directory.
    #[test]
    fn a_directory_recreated_in_one_batch_is_watched_again() {
        let dir = tempfile::TempDir::new().unwrap();
        let levels = dir.path().join("levels");
        fs::create_dir(&levels).unwrap();
        let mut tracked = tracked(dir.path());
        watch_except_packages(&mut tracked.debouncer, &levels, &mut tracked.watched).unwrap();
        fs::remove_dir(&levels).unwrap();
        fs::create_dir_all(levels.join("one")).unwrap();
        fs::write(levels.join("one/a.txt"), "a").unwrap();
        let removed = Event::new(EventKind::Remove(notify::event::RemoveKind::Folder))
            .add_path(levels.clone());
        assert_eq!(
            tracked.changes(&[removed, created(levels.clone())]),
            [FileChange::Created {
                path: "levels/one/a.txt".into(),
                kind: AssetKind::Other
            }]
        );
        assert!(tracked.watched.contains(&levels.join("one")));
    }

    /// One write whose events arrive in two batches is one change.
    #[test]
    fn one_write_split_across_batches_is_reported_once() {
        let dir = tempfile::TempDir::new().unwrap();
        let scene = dir.path().join("main.scene.ron");
        fs::write(&scene, "before").unwrap();
        let mut tracked = tracked(dir.path());
        tracked
            .known
            .insert("main.scene.ron".into(), AssetKind::Scene);
        fs::write(&scene, "after").unwrap();
        let modified = || {
            Event::new(EventKind::Modify(notify::event::ModifyKind::Any)).add_path(scene.clone())
        };
        let changed = FileChange::Changed {
            path: "main.scene.ron".into(),
            kind: AssetKind::Scene,
        };
        assert_eq!(
            tracked.changes(&[modified()]),
            std::slice::from_ref(&changed)
        );
        assert_eq!(tracked.changes(&[modified()]), []);
        fs::write(&scene, "after again").unwrap();
        assert_eq!(tracked.changes(&[modified()]), [changed]);
    }

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
