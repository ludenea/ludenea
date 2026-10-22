//! Watches a project's textures while a game plays, so the window repacks the ones that
//! change. Only what is drawn changes: the simulation and its hash never see a texture.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use ludenea_app::meta::is_png;
use ludenea_app::project::{ProjectTextures, project_folders, project_textures};
use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::RuntimeError;

/// How long the files must stay unchanged before a change is reported: a save writes a file
/// in several steps, and an image editor may write the PNG and its meta one after the other.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// A reading of the textures, or why there is none.
type Reading = Result<ProjectTextures, String>;

/// Reports, debounced, that a `.png` or a `.png.meta` changed under the folders
/// `project_textures` scans, with the textures read again on the watcher's own thread, so
/// a frame never waits for the files. A second watcher beside the project's own
/// (`ludenea-project`), which the runtime does not depend on.
pub struct TextureWatch {
    readings: Receiver<Reading>,
    /// Watches while it lives; dropping it ends the reading thread.
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl TextureWatch {
    /// Watches the folders `project_textures(scene, project)` scans, recursively. A project's
    /// `scenes/` or `assets/` that does not exist yet is not watched.
    pub fn start(scene: &Path, project: Option<&Path>) -> Result<TextureWatch, RuntimeError> {
        let folders = project_folders(scene, project).map_err(RuntimeError::Watch)?;
        let (sender, events) = channel();
        let mut debouncer = new_debouncer(DEBOUNCE, None, sender).map_err(|error| {
            RuntimeError::Watch(format!("cannot start the file watcher: {error}"))
        })?;
        // ponytail: a folder created after the start is not watched; watch the project root
        // without its build outputs if creating `assets/` while playing matters.
        for folder in folders.iter().filter(|folder| folder.is_dir()) {
            debouncer
                .watch(folder, RecursiveMode::Recursive)
                .map_err(|error| {
                    RuntimeError::Watch(format!("cannot watch {}: {error}", folder.display()))
                })?;
        }
        let (readings_sender, readings) = channel();
        let (scene, project) = (scene.to_path_buf(), project.map(Path::to_path_buf));
        thread::Builder::new()
            .name("texture watch".to_string())
            .spawn(move || read_on_change(&events, &readings_sender, &scene, project.as_deref()))
            .map_err(|error| {
                RuntimeError::Watch(format!("cannot start the texture reading thread: {error}"))
            })?;
        Ok(TextureWatch {
            readings,
            _debouncer: debouncer,
        })
    }

    /// Without waiting: the next reading of the textures (`project_textures`'s result),
    /// made once a `.png` or a `.png.meta` changed and the files settled for the debounce,
    /// or the watcher's errors; `None` when there is none. Every change gives a reading, the
    /// same files written again too: the display, which knows what it applied, skips a
    /// reading it holds.
    pub fn poll(&mut self) -> Option<Reading> {
        self.readings.try_recv().ok()
    }
}

/// Until the debouncer or the watch is dropped: for each batch of debounced events (with
/// those already waiting), sends the watcher's errors, then a reading of the textures when
/// a texture file changed.
fn read_on_change(
    events: &Receiver<DebounceEventResult>,
    readings: &Sender<Reading>,
    scene: &Path,
    project: Option<&Path>,
) {
    while let Ok(batch) = events.recv() {
        let mut changed = false;
        let mut errors = Vec::new();
        for batch in std::iter::once(batch).chain(events.try_iter()) {
            match batch {
                Ok(events) => {
                    changed |= events.iter().any(|event| {
                        // Reading a file is an access event on some platforms: the reads
                        // below must not report a change.
                        !matches!(event.kind, EventKind::Access(_))
                            && event.paths.iter().any(|path| is_texture_file(path))
                    });
                }
                Err(failures) => errors.extend(failures.iter().map(ToString::to_string)),
            }
        }
        if !errors.is_empty() {
            let error = format!("the texture watcher failed: {}", errors.join("; "));
            if readings.send(Err(error)).is_err() {
                return;
            }
        }
        if changed && readings.send(project_textures(scene, project)).is_err() {
            return;
        }
    }
}

/// A PNG or a PNG's meta.
fn is_texture_file(path: &Path) -> bool {
    let name = path.to_string_lossy();
    is_png(name.strip_suffix(".meta").unwrap_or(&name))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::native::surface::now;

    /// How long a test waits for a change to be reported, far past the debounce.
    const PATIENCE: Duration = Duration::from_secs(10);

    /// Polls `watch` until it reports, or `PATIENCE` runs out.
    fn next(watch: &mut TextureWatch) -> Option<Result<ProjectTextures, String>> {
        let start = now();
        while start.elapsed() < PATIENCE {
            if let Some(update) = watch.poll() {
                return Some(update);
            }
            thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// Polls `watch` for `period`, failing on any report.
    fn quiet(watch: &mut TextureWatch, period: Duration) {
        let start = now();
        while start.elapsed() < period {
            if let Some(update) = watch.poll() {
                panic!("unexpected report: {update:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Takes the readings still on their way, until none comes for a second.
    fn quiet_after(watch: &mut TextureWatch) {
        let mut last = now();
        while last.elapsed() < Duration::from_secs(1) {
            if watch.poll().is_some() {
                last = now();
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// A scene in a folder of its own, watched, with the folder's canonical path (macOS
    /// reports `/private/var` for `/var`).
    fn watched() -> (tempfile::TempDir, PathBuf, TextureWatch) {
        let guard = tempfile::tempdir().unwrap();
        let folder = guard.path().canonicalize().unwrap();
        let scene = folder.join("main.scene.ron");
        fs::write(&scene, "").unwrap();
        // Events of the files written before the watch starts may still be on their way.
        thread::sleep(Duration::from_millis(200));
        let mut watch = TextureWatch::start(&scene, None).unwrap();
        quiet(&mut watch, Duration::from_millis(300));
        (guard, folder, watch)
    }

    #[test]
    fn a_written_png_is_reported_once_after_the_debounce() {
        let (_guard, folder, mut watch) = watched();
        fs::write(
            folder.join("hero.png.meta"),
            "Meta(version: 2, id: \"00000000000000000000000000000001\", kind: Texture((pixels_per_unit: 8, grid: None, pivot: None, filter: Linear)))",
        )
        .unwrap();
        fs::write(folder.join("hero.png"), b"png bytes").unwrap();
        // The two writes may settle in two batches, the first read before the PNG exists:
        // the readings are taken until one holds the texture.
        let start = now();
        let textures = loop {
            assert!(start.elapsed() < PATIENCE, "the texture is never reported");
            let update = next(&mut watch).expect("the change is reported");
            let textures = update.unwrap().textures;
            if !textures.is_empty() {
                break textures;
            }
        };
        let [(_, path, bytes, _)] = textures.as_slice() else {
            panic!("{textures:?}");
        };
        assert_eq!(path, &folder.join("hero.png").display().to_string());
        assert_eq!(bytes, b"png bytes");
        quiet(&mut watch, Duration::from_secs(1));
    }

    #[test]
    fn the_same_files_written_again_are_read_again() {
        let (_guard, folder, mut watch) = watched();
        fs::write(folder.join("hero.png"), b"png bytes").unwrap();
        fs::write(
            folder.join("hero.png.meta"),
            "Meta(version: 2, id: \"00000000000000000000000000000001\", kind: Texture((pixels_per_unit: 8, grid: None, pivot: None, filter: Linear)))",
        )
        .unwrap();
        let first = next(&mut watch).expect("the change is reported");
        quiet_after(&mut watch);
        // The display may have refused the first reading: the same files are read again.
        fs::write(folder.join("hero.png"), b"png bytes").unwrap();
        let again = next(&mut watch).expect("the rewrite is reported");
        assert_eq!(again, first);
    }

    #[test]
    fn the_textures_are_read_once_settled_not_when_polled() {
        let (_guard, folder, mut watch) = watched();
        let (png, meta) = (folder.join("hero.png"), folder.join("hero.png.meta"));
        fs::write(&png, b"png bytes").unwrap();
        fs::write(
            &meta,
            "Meta(version: 2, id: \"00000000000000000000000000000001\", kind: Texture((pixels_per_unit: 8, grid: None, pivot: None, filter: Linear)))",
        )
        .unwrap();
        // Not polled meanwhile: a frame never waits for the files to be read.
        thread::sleep(Duration::from_secs(2));
        fs::remove_file(&png).unwrap();
        fs::remove_file(&meta).unwrap();
        let first = next(&mut watch).expect("the change is reported");
        assert_eq!(first.unwrap().textures.len(), 1, "read before the removal");
    }

    #[test]
    fn an_unrelated_file_is_ignored() {
        let (_guard, folder, mut watch) = watched();
        fs::write(folder.join("notes.txt"), "hello").unwrap();
        fs::write(folder.join("level.prefab.ron"), "Prefab(version: 1)").unwrap();
        quiet(&mut watch, Duration::from_secs(1));
    }
}
