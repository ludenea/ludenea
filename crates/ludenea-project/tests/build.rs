//! These tests run real `cargo build`s of a generated project, so they are `#[ignore]`d; run
//! them with `cargo test -p ludenea-project --test build -- --ignored`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use ludenea_control::{
    ClientMessage, GameInfo, NamedSchema, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, ServerMessage,
};
use ludenea_project::{
    BuildArtifact, BuildError, BuildEvent, BuildTarget, GameProcess, OutputStream, Project,
};
use ludenea_schema::{FieldSchema, TypeSchema};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(60);

fn new_project(dir: &TempDir) -> Project {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Project::create(dir.path(), "demo", &engine).expect("the project is created")
}

fn build(
    project: &Project,
    target: BuildTarget,
) -> (Result<BuildArtifact, BuildError>, Vec<BuildEvent>) {
    let (sender, receiver) = mpsc::channel();
    let handle = project.build(target, sender);
    let result = handle.join().expect("the build thread does not panic");
    (result, receiver.try_iter().collect())
}

/// A natively built project whose `app()` prints a line, shared by the tests that run it.
fn game_binary() -> &'static PathBuf {
    static BUILT: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &BUILT
        .get_or_init(|| {
            let dir = TempDir::new().unwrap();
            let mut project = new_project(&dir);
            let lib = project.read_text("src/lib.rs").unwrap();
            let lib = lib.replace(
                "let mut app = App::new();",
                "println!(\"game says hello\");\n    let mut app = App::new();",
            );
            project.write_text("src/lib.rs", &lib).unwrap();
            let (result, _) = build(&project, BuildTarget::Native);
            let Ok(BuildArtifact::Native { binary }) = result else {
                panic!("native build failed: {result:?}");
            };
            (dir, binary)
        })
        .1
}

fn handshake(game: &mut GameProcess, messages: &Receiver<String>) -> GameInfo {
    let hello = ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        protocol_fingerprint: PROTOCOL_FINGERPRINT,
    };
    game.send(&serde_json::to_string(&hello).unwrap()).unwrap();
    let line = messages.recv_timeout(TIMEOUT).expect("a welcome arrives");
    match serde_json::from_str(&line).unwrap() {
        ServerMessage::Welcome { game, .. } => game,
        other => panic!("{other:?}"),
    }
}

#[test]
#[ignore]
fn native_build_produces_binary_and_reports_progress() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let (result, events) = build(&project, BuildTarget::Native);
    let Ok(BuildArtifact::Native { binary }) = result else {
        panic!("{result:?}");
    };
    assert!(binary.is_file());
    assert!(matches!(
        events.last(),
        Some(BuildEvent::Finished { ok: true })
    ));
    let progress: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            BuildEvent::Progress { done, total } => Some((*done, *total)),
            _ => None,
        })
        .collect();
    assert!(!progress.is_empty());
    assert!(progress.iter().all(|(_, total)| total.is_none()));
    assert!(
        dir.path()
            .join("demo/.ludenea/build/native.count")
            .is_file()
    );

    // The second build knows how many artifacts to expect.
    let (_, events) = build(&project, BuildTarget::Native);
    assert!(events.iter().any(|event| matches!(
        event,
        BuildEvent::Progress { total: Some(total), .. } if *total > 0
    )));
}

#[test]
#[ignore]
fn build_error_reports_file_and_line() {
    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    let lib = project.read_text("src/lib.rs").unwrap();
    project
        .write_text(
            "src/lib.rs",
            &format!("{lib}\nfn broken() -> u32 {{ \"no\" }}\n"),
        )
        .unwrap();
    let (result, events) = build(&project, BuildTarget::Native);
    assert!(matches!(result, Err(BuildError::Failed)), "{result:?}");
    assert!(matches!(
        events.last(),
        Some(BuildEvent::Finished { ok: false })
    ));
    let error = events
        .iter()
        .find_map(|event| match event {
            BuildEvent::Diagnostic(diagnostic) if diagnostic.level == "error" => Some(diagnostic),
            _ => None,
        })
        .expect("an error diagnostic");
    assert_eq!(error.file.as_deref(), Some("src/lib.rs"));
    assert!(error.line.is_some_and(|line| line > 0));
    assert!(
        error.message.contains("mismatched types"),
        "{}",
        error.message
    );
}

#[test]
#[ignore]
fn wasm_build_produces_glue_and_wasm_in_hashed_dir() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let (result, _) = build(&project, BuildTarget::Wasm);
    let Ok(BuildArtifact::Wasm { dir: out }) = result else {
        panic!("{result:?}");
    };
    assert!(out.join("game.js").is_file());
    assert!(out.join("game_bg.wasm").is_file());
    let parent = out.parent().unwrap();
    assert!(parent.ends_with(".ludenea/build/wasm"));
    let name = out.file_name().unwrap().to_str().unwrap();
    assert_eq!(name.len(), 16);
    assert!(name.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
#[ignore]
fn game_process_handshakes_over_stdio() {
    let mut game = GameProcess::start(game_binary()).unwrap();
    let messages = game.take_messages().unwrap();
    let info = handshake(&mut game, &messages);
    assert!(info.tick_rate > 0);
    game.stop();
}

#[test]
#[ignore]
fn game_println_is_output_not_protocol() {
    let mut game = GameProcess::start(game_binary()).unwrap();
    let messages = game.take_messages().unwrap();
    let output = game.take_output().unwrap();
    handshake(&mut game, &messages);
    let line = output
        .recv_timeout(TIMEOUT)
        .expect("the game output arrives");
    assert_eq!(line.stream, OutputStream::Stdout);
    assert_eq!(line.line, "game says hello");
    game.stop();
}

#[test]
#[ignore]
fn game_types_declare_template_components() {
    let mut game = GameProcess::start(game_binary()).unwrap();
    let messages = game.take_messages().unwrap();
    let info = handshake(&mut game, &messages);
    game.stop();
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    project.write_game_types(&info).unwrap();
    let types = dir.path().join("demo/.ludenea/types");
    let declarations = std::fs::read_to_string(types.join("game.d.ts")).unwrap();
    assert!(declarations.contains("declare module \"@ludenea/game\""));
    assert!(declarations.contains("declare module \"@ludenea/editor-shell\""));
    assert!(declarations.contains("interface ComponentTypes"));
    assert!(declarations.contains("interface AssetTypes extends GameAssetTypes"));
    assert!(
        declarations.contains("\"demo::Settings\": Settings"),
        "{declarations}"
    );
    let component = info.components[0].name.rsplit("::").next().unwrap();
    assert!(declarations.contains(component), "{declarations}");
    for file in ["editor-shell.d.ts", "control-client.d.ts", "react.d.ts"] {
        assert!(types.join(file).is_file(), "{file}");
    }
}

/// Type-checks against the template's own types, so `@ts-expect-error` fails the check if an
/// unknown component name is ever accepted.
/// A game component named like an editor export (`demo::Editor`) must not clash with it.
const TYPE_PROBE: &str = r#"import type { Editor } from "@ludenea/editor-shell";
import type { Editor as GameEditor, Pose } from "@ludenea/game";
import type { Entity } from "@ludenea/control-client";

type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;

export async function probe(editor: Editor, entity: Entity): Promise<number | undefined> {
  const pose = await editor.entities.get(entity, "ludenea::Pose");
  const exact: Same<typeof pose, Pose | undefined> = true;
  const named = await editor.entities.get(entity, "demo::Editor");
  const game: Same<typeof named, GameEditor | undefined> = true;
  // @ts-expect-error: not a component of this game
  await editor.entities.get(entity, "ludenea::Nothing");
  return exact && game ? pose?.position.x : named?.title.length;
}
"#;

/// Needs the editor's `node_modules` (`pnpm -C editor install`).
#[test]
#[ignore]
fn generated_game_types_type_check_project_plugins() {
    let mut game = GameProcess::start(game_binary()).unwrap();
    let messages = game.take_messages().unwrap();
    let mut info = handshake(&mut game, &messages);
    game.stop();
    info.components.push(NamedSchema {
        name: "demo::Editor".to_string(),
        schema: TypeSchema::Struct {
            name: "demo::Editor".to_string(),
            fields: vec![FieldSchema {
                name: "title".to_string(),
                schema: TypeSchema::String,
            }],
        },
    });
    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    project.write_game_types(&info).unwrap();
    let editor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editor");
    // The end-to-end test plugin, which reads `ludenea::Body` through the typed API.
    let fixture =
        std::fs::read_to_string(editor.join("e2e/fixtures/test-plugin/index.tsx")).unwrap();
    project.write_text("editor/index.tsx", &fixture).unwrap();
    project.write_text("editor/probe.ts", TYPE_PROBE).unwrap();
    let output = std::process::Command::new(editor.join("node_modules/.bin/tsc"))
        .args(["--noEmit", "-p"])
        .arg(dir.path().join("demo/editor"))
        .output()
        .expect("tsc runs (pnpm -C editor install)");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn wait_is_none_while_running_and_has_the_code_after_exit() {
    let mut game = GameProcess::start(game_binary()).unwrap();
    let messages = game.take_messages().unwrap();
    handshake(&mut game, &messages);
    assert_eq!(game.wait(), None);
    assert!(game.take_messages().is_none(), "taken once");
    game.close_input();
    assert!(game.send("{}").is_err());
    // The receiver disconnects when the game exits.
    while messages.recv_timeout(TIMEOUT).is_ok() {}
    assert!(matches!(
        messages.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    let mut code = game.wait();
    for _ in 0..100 {
        if code.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        code = game.wait();
    }
    assert_eq!(code.and_then(|status| status.code()), Some(0));
}
