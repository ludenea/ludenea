use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::{fs, io, thread};

use cargo_metadata::{Metadata, MetadataCommand};
use notify::event::{AccessKind, AccessMode};
use notify::{Event, EventKind, RecursiveMode};
use notify_debouncer_full::new_debouncer;
use rolldown::{
    Bundler, BundlerOptions, BundlerTransformOptions, CodeSplittingMode, Either, JsxOptions,
    OutputFormat, Platform, TsConfig,
};
use rolldown_common::{Output, ResolvedExternal};
use rolldown_error::{BuildDiagnostic, DiagnosticOptions, Severity};
use rolldown_plugin::{
    HookResolveIdArgs, HookResolveIdOutput, HookResolveIdReturn, HookUsage, Plugin, PluginContext,
};
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use crate::files::WATCH_DEBOUNCE;
use crate::{Diagnostic, Project, Watch, next_raw};

/// Provided by the editor at runtime (import map), so every plugin shares one instance.
const SHARED_MODULES: [&str; 4] = [
    "react",
    "react/jsx-runtime",
    "@ludenea/editor-shell",
    "@ludenea/control-client",
];
const ENTRY_FILES: [&str; 2] = ["index.ts", "index.tsx"];
/// What a plugin may import besides the shared modules: no styles, images, or other assets,
/// which the editor would never load.
const SCRIPT_EXTENSIONS: [&str; 9] = ["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "json"];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct PluginBundle {
    pub name: String,
    /// `plugins/<name>.<hash>.js`, relative to `.ludenea/`.
    pub url_path: String,
    pub source: PluginSource,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
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
    /// Worth telling the user about a plugin that bundled (rolldown's warnings) or about a
    /// crate whose plugins cannot be looked for.
    Warnings {
        name: String,
        diagnostics: Vec<Diagnostic>,
    },
}

/// A crate's `editor/` directory, holding `index.ts` or `index.tsx` (the project's may not
/// exist yet).
struct Entry {
    name: String,
    source: PluginSource,
    directory: PathBuf,
}

impl Project {
    /// Bundles the plugin of the project and those of its dependencies into `.ludenea/plugins/`:
    /// one `Rebuilt` or `Failed` per plugin, so one failing plugin never keeps the others from
    /// loading. If the plugins cannot be listed, the only event is a failure of the project's.
    /// An entry is `editor/index.ts` or `editor/index.tsx`; `index.ts` wins when both exist.
    pub fn bundle_plugins(&self) -> Vec<PluginEvent> {
        let (entries, mut events) = match self.plugin_entries() {
            Ok(listed) => listed,
            Err(diagnostic) => {
                return vec![PluginEvent::Failed {
                    name: self.manifest.name.clone(),
                    diagnostics: vec![diagnostic],
                }];
            }
        };
        for entry in entries
            .iter()
            .filter(|entry| entry_file(&entry.directory).is_some())
        {
            match bundle(&self.root, entry) {
                Ok((bundle, warnings)) => {
                    events.push(PluginEvent::Rebuilt(bundle));
                    events.extend(warnings_of(&entry.name, warnings));
                }
                Err(diagnostics) => events.push(PluginEvent::Failed {
                    name: entry.name.clone(),
                    diagnostics,
                }),
            }
        }
        events
    }

    /// Rebundles a plugin whenever a file of its `editor/` directory changes, until the watch
    /// is dropped. If watching cannot start, the only event is a failure of the project's
    /// plugin.
    pub fn watch_plugins(&self) -> Watch<PluginEvent> {
        let (watch, sender, dropped) = Watch::new();
        let (raw_sender, raw_receiver) = mpsc::channel();
        // ponytail: the dependencies are listed once; a dependency added later needs a new
        // watch. The project's own `editor/` is watched for even before it exists.
        let watched = self.plugin_entries().and_then(|(entries, _)| {
            let cannot = |error| error_diagnostic(format!("cannot watch plugins: {error}"));
            let mut debouncer = new_debouncer(WATCH_DEBOUNCE, None, raw_sender).map_err(cannot)?;
            let mut recursive = BTreeSet::new();
            for entry in &entries {
                if entry.directory.is_dir() {
                    debouncer
                        .watch(&entry.directory, RecursiveMode::Recursive)
                        .map_err(cannot)?;
                    recursive.insert(entry.directory.clone());
                } else if let Some(parent) = entry.directory.parent() {
                    debouncer
                        .watch(parent, RecursiveMode::NonRecursive)
                        .map_err(cannot)?;
                }
            }
            Ok((entries, debouncer, recursive))
        });
        let (entries, mut debouncer, mut recursive) = match watched {
            Ok(watched) => watched,
            Err(diagnostic) => {
                let _ = sender.send(PluginEvent::Failed {
                    name: self.manifest.name.clone(),
                    diagnostics: vec![diagnostic],
                });
                return watch;
            }
        };
        let root = self.root.clone();
        // The thread owns the debouncer, which stops watching when it is dropped.
        thread::spawn(move || {
            // The last bundle announced per plugin, so an unchanged rebuild stays silent.
            let mut announced: BTreeMap<String, String> = BTreeMap::new();
            while let Some(batch) = next_raw(&raw_receiver, &dropped) {
                let Ok(events) = batch else { continue };
                for entry in changed_entries(&entries, events.iter().map(|event| &event.event)) {
                    // An `editor/` created after watching started.
                    if entry.directory.is_dir()
                        && !recursive.contains(&entry.directory)
                        && debouncer
                            .watch(&entry.directory, RecursiveMode::Recursive)
                            .is_ok()
                    {
                        recursive.insert(entry.directory.clone());
                    }
                    if entry_file(&entry.directory).is_none() {
                        continue;
                    }
                    let events = match bundle(&root, entry) {
                        Ok((bundle, warnings)) => {
                            if announced.get(&bundle.name) == Some(&bundle.url_path) {
                                continue;
                            }
                            announced.insert(bundle.name.clone(), bundle.url_path.clone());
                            let mut events = vec![PluginEvent::Rebuilt(bundle)];
                            events.extend(warnings_of(&entry.name, warnings));
                            events
                        }
                        Err(diagnostics) => {
                            // After a failure, the next good bundle is news even if unchanged.
                            announced.remove(&entry.name);
                            vec![PluginEvent::Failed {
                                name: entry.name.clone(),
                                diagnostics,
                            }]
                        }
                    };
                    for event in events {
                        if sender.send(event).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        watch
    }

    /// The project's plugin, then those of its dependencies in `cargo metadata` order, with a
    /// warning for each dependency that cannot be looked into.
    fn plugin_entries(&self) -> Result<(Vec<Entry>, Vec<PluginEvent>), Diagnostic> {
        let metadata = metadata(&self.root.join("Cargo.toml")).map_err(|error| {
            error_diagnostic(format!("cannot list the project's dependencies: {error}"))
        })?;
        let mut entries = vec![Entry {
            name: self.manifest.name.clone(),
            source: PluginSource::Project,
            directory: self.root.join("editor"),
        }];
        let mut warnings = Vec::new();
        for package in metadata.packages {
            let name = package.name.to_string();
            let Some(manifest_directory) = package.manifest_path.parent() else {
                continue;
            };
            let directory = match manifest_directory.as_std_path().canonicalize() {
                Ok(directory) => directory,
                Err(error) => {
                    let message =
                        format!("cannot look for a plugin in {manifest_directory}: {error}");
                    warnings.extend(warnings_of(&name, vec![warning_diagnostic(message)]));
                    continue;
                }
            };
            let editor = directory.join("editor");
            if directory != self.root && entry_file(&editor).is_some() {
                entries.push(Entry {
                    source: PluginSource::Engine {
                        crate_name: name.clone(),
                    },
                    name,
                    directory: editor,
                });
            }
        }
        Ok((entries, warnings))
    }
}

/// `cargo metadata` of a project, offline when every dependency is already downloaded.
fn metadata(manifest: &Path) -> Result<Metadata, cargo_metadata::Error> {
    let run = |offline: bool| {
        let mut command = MetadataCommand::new();
        command.manifest_path(manifest);
        if offline {
            command.other_options(vec!["--offline".to_string()]);
        }
        command.exec()
    };
    run(true).or_else(|_| run(false))
}

/// A `Warnings` event, when there is something to tell.
fn warnings_of(name: &str, diagnostics: Vec<Diagnostic>) -> Option<PluginEvent> {
    (!diagnostics.is_empty()).then(|| PluginEvent::Warnings {
        name: name.to_string(),
        diagnostics,
    })
}

/// The plugins whose `editor/` directory saw a change (or appeared). Accesses are not changes
/// (bundling opens and reads the sources, and on Linux those are events too), except closing a
/// file written to, which is how some editors save.
fn changed_entries<'a>(
    entries: &'a [Entry],
    events: impl IntoIterator<Item = &'a Event>,
) -> impl Iterator<Item = &'a Entry> {
    let paths: Vec<&PathBuf> = events
        .into_iter()
        .filter(|event| match event.kind {
            EventKind::Access(kind) => kind == AccessKind::Close(AccessMode::Write),
            _ => true,
        })
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

/// Bundles `entry` into `.ludenea/plugins/<name>.<hash>.js`, removing its older bundles. Returns
/// the bundle with rolldown's warnings, or every error.
fn bundle(root: &Path, entry: &Entry) -> Result<(PluginBundle, Vec<Diagnostic>), Vec<Diagnostic>> {
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
    let mut diagnostics = std::mem::take(&mut *rejected.lock().expect("not poisoned"));
    let output = match output {
        Ok(output) if diagnostics.is_empty() => output,
        Ok(_) => return Err(diagnostics),
        Err(errors) => {
            let errors = errors.into_vec();
            diagnostics.extend(
                errors
                    .iter()
                    .map(|error| rolldown_diagnostic(error, &paths)),
            );
            return Err(diagnostics);
        }
    };
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
    let warnings = output
        .warnings
        .iter()
        .map(|warning| rolldown_diagnostic(warning, &paths))
        .collect();
    let bundle = PluginBundle {
        name: entry.name.clone(),
        url_path: format!("plugins/{}", bundle_file_name(&entry.name, &code)),
        source: entry.source.clone(),
    };
    Ok((bundle, warnings))
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

fn warning_diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        level: "warning".to_string(),
        ..error_diagnostic(message)
    }
}

/// Rejects bare imports other than the shared modules, which rolldown marks external before
/// asking plugins, and imports of files that are not scripts.
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
        let message = if specifier.starts_with('.') || Path::new(specifier).is_absolute() {
            // A file named exactly so is what the import means (`./theme.dark` may still be
            // `./theme.dark.ts`).
            let named = match args
                .importer
                .and_then(|importer| Path::new(importer).parent())
            {
                Some(directory) => directory.join(specifier),
                None => PathBuf::from(specifier),
            };
            let script = named
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| SCRIPT_EXTENSIONS.contains(&extension));
            if script || !named.is_file() {
                return Ok(None);
            }
            format!(
                "cannot import {specifier:?}: editor plugins may import only JavaScript, \
                 TypeScript, and JSON files"
            )
        } else {
            format!(
                "cannot import {specifier:?}: editor plugins may import only React, the editor \
                 packages, and their own files (react, react/jsx-runtime, @ludenea/editor-shell, \
                 @ludenea/control-client, and relative paths)"
            )
        };
        self.rejected
            .lock()
            .expect("not poisoned")
            .push(Diagnostic {
                level: "error".to_string(),
                message,
                file: args
                    .importer
                    .map(|importer| relative_path(&self.root, importer)),
                line: None,
                column: None,
            });
        // External rather than an error, so rolldown goes on and reports the other problems;
        // a rejection fails the bundle, which is never written.
        Ok(Some(HookResolveIdOutput {
            id: specifier.into(),
            external: Some(ResolvedExternal::Bool(true)),
            ..Default::default()
        }))
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId
    }
}

#[cfg(test)]
mod tests {
    use notify::event::ModifyKind;

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

    /// Some editors save by writing in place: the close of that write is the change.
    #[test]
    fn closing_a_written_plugin_source_is_a_change() {
        let entries = [Entry {
            name: "demo".to_string(),
            source: PluginSource::Project,
            directory: PathBuf::from("/project/editor"),
        }];
        let source = PathBuf::from("/project/editor/index.ts");
        let closed = |mode| {
            Event::new(EventKind::Access(AccessKind::Close(mode))).add_path(source.clone())
        };
        let read = closed(AccessMode::Read);
        assert_eq!(changed_entries(&entries, [&read]).count(), 0);
        let written = closed(AccessMode::Write);
        assert_eq!(changed_entries(&entries, [&written]).count(), 1);
    }
}
