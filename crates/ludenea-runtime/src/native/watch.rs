//! Watches a project's textures while a game plays, so the window repacks the ones that
//! change. Only what is drawn changes: the simulation and its hash never see a texture.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use ludenea_app::meta::is_png;
use ludenea_app::project::{ProjectTextures, project_folders, project_textures};
use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::RuntimeError;

/// How long the files must stay unchanged before a change is reported: a save writes a file
/// in several steps, and an image editor may write the PNG and its meta one after the other.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Reports, debounced, that a `.png` or a `.png.meta` changed under the folders
/// `project_textures` scans, with the textures read again. A second watcher beside the
/// project's own (`ludenea-project`), which the runtime does not depend on.
pub struct TextureWatch {
    scene: PathBuf,
    project: Option<PathBuf>,
    events: Receiver<DebounceEventResult>,
    /// Watches while it lives.
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    /// A texture file changed since the textures were last read.
    changed: bool,
    /// What `poll` returned last: the same textures read again are not reported twice.
    last: Option<Result<ProjectTextures, String>>,
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
        Ok(TextureWatch {
            scene: scene.to_path_buf(),
            project: project.map(Path::to_path_buf),
            events,
            _debouncer: debouncer,
            changed: false,
            last: None,
        })
    }

    /// Without waiting: the textures read again (`project_textures`'s result) when a `.png`
    /// or a `.png.meta` changed since the last call and the files have settled for the
    /// debounce, unless they read as what was returned last; or the watcher's errors, the
    /// change then reported at the next call. `None` when nothing changed.
    pub fn poll(&mut self) -> Option<Result<ProjectTextures, String>> {
        let mut errors = Vec::new();
        while let Ok(batch) = self.events.try_recv() {
            match batch {
                Ok(events) => {
                    self.changed |= events.iter().any(|event| {
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
            return Some(Err(format!(
                "the texture watcher failed: {}",
                errors.join("; ")
            )));
        }
        if !std::mem::take(&mut self.changed) {
            return None;
        }
        let read = project_textures(&self.scene, self.project.as_deref());
        if self.last.as_ref() == Some(&read) {
            return None;
        }
        self.last = Some(read.clone());
        Some(read)
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
    use std::thread;
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
        let update = next(&mut watch).expect("the change is reported");
        let textures = update.unwrap().textures;
        let [(_, path, bytes, _)] = textures.as_slice() else {
            panic!("{textures:?}");
        };
        assert_eq!(path, &folder.join("hero.png").display().to_string());
        assert_eq!(bytes, b"png bytes");
        quiet(&mut watch, Duration::from_secs(1));
    }

    #[test]
    fn an_unrelated_file_is_ignored() {
        let (_guard, folder, mut watch) = watched();
        fs::write(folder.join("notes.txt"), "hello").unwrap();
        fs::write(folder.join("level.prefab.ron"), "Prefab(version: 1)").unwrap();
        quiet(&mut watch, Duration::from_secs(1));
    }
}
