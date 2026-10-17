use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::{fs, io, thread};

use cargo_metadata::MetadataCommand;
use notify::{Event, EventKind, RecursiveMode};
use notify_debouncer_full::new_debouncer;
use rolldown::{
    Bundler, BundlerOptions, BundlerTransformOptions, CodeSplittingMode, Either, JsxOptions,
    OutputFormat, Platform, TsConfig,
};
use rolldown_common::Output;
use rolldown_error::{BuildDiagnostic, DiagnosticOptions, Severity};
use rolldown_plugin::{HookResolveIdArgs, HookResolveIdReturn, HookUsage, Plugin, PluginContext};
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use crate::files::WATCH_DEBOUNCE;
use crate::{Diagnostic, Project};

/// Provided by the editor at runtime (import map), so every plugin shares one instance.
const SHARED_MODULES: [&str; 4] = [
    "react",
    "react/jsx-runtime",
    "@ludenea/editor-shell",
    "@ludenea/control-client",
];
const ENTRY_FILES: [&str; 2] = ["index.ts", "index.tsx"];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PluginBundle {
    pub name: String,
    /// `plugins/<name>.<hash>.js`, relative to `.ludenea/`.
    pub url_path: String,
    pub source: PluginSource,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginSource {
    Project,
    Engine { crate_name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEvent {
    Rebuilt(PluginBundle),
    Failed {
        name: String,
        diagnostics: Vec<Diagnostic>,
    },
}

/// A crate's `editor/` directory holding `index.ts` or `index.tsx`.
struct Entry {
    name: String,
    source: PluginSource,
    directory: PathBuf,
}

impl Project {
    /// Bundles the plugin of the project and those of its dependencies into `.ludenea/plugins/`.
    /// Every plugin is attempted; the diagnostics of all failing ones are returned together.
    /// An entry is `editor/index.ts` or `editor/index.tsx`; `index.ts` wins when both exist.
    pub fn bundle_plugins(&self) -> Result<Vec<PluginBundle>, Vec<Diagnostic>> {
        let entries = self
            .plugin_entries()
            .map_err(|diagnostic| vec![diagnostic])?;
        let mut bundles = Vec::new();
        let mut diagnostics = Vec::new();
        for entry in &entries {
            match bundle(&self.root, entry) {
                Ok(bundle) => bundles.push(bundle),
                Err(mut failed) => diagnostics.append(&mut failed),
            }
        }
        if diagnostics.is_empty() {
            Ok(bundles)
        } else {
            Err(diagnostics)
        }
    }

    /// Rebundles a plugin whenever a file of its `editor/` directory changes. Watching stops when
    /// the receiver is dropped (noticed at the next change). If watching cannot start, the only
    /// event is a failure of the project's plugin.
    pub fn watch_plugins(&self) -> Receiver<PluginEvent> {
        let (sender, receiver) = mpsc::channel();
        let (raw_sender, raw_receiver) = mpsc::channel();
        // ponytail: the plugin list is read once; a dependency added later needs a new watch.
        let watched = self.plugin_entries().and_then(|entries| {
            let mut debouncer = new_debouncer(WATCH_DEBOUNCE, None, raw_sender)
                .map_err(|error| error_diagnostic(format!("cannot watch plugins: {error}")))?;
            for entry in &entries {
                debouncer
                    .watch(&entry.directory, RecursiveMode::Recursive)
                    .map_err(|error| error_diagnostic(format!("cannot watch plugins: {error}")))?;
            }
            Ok((entries, debouncer))
        });
        let (entries, debouncer) = match watched {
            Ok(watched) => watched,
            Err(diagnostic) => {
                let _ = sender.send(PluginEvent::Failed {
                    name: self.manifest.name.clone(),
                    diagnostics: vec![diagnostic],
                });
                return receiver;
            }
        };
        let root = self.root.clone();
        // The thread owns the debouncer, which stops watching when it is dropped.
        thread::spawn(move || {
            let _debouncer = debouncer;
            // The last bundle announced per plugin, so an unchanged rebuild stays silent.
            let mut announced: BTreeMap<String, String> = BTreeMap::new();
            for batch in raw_receiver {
                let Ok(events) = batch else { continue };
                for entry in changed_entries(&entries, events.iter().map(|event| &event.event)) {
                    let event = match bundle(&root, entry) {
                        Ok(bundle) => {
                            if announced.get(&bundle.name) == Some(&bundle.url_path) {
                                continue;
                            }
                            announced.insert(bundle.name.clone(), bundle.url_path.clone());
                            PluginEvent::Rebuilt(bundle)
                        }
                        Err(diagnostics) => {
                            // After a failure, the next good bundle is news even if unchanged.
                            announced.remove(&entry.name);
                            PluginEvent::Failed {
                                name: entry.name.clone(),
                                diagnostics,
                            }
                        }
                    };
                    if sender.send(event).is_err() {
                        return;
                    }
                }
            }
        });
        receiver
    }

    /// The project's plugin, then those of its dependencies in `cargo metadata` order.
    fn plugin_entries(&self) -> Result<Vec<Entry>, Diagnostic> {
        let metadata = MetadataCommand::new()
            .manifest_path(self.root.join("Cargo.toml"))
            .exec()
            .map_err(|error| {
                error_diagnostic(format!("cannot list the project's dependencies: {error}"))
            })?;
        let project = (
            self.manifest.name.clone(),
            PluginSource::Project,
            self.root.clone(),
        );
        let dependencies = metadata.packages.into_iter().filter_map(|package| {
            let directory = package
                .manifest_path
                .parent()?
                .as_std_path()
                .canonicalize()
                .ok()?;
            let name = package.name.to_string();
            (directory != self.root).then(|| {
                let source = PluginSource::Engine {
                    crate_name: name.clone(),
                };
                (name, source, directory)
            })
        });
        Ok(std::iter::once(project)
            .chain(dependencies)
            .filter_map(|(name, source, crate_directory)| {
                let directory = crate_directory.join("editor");
                entry_file(&directory)?;
                Some(Entry {
                    name,
                    source,
                    directory,
                })
            })
            .collect())
    }
}

/// The plugins whose `editor/` directory saw a change. Accesses are not changes: bundling opens
/// the sources, and on Linux those openings are events too.
fn changed_entries<'a>(
    entries: &'a [Entry],
    events: impl IntoIterator<Item = &'a Event>,
) -> impl Iterator<Item = &'a Entry> {
    let paths: Vec<&PathBuf> = events
        .into_iter()
        .filter(|event| !matches!(event.kind, EventKind::Access(_)))
        .flat_map(|event| &event.paths)
        .collect();
    entries
        .iter()
        .filter(move |entry| paths.iter().any(|path| path.starts_with(&entry.directory)))
}

/// `index.ts` wins when both entries exist.
fn entry_file(directory: &Path) -> Option<PathBuf> {
    ENTRY_FILES
        .iter()
        .map(|file| directory.join(file))
        .find(|path| path.is_file())
}

/// Bundles `entry` into `.ludenea/plugins/<name>.<hash>.js`, removing its older bundles.
fn bundle(root: &Path, entry: &Entry) -> Result<PluginBundle, Vec<Diagnostic>> {
    let input = entry_file(&entry.directory).ok_or_else(|| {
        vec![error_diagnostic(format!(
            "the plugin {} has no editor/index.ts or editor/index.tsx",
            entry.name
        ))]
    })?;
    let paths = DiagnosticOptions {
        cwd: root.to_path_buf(),
    };
    let rejected = Arc::new(Mutex::new(Vec::new()));
    let options = BundlerOptions {
        input: Some(vec![input.to_string_lossy().into_owned().into()]),
        cwd: Some(entry.directory.clone()),
        format: Some(OutputFormat::Esm),
        platform: Some(Platform::Browser),
        external: Some(SHARED_MODULES.map(String::from).to_vec().into()),
        code_splitting: Some(CodeSplittingMode::Bool(false)),
        // A tsconfig is for the editor's type checking; it must not change the output.
        tsconfig: Some(TsConfig::Auto(false)),
        transform: Some(BundlerTransformOptions {
            jsx: Some(Either::Right(JsxOptions {
                runtime: Some("automatic".to_string()),
                development: Some(false),
                ..Default::default()
            })),
            ..Default::default()
        }),
        ..Default::default()
    };
    let only_own_files = OnlyOwnFiles {
        root: root.to_path_buf(),
        rejected: Arc::clone(&rejected),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|error| {
            vec![error_diagnostic(format!(
                "cannot start the bundler: {error}"
            ))]
        })?;
    let output = runtime.block_on(async {
        let mut bundler =
            Bundler::with_plugins(options, vec![OnlyOwnFiles::new_shared(only_own_files)])?;
        let output = bundler.generate().await;
        let _ = bundler.close().await;
        output
    });
    let output = output.map_err(|errors| {
        let rejected = std::mem::take(&mut *rejected.lock().expect("not poisoned"));
        if rejected.is_empty() {
            errors
                .into_vec()
                .iter()
                .map(|error| rolldown_diagnostic(error, &paths))
                .collect()
        } else {
            // rolldown wraps each rejection in a less precise error of its own.
            rejected
        }
    })?;
    let code = output
        .assets
        .iter()
        .find_map(|output| match output {
            Output::Chunk(chunk) => Some(chunk.code.clone()),
            Output::Asset(_) => None,
        })
        .unwrap_or_default();
    write_bundle(root, &entry.name, &code).map_err(|error| {
        vec![error_diagnostic(format!(
            "cannot write the bundle of {}: {error}",
            entry.name
        ))]
    })?;
    Ok(PluginBundle {
        name: entry.name.clone(),
        url_path: format!("plugins/{}", bundle_file_name(&entry.name, &code)),
        source: entry.source.clone(),
    })
}

fn bundle_file_name(name: &str, code: &str) -> String {
    format!("{name}.{:016x}.js", xxh3_64(code.as_bytes()))
}

fn write_bundle(root: &Path, name: &str, code: &str) -> io::Result<()> {
    // ponytail: one lock for every plugin of the process, so two bundles of the same plugin never
    // delete each other's file; a lock per plugin name if writing ever becomes a bottleneck.
    static WRITING: Mutex<()> = Mutex::new(());
    let _writing = WRITING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = root.join(".ludenea/plugins");
    fs::create_dir_all(&directory)?;
    let file_name = bundle_file_name(name, code);
    fs::write(directory.join(&file_name), code)?;
    for old in fs::read_dir(&directory)?.flatten() {
        let old_name = old.file_name().to_string_lossy().into_owned();
        let hash = old_name
            .strip_prefix(&format!("{name}."))
            .and_then(|rest| rest.strip_suffix(".js"));
        if hash.is_some_and(|hash| !hash.contains('.')) && old_name != file_name {
            // Best effort: a stale bundle left behind is harmless, a failed build is not.
            let _ = fs::remove_file(old.path());
        }
    }
    Ok(())
}

fn rolldown_diagnostic(error: &BuildDiagnostic, paths: &DiagnosticOptions) -> Diagnostic {
    let location = error.to_diagnostic_with(paths).get_primary_location();
    let level = match error.severity() {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    };
    Diagnostic {
        level: level.to_string(),
        message: error.to_message_with(paths),
        file: location.as_ref().map(|(file, ..)| file.clone()),
        line: location
            .as_ref()
            .and_then(|(_, line, ..)| u32::try_from(*line).ok()),
        // rolldown counts columns from 0, cargo and editors from 1.
        column: location.and_then(|(_, _, column, _)| u32::try_from(column + 1).ok()),
    }
}

/// Relative to `root` the way rolldown writes the files of its diagnostics.
fn relative_path(root: &Path, path: &str) -> String {
    DiagnosticOptions {
        cwd: root.to_path_buf(),
    }
    .stabilize_path(path)
}

fn error_diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        level: "error".to_string(),
        message,
        file: None,
        line: None,
        column: None,
    }
}

/// Rejects bare imports other than the shared modules, which rolldown marks external before
/// asking plugins.
#[derive(Debug)]
struct OnlyOwnFiles {
    root: PathBuf,
    rejected: Arc<Mutex<Vec<Diagnostic>>>,
}

impl Plugin for OnlyOwnFiles {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("ludenea:only-own-files")
    }

    async fn resolve_id(
        &self,
        _context: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        let specifier = args.specifier;
        if specifier.starts_with('.') || Path::new(specifier).is_absolute() {
            return Ok(None);
        }
        let message = format!(
            "cannot import {specifier:?}: editor plugins may import only React, the editor \
             packages, and their own files (react, react/jsx-runtime, @ludenea/editor-shell, \
             @ludenea/control-client, and relative paths)"
        );
        self.rejected
            .lock()
            .expect("not poisoned")
            .push(Diagnostic {
                level: "error".to_string(),
                message: message.clone(),
                file: args
                    .importer
                    .map(|importer| relative_path(&self.root, importer)),
                line: None,
                column: None,
            });
        Err(io::Error::other(message).into())
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId
    }
}

#[cfg(test)]
mod tests {
    use notify::event::{AccessKind, AccessMode, ModifyKind};

    use super::*;

    #[test]
    fn opening_plugin_sources_is_not_a_change() {
        let entries = [Entry {
            name: "demo".to_string(),
            source: PluginSource::Project,
            directory: PathBuf::from("/project/editor"),
        }];
        let source = PathBuf::from("/project/editor/index.ts");
        let opened = Event::new(EventKind::Access(AccessKind::Open(AccessMode::Read)))
            .add_path(source.clone());
        assert_eq!(changed_entries(&entries, [&opened]).count(), 0);
        let modified = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(source);
        let changed: Vec<_> = changed_entries(&entries, [&opened, &modified])
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(changed, ["demo"]);
    }
}
