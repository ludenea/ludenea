use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludenea_project::{Diagnostic, PluginBundle, PluginEvent, PluginSource, Project};
use tempfile::TempDir;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/engine-plugin")
}

/// A minimal project named `demo` without the engine, so `cargo metadata` stays fast.
fn project(dir: &TempDir, dependencies: &str, files: &[(&str, &str)]) -> Project {
    let root = dir.path().join("demo");
    let cargo = format!(
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\n{dependencies}\n\n[workspace]\n"
    );
    let base = [
        ("ludenea.toml", "name = \"demo\"\nengine = \"0.1.0\"\n"),
        ("Cargo.toml", cargo.as_str()),
        ("src/lib.rs", ""),
    ];
    for (path, text) in base.iter().chain(files) {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    Project::open(&root).unwrap()
}

/// The bundles of a listing in which every plugin bundled.
fn bundles(project: &Project) -> Vec<PluginBundle> {
    project
        .bundle_plugins()
        .into_iter()
        .map(|event| match event {
            PluginEvent::Rebuilt(bundle) => bundle,
            failed => panic!("{failed:?}"),
        })
        .collect()
}

/// The diagnostics of the plugins that failed to bundle.
fn failures(project: &Project) -> Vec<Diagnostic> {
    project
        .bundle_plugins()
        .into_iter()
        .flat_map(|event| match event {
            PluginEvent::Failed { diagnostics, .. } => diagnostics,
            PluginEvent::Rebuilt(_) | PluginEvent::Warnings { .. } => Vec::new(),
        })
        .collect()
}

fn code(project: &Project, bundle: &PluginBundle) -> String {
    fs::read_to_string(project.root().join(".ludenea").join(&bundle.url_path)).unwrap()
}

const PANEL: &str = r#"import { useState } from "react";
import { createPortal } from "react-dom";
import { definePlugin } from "@ludenea/editor-shell";
import { connect } from "@ludenea/control-client";
import { Counter } from "./counter";

export default definePlugin({ panel: () => <Counter start={1} />, connect, createPortal });
"#;

const COUNTER: &str = r#"import { useState } from "react";

type Props = { start: number };

export function Counter(props: Props) {
  const [count] = useState<number>(props.start);
  return <span className="own-file-marker">{count}</span>;
}
"#;

#[test]
fn bundles_tsx_plugin_with_shared_imports_external() {
    let dir = TempDir::new().unwrap();
    let project = project(
        &dir,
        "",
        &[
            ("editor/index.tsx", PANEL),
            ("editor/counter.tsx", COUNTER),
            // A local install must never end up in the bundle.
            (
                "editor/node_modules/react/index.js",
                "export const useState = 'react-source-marker';",
            ),
            (
                "editor/node_modules/react/package.json",
                r#"{ "name": "react", "main": "index.js" }"#,
            ),
        ],
    );
    let bundles = bundles(&project);
    assert_eq!(bundles.len(), 1);
    let bundle = &bundles[0];
    assert_eq!(bundle.name, "demo");
    assert_eq!(bundle.source, PluginSource::Project);
    assert!(
        bundle.url_path.starts_with("plugins/demo."),
        "{}",
        bundle.url_path
    );
    assert!(bundle.url_path.ends_with(".js"), "{}", bundle.url_path);
    let code = code(&project, bundle);
    for import in [
        "from \"react\"",
        "from \"react/jsx-runtime\"",
        "from \"react-dom\"",
        "from \"@ludenea/editor-shell\"",
        "from \"@ludenea/control-client\"",
    ] {
        assert!(code.contains(import), "missing {import} in\n{code}");
    }
    assert!(code.contains("own-file-marker"), "{code}");
    assert!(!code.contains("jsx-dev-runtime"), "{code}");
    assert!(!code.contains("react-source-marker"), "{code}");
    assert!(!code.contains("type Props"), "{code}");
}

#[test]
fn index_ts_and_index_tsx_are_both_entries() {
    for entry in ["editor/index.ts", "editor/index.tsx"] {
        let dir = TempDir::new().unwrap();
        let project = project(
            &dir,
            "",
            &[(entry, "export const which: string = 'entry';")],
        );
        let bundles = bundles(&project);
        assert_eq!(bundles.len(), 1, "{entry}");
        assert!(code(&project, &bundles[0]).contains("entry"), "{entry}");
    }
}

#[test]
fn third_party_import_is_rejected_with_rule() {
    let dir = TempDir::new().unwrap();
    let project = project(
        &dir,
        "",
        &[
            (
                "editor/index.ts",
                "import pad from \"left-pad\";\nexport default pad;\n",
            ),
            ("editor/node_modules/left-pad/index.js", "export default 1;"),
        ],
    );
    let diagnostics = failures(&project);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.level, "error");
    assert_eq!(diagnostic.file.as_deref(), Some("editor/index.ts"));
    assert!(
        diagnostic.message.contains("\"left-pad\""),
        "{diagnostic:?}"
    );
    assert!(
        diagnostic
            .message
            .contains("may import only React, the editor packages, and their own files"),
        "{diagnostic:?}"
    );
}

#[test]
fn stale_bundle_that_cannot_be_removed_does_not_fail_the_bundle() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "", &[("editor/index.ts", "export default 1;")]);
    // Named like an older bundle of the plugin, but a directory: removing it as a file fails.
    let stale = project
        .root()
        .join(".ludenea/plugins/demo.0000000000000000.js");
    fs::create_dir_all(&stale).unwrap();
    let bundles = bundles(&project);
    assert!(code(&project, &bundles[0]).contains("export"));
}

#[test]
fn syntax_error_reports_file_and_line() {
    let dir = TempDir::new().unwrap();
    let project = project(
        &dir,
        "",
        &[(
            "editor/index.ts",
            "export const a = 1;\nexport const b = (;\n",
        )],
    );
    let diagnostics = failures(&project);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.level, "error");
    assert_eq!(diagnostic.file.as_deref(), Some("editor/index.ts"));
    assert_eq!(diagnostic.line, Some(2));
    assert!(diagnostic.column.is_some(), "{diagnostic:?}");
}

#[test]
fn engine_plugin_of_dependency_is_discovered() {
    let dir = TempDir::new().unwrap();
    let dependency = format!(
        "engine-plugin = {{ path = {:?} }}",
        fixture().to_string_lossy().replace('\\', "/")
    );
    let project = project(
        &dir,
        &dependency,
        &[("editor/index.ts", "export default 'project';")],
    );
    let bundles = bundles(&project);
    let sources: Vec<_> = bundles
        .iter()
        .map(|bundle| (bundle.name.as_str(), bundle.source.clone()))
        .collect();
    assert_eq!(
        sources,
        [
            ("demo", PluginSource::Project),
            (
                "engine-plugin",
                PluginSource::Engine {
                    crate_name: "engine-plugin".to_string()
                }
            ),
        ]
    );
    assert!(code(&project, &bundles[1]).contains("engine-plugin fixture"));
}

#[test]
fn editing_plugin_source_emits_rebuilt_with_new_hash() {
    let dir = TempDir::new().unwrap();
    let project = project(
        &dir,
        "",
        &[
            ("editor/index.ts", "export * from './greeting';"),
            ("editor/greeting.ts", "export const greeting = 'before';"),
        ],
    );
    let before = bundles(&project).remove(0);
    let events = project.watch_plugins();
    fs::write(
        project.root().join("editor/greeting.ts"),
        "export const greeting = 'after';",
    )
    .unwrap();
    let mut seen = Vec::new();
    while let Ok(event) = events.recv_timeout(Duration::from_secs(15)) {
        if let PluginEvent::Rebuilt(bundle) = &event
            && bundle.url_path != before.url_path
        {
            assert_eq!(bundle.name, "demo");
            assert!(code(&project, bundle).contains("after"));
            return;
        }
        seen.push(event);
    }
    panic!("never saw a rebuild with a new hash; saw {seen:?}");
}

#[test]
fn a_failing_plugin_does_not_keep_the_others_from_loading() {
    let dir = TempDir::new().unwrap();
    let broken = dir.path().join("broken");
    for (path, text) in [
        (
            "Cargo.toml",
            "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        ),
        ("src/lib.rs", ""),
        ("editor/index.ts", "export const b = (;\n"),
    ] {
        let file = broken.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let dependency = format!(
        "broken = {{ path = {:?} }}",
        broken.to_string_lossy().replace('\\', "/")
    );
    let project = project(
        &dir,
        &dependency,
        &[("editor/index.ts", "export default 'project';")],
    );
    let listed = project.bundle_plugins();
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert!(
        matches!(&listed[0], PluginEvent::Rebuilt(bundle) if bundle.name == "demo"),
        "{listed:?}"
    );
    assert!(
        matches!(&listed[1], PluginEvent::Failed { name, diagnostics } if name == "broken" && diagnostics[0].line == Some(1)),
        "{listed:?}"
    );
}

#[test]
fn non_javascript_import_is_rejected_with_rule() {
    let dir = TempDir::new().unwrap();
    let project = project(
        &dir,
        "",
        &[
            (
                "editor/index.ts",
                "import \"./style.css\";\nexport default 1;\n",
            ),
            ("editor/style.css", "body { color: red; }"),
        ],
    );
    let diagnostics = failures(&project);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.file.as_deref(), Some("editor/index.ts"));
    assert!(diagnostic.message.contains("style.css"), "{diagnostic:?}");
    assert!(
        diagnostic
            .message
            .contains("may import only JavaScript, TypeScript, and JSON files"),
        "{diagnostic:?}"
    );
}

#[test]
fn a_rejected_import_does_not_hide_other_errors() {
    let dir = TempDir::new().unwrap();
    let project = project(
        &dir,
        "",
        &[
            (
                "editor/index.ts",
                "import pad from \"left-pad\";\nimport { b } from \"./broken\";\nexport default [pad, b];\n",
            ),
            ("editor/broken.ts", "export const b = (;\n"),
        ],
    );
    let diagnostics = failures(&project);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("\"left-pad\"")),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.file.as_deref() == Some("editor/broken.ts") && diagnostic.line == Some(1)
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn bundling_warnings_are_reported_next_to_the_bundle() {
    let dir = TempDir::new().unwrap();
    let project = project(
        &dir,
        "",
        &[("editor/index.ts", "export default eval(\"1\");\n")],
    );
    let events = project.bundle_plugins();
    assert!(
        matches!(&events[0], PluginEvent::Rebuilt(bundle) if bundle.name == "demo"),
        "{events:?}"
    );
    assert!(
        matches!(
            &events[1..],
            [PluginEvent::Warnings { name, diagnostics }]
                if name == "demo" && diagnostics.iter().all(|diagnostic| diagnostic.level == "warning")
        ),
        "{events:?}"
    );
}

#[test]
fn index_ts_wins_over_index_tsx() {
    let dir = TempDir::new().unwrap();
    let project = project(
        &dir,
        "",
        &[
            ("editor/index.ts", "export const which = 'ts-entry';"),
            ("editor/index.tsx", "export const which = 'tsx-entry';"),
        ],
    );
    let code = code(&project, &bundles(&project)[0]);
    assert!(
        code.contains("ts-entry") && !code.contains("tsx-entry"),
        "{code}"
    );
}

/// The next event of `events`, failing after a generous timeout.
fn next_event(events: &ludenea_project::Watch<PluginEvent>) -> PluginEvent {
    events
        .recv_timeout(Duration::from_secs(15))
        .expect("an event arrives")
}

/// Events arriving within a second and a half.
fn quiet(events: &ludenea_project::Watch<PluginEvent>) -> Vec<PluginEvent> {
    std::iter::from_fn(|| events.recv_timeout(Duration::from_millis(1500)).ok()).collect()
}

#[test]
fn a_project_plugin_created_after_watching_starts_is_bundled() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "", &[]);
    let events = project.watch_plugins();
    std::thread::sleep(Duration::from_millis(500));
    fs::create_dir_all(project.root().join("editor")).unwrap();
    fs::write(
        project.root().join("editor/index.ts"),
        "export default 'late';",
    )
    .unwrap();
    match next_event(&events) {
        PluginEvent::Rebuilt(bundle) => {
            assert_eq!(bundle.name, "demo");
            assert!(code(&project, &bundle).contains("late"));
        }
        other => panic!("{other:?}"),
    }
}

/// Deleting `editor/` and creating it again keeps the plugin watched.
#[test]
fn a_recreated_project_plugin_directory_is_watched_again() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "", &[("editor/index.ts", "export default 'first';")]);
    let editor = project.root().join("editor");
    let events = project.watch_plugins();
    std::thread::sleep(Duration::from_millis(500));
    fs::remove_dir_all(&editor).unwrap();
    quiet(&events);
    fs::create_dir_all(&editor).unwrap();
    fs::write(editor.join("index.ts"), "export default 'second';").unwrap();
    match next_event(&events) {
        PluginEvent::Rebuilt(bundle) => assert!(code(&project, &bundle).contains("second")),
        other => panic!("{other:?}"),
    }
    std::thread::sleep(Duration::from_millis(500));
    fs::write(editor.join("index.ts"), "export default 'third';").unwrap();
    match next_event(&events) {
        PluginEvent::Rebuilt(bundle) => assert!(code(&project, &bundle).contains("third")),
        other => panic!("{other:?}"),
    }
}

/// A failure is reported; the next good bundle is announced even if it is the one announced
/// before; a save that changes nothing announces nothing; only the changed plugin rebuilds.
#[test]
fn watching_reports_failures_and_only_news() {
    let dir = TempDir::new().unwrap();
    let other = dir.path().join("other");
    for (path, text) in [
        (
            "Cargo.toml",
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        ),
        ("src/lib.rs", ""),
        ("editor/index.ts", "export default 'other';\n"),
    ] {
        let file = other.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let dependency = format!(
        "other = {{ path = {:?} }}",
        other.to_string_lossy().replace('\\', "/")
    );
    let good = "export default 'good';\n";
    let project = project(&dir, &dependency, &[("editor/index.ts", good)]);
    let index = project.root().join("editor/index.ts");
    let events = project.watch_plugins();
    std::thread::sleep(Duration::from_millis(500));

    fs::write(&index, "export const b = (;\n").unwrap();
    assert!(
        matches!(next_event(&events), PluginEvent::Failed { name, .. } if name == "demo"),
        "a failure is reported"
    );
    fs::write(&index, good).unwrap();
    let PluginEvent::Rebuilt(first) = next_event(&events) else {
        panic!("the repaired plugin is announced");
    };
    assert_eq!(first.name, "demo");
    assert_eq!(quiet(&events), [], "only the changed plugin rebuilds");

    fs::write(&index, good).unwrap();
    assert_eq!(quiet(&events), [], "an unchanged bundle is not news");
    fs::write(&index, "export default 'better';\n").unwrap();
    assert!(
        matches!(next_event(&events), PluginEvent::Rebuilt(bundle) if bundle.name == "demo" && bundle.url_path != first.url_path)
    );
}
