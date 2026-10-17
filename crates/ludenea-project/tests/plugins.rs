use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludenea_project::{PluginBundle, PluginEvent, PluginSource, Project};
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

fn code(project: &Project, bundle: &PluginBundle) -> String {
    fs::read_to_string(project.root().join(".ludenea").join(&bundle.url_path)).unwrap()
}

const PANEL: &str = r#"import { useState } from "react";
import { definePlugin } from "@ludenea/editor-shell";
import { connect } from "@ludenea/control-client";
import { Counter } from "./counter";

export default definePlugin({ panel: () => <Counter start={1} />, connect });
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
    let bundles = project.bundle_plugins().unwrap();
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
        let bundles = project.bundle_plugins().unwrap();
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
    let diagnostics = project.bundle_plugins().unwrap_err();
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
    let bundles = project.bundle_plugins().unwrap();
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
    let diagnostics = project.bundle_plugins().unwrap_err();
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
    let bundles = project.bundle_plugins().unwrap();
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
    let before = project.bundle_plugins().unwrap().remove(0);
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
