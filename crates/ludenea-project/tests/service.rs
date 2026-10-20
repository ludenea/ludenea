//! The `#[ignore]`d tests run real `cargo build`s; run them with
//! `cargo test -p ludenea-project --test service -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludenea_control::{
    Bytes, ClientMessage, GameInfo, Hash64, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, ServerMessage,
};
use ludenea_ecs::AssetId;
use ludenea_project::protocol::{
    FailureCode, PROJECT_PROTOCOL_FINGERPRINT, PROJECT_PROTOCOL_VERSION, ProjectClientMessage,
    ProjectEvent, ProjectReply, ProjectRequest, ProjectServerMessage, ServiceMode,
};
use ludenea_project::service::ProjectService;
use ludenea_project::{AssetKind, BuildEvent, Project};
use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;

const POLL: Duration = Duration::from_millis(20);
/// Fifteen minutes of polls: a first build compiles the whole engine.
const BUILD_POLLS: u32 = 45_000;

fn engine() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A minimal project named `demo` without the engine, so its background build fails fast.
fn minimal_project(dir: &TempDir) -> PathBuf {
    let root = dir.path().join("demo");
    let files = [
        ("ludenea.toml", "name = \"demo\"\nengine = \"0.1.0\"\n"),
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        ),
        ("src/lib.rs", ""),
        ("scenes/main.scene.ron", "(entities: [])\n"),
    ];
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    root
}

fn open(service: &mut ProjectService, root: &Path) -> ProjectReply {
    let path = root.to_string_lossy().into_owned();
    service.handle(ProjectRequest::Open { path }).unwrap()
}

/// Waits for an event `wanted` accepts, failing on `GameExited` and after the timeout.
fn wait_for(events: &mut Receiver<ProjectEvent>, wanted: impl Fn(&ProjectEvent) -> bool) {
    for _ in 0..BUILD_POLLS {
        match events.try_recv() {
            Ok(event) if wanted(&event) => return,
            Ok(ProjectEvent::GameExited { code }) => panic!("the game exited with {code:?}"),
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => std::thread::sleep(POLL),
            Err(TryRecvError::Closed) => panic!("the service is gone"),
        }
    }
    panic!("timed out");
}

fn finished(ok: bool) -> impl Fn(&ProjectEvent) -> bool {
    move |event| matches!(event, ProjectEvent::Build { event: BuildEvent::Finished { ok: done } } if *done == ok)
}

/// Sends a control request through the relay channel and returns the response line.
fn ask_game(
    service: &mut ProjectService,
) -> (
    std::sync::mpsc::Sender<String>,
    std::sync::mpsc::Receiver<String>,
) {
    let (to_game, from_game) = service.take_control_channel().expect("a game is running");
    let hello = ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        protocol_fingerprint: PROTOCOL_FINGERPRINT,
    };
    to_game
        .send(serde_json::to_string(&hello).unwrap())
        .unwrap();
    let line = from_game.recv_timeout(Duration::from_secs(60)).unwrap();
    assert!(
        matches!(
            serde_json::from_str(&line).unwrap(),
            ServerMessage::Welcome { .. }
        ),
        "{line}"
    );
    (to_game, from_game)
}

#[test]
fn list_and_read_without_build() {
    let dir = TempDir::new().unwrap();
    let root = minimal_project(&dir);
    let mut service = ProjectService::new(ServiceMode::Browser, None);
    let ProjectReply::Info(info) = open(&mut service, &root) else {
        panic!("Open answers Info");
    };
    assert_eq!(info.name, "demo");
    assert_eq!(info.mode, ServiceMode::Browser);
    let ProjectReply::Files { entries } = service.handle(ProjectRequest::ListFiles).unwrap() else {
        panic!("ListFiles answers Files");
    };
    let paths: Vec<_> = entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["scenes/main.scene.ron"]);
    let read = service.handle(ProjectRequest::ReadText {
        path: "scenes/main.scene.ron".to_string(),
    });
    assert_eq!(
        read.unwrap(),
        ProjectReply::Text {
            text: "(entities: [])\n".to_string()
        }
    );
    let endpoint = service.handle(ProjectRequest::GameEndpoint).unwrap_err();
    assert_eq!(endpoint.code, FailureCode::NoGame);
}

fn hello(protocol_version: u32, protocol_fingerprint: Hash64) -> String {
    serde_json::to_string(&ProjectClientMessage::Hello {
        protocol_version,
        protocol_fingerprint,
    })
    .unwrap()
}

/// Answers `hello` to a new service, asserting it is refused as another version, and returns
/// the refusal's message.
fn refused_hello(hello: &str) -> String {
    let mut service = ProjectService::new(ServiceMode::Desktop, None);
    let mut connected = false;
    let ProjectServerMessage::Refused { error } = service.answer(&mut connected, hello) else {
        panic!("{hello} is welcomed");
    };
    assert_eq!(error.code, FailureCode::VersionMismatch);
    assert!(!connected);
    error.message
}

#[test]
fn hello_with_another_project_protocol_version_is_refused() {
    let message = refused_hello(&hello(
        PROJECT_PROTOCOL_VERSION + 1,
        PROJECT_PROTOCOL_FINGERPRINT,
    ));
    assert_eq!(
        message,
        format!(
            "client speaks project protocol {}, service speaks {PROJECT_PROTOCOL_VERSION}; \
             rebuild the editor (pnpm -C editor build) and the service from the same checkout",
            PROJECT_PROTOCOL_VERSION + 1
        )
    );
}

#[test]
fn hello_of_an_earlier_version_is_refused_as_such() {
    // Version 2 had no fingerprint, so its hello is not a message of this version.
    let message = refused_hello(r#"{"type":"hello","protocol_version":2}"#);
    assert_eq!(
        message,
        format!(
            "client speaks project protocol 2, service speaks {PROJECT_PROTOCOL_VERSION}; \
             rebuild the editor (pnpm -C editor build) and the service from the same checkout"
        )
    );
}

#[test]
fn a_hello_that_does_not_decode_says_what_it_needs() {
    let unreadable = [
        format!(r#"{{"type":"hello","protocol_version":{PROJECT_PROTOCOL_VERSION}}}"#),
        format!(
            r#"{{"type":"hello","protocol_version":{PROJECT_PROTOCOL_VERSION},"protocol_fingerprint":42}}"#
        ),
        r#"{"type":"hello"}"#.to_string(),
    ];
    for hello in unreadable {
        let mut service = ProjectService::new(ServiceMode::Desktop, None);
        let mut connected = false;
        let ProjectServerMessage::Refused { error } = service.answer(&mut connected, &hello) else {
            panic!("{hello} is welcomed");
        };
        assert_eq!(error.code, FailureCode::InvalidRequest, "{hello}");
        let reason = serde_json::from_str::<ProjectClientMessage>(&hello).unwrap_err();
        assert_eq!(
            error.message,
            format!(
                "cannot read this hello: {reason}; a hello needs protocol_version and \
                 protocol_fingerprint"
            ),
            "{hello}"
        );
        assert!(!connected);
    }
}

#[test]
fn hello_with_another_fingerprint_is_refused() {
    let other = Hash64(PROJECT_PROTOCOL_FINGERPRINT.0 ^ 1);
    let message = refused_hello(&hello(PROJECT_PROTOCOL_VERSION, other));
    assert_eq!(
        message,
        format!(
            "client speaks another revision of project protocol {PROJECT_PROTOCOL_VERSION} \
             (fingerprint {:016x}, service {:016x}); rebuild the editor (pnpm -C editor build) \
             and the service from the same checkout",
            other.0, PROJECT_PROTOCOL_FINGERPRINT.0
        )
    );
}

/// Each request a minimal project answers succeeds with a reply `ProjectRequest::reply_types`
/// names. `Create` builds a real project, so `Open` stands for it (both reply through the
/// same install); `GameEndpoint` and `RestartGame` need a built game, and the ignored build
/// tests and the service's own tests check their replies.
#[test]
fn every_request_names_the_replies_it_gets() {
    let dir = TempDir::new().unwrap();
    let root = minimal_project(&dir);
    let mut service = ProjectService::new(ServiceMode::Browser, None);
    let scene = || "scenes/main.scene.ron".to_string();
    let requests = [
        ProjectRequest::Open {
            path: root.to_string_lossy().into_owned(),
        },
        ProjectRequest::Info,
        ProjectRequest::ListFiles,
        ProjectRequest::ReadText { path: scene() },
        ProjectRequest::WriteText {
            path: scene(),
            text: "(entities: [])\n".to_string(),
        },
        ProjectRequest::WriteBinary {
            path: "assets/blob.bin".to_string(),
            bytes: Bytes(vec![1, 2]),
        },
        ProjectRequest::ReadBinary {
            path: "assets/blob.bin".to_string(),
        },
        ProjectRequest::CreateAsset {
            path: "scenes/other.scene.ron".to_string(),
            kind: AssetKind::Scene,
            text: "(entities: [])\n".to_string(),
        },
        ProjectRequest::ReadAssetTexts,
        ProjectRequest::StoreSession {
            document: AssetId(7),
            bytes: Bytes(vec![3]),
        },
        ProjectRequest::LoadStoredSession {
            document: AssetId(7),
        },
        ProjectRequest::SetAsideStoredSession {
            document: AssetId(7),
        },
        ProjectRequest::Rename {
            from: "scenes/other.scene.ron".to_string(),
            to: "scenes/renamed.scene.ron".to_string(),
        },
        ProjectRequest::Delete {
            path: "scenes/renamed.scene.ron".to_string(),
        },
        ProjectRequest::ListPlugins,
        ProjectRequest::WriteGameTypes {
            info: GameInfo {
                components: Vec::new(),
                resources: Vec::new(),
                assets: Vec::new(),
                events: Vec::new(),
                fingerprint: Hash64(0),
                input: None,
                tick_rate: 60,
                debug_layers: Vec::new(),
            },
        },
        ProjectRequest::Build,
    ];
    for request in requests {
        let name = format!("{request:?}");
        let expected = request.reply_types();
        let reply = service
            .handle(request)
            .unwrap_or_else(|failure| panic!("{name} failed: {failure:?}"));
        let reply_type = serde_json::to_value(&reply).unwrap()["type"].clone();
        assert!(
            expected.iter().any(|expected| reply_type == *expected),
            "{name} replied {reply_type}, not one of {expected:?}"
        );
    }
}

#[test]
fn requests_outside_project_are_rejected() {
    let dir = TempDir::new().unwrap();
    let root = minimal_project(&dir);
    fs::write(dir.path().join("secret.txt"), "secret").unwrap();
    let mut service = ProjectService::new(ServiceMode::Browser, None);
    let before = service.handle(ProjectRequest::ListFiles).unwrap_err();
    assert_eq!(before.code, FailureCode::NoProject);
    open(&mut service, &root);
    let outside = "../secret.txt".to_string();
    let absolute = dir.path().join("secret.txt").to_string_lossy().into_owned();
    let requests = [
        ProjectRequest::ReadText {
            path: outside.clone(),
        },
        ProjectRequest::ReadText { path: absolute },
        ProjectRequest::WriteText {
            path: outside.clone(),
            text: "overwritten".to_string(),
        },
        ProjectRequest::Rename {
            from: "scenes/main.scene.ron".to_string(),
            to: outside.clone(),
        },
        ProjectRequest::Delete { path: outside },
    ];
    for request in requests {
        let failure = service.handle(request.clone()).unwrap_err();
        assert_eq!(failure.code, FailureCode::OutsideProject, "{request:?}");
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("secret.txt")).unwrap(),
        "secret"
    );
    assert!(root.join("scenes/main.scene.ron").exists());
}

#[test]
fn opening_another_project_is_announced() {
    let dir = TempDir::new().unwrap();
    let root = minimal_project(&dir);
    let mut service = ProjectService::new(ServiceMode::Browser, None);
    let mut events = service.subscribe();
    let ProjectReply::Info(info) = open(&mut service, &root) else {
        panic!("Open answers Info");
    };
    wait_for(
        &mut events,
        |event| matches!(event, ProjectEvent::ProjectOpened { info: opened } if *opened == info),
    );
}

#[test]
fn list_plugins_reports_a_failing_plugin_as_an_event() {
    let dir = TempDir::new().unwrap();
    let root = minimal_project(&dir);
    fs::create_dir_all(root.join("editor")).unwrap();
    fs::write(root.join("editor/index.ts"), "export const b = (;\n").unwrap();
    let mut service = ProjectService::new(ServiceMode::Browser, None);
    open(&mut service, &root);
    let mut events = service.subscribe();
    assert_eq!(
        service.handle(ProjectRequest::ListPlugins).unwrap(),
        ProjectReply::Plugins { plugins: vec![] }
    );
    wait_for(
        &mut events,
        |event| matches!(event, ProjectEvent::PluginFailed { name, diagnostics } if name == "demo" && !diagnostics.is_empty()),
    );
}

#[test]
fn create_without_engine_path_names_the_setting() {
    let dir = TempDir::new().unwrap();
    let mut service = ProjectService::new(ServiceMode::Desktop, None);
    let failure = service
        .handle(ProjectRequest::Create {
            parent: dir.path().to_string_lossy().into_owned(),
            name: "demo".to_string(),
            engine_path: None,
        })
        .unwrap_err();
    assert_eq!(failure.code, FailureCode::NoEnginePath);
    assert!(
        failure.message.contains("ludenea.engine-path"),
        "{}",
        failure.message
    );
    assert!(!dir.path().join("demo").exists());
}

#[test]
#[ignore]
fn open_builds_in_background_and_reports_game_ready() {
    let dir = TempDir::new().unwrap();
    let root = Project::create(dir.path(), "demo", &engine())
        .unwrap()
        .root()
        .to_path_buf();
    let mut service = ProjectService::new(ServiceMode::Desktop, None);
    let mut events = service.subscribe();
    open(&mut service, &root);
    wait_for(&mut events, finished(true));
    wait_for(&mut events, |event| {
        matches!(event, ProjectEvent::GameReady)
    });
    assert_eq!(
        service.handle(ProjectRequest::GameEndpoint).unwrap(),
        ProjectReply::Native
    );
    ask_game(&mut service);
}

#[test]
#[ignore]
fn failed_rebuild_keeps_previous_game_in_both_modes() {
    for mode in [ServiceMode::Desktop, ServiceMode::Browser] {
        let dir = TempDir::new().unwrap();
        let root = Project::create(dir.path(), "demo", &engine())
            .unwrap()
            .root()
            .to_path_buf();
        let mut service = ProjectService::new(mode, None);
        let mut events = service.subscribe();
        open(&mut service, &root);
        wait_for(&mut events, |event| {
            matches!(event, ProjectEvent::GameReady)
        });
        let endpoint = service.handle(ProjectRequest::GameEndpoint).unwrap();
        let relay = (mode == ServiceMode::Desktop).then(|| ask_game(&mut service));

        service
            .handle(ProjectRequest::WriteText {
                path: "src/lib.rs".to_string(),
                text: "this is not rust".to_string(),
            })
            .unwrap();
        service.handle(ProjectRequest::Build).unwrap();
        wait_for(&mut events, finished(false));

        assert_eq!(
            service.handle(ProjectRequest::GameEndpoint).unwrap(),
            endpoint,
            "{mode:?}"
        );
        match (&endpoint, relay) {
            (ProjectReply::Wasm { glue_url }, _) => {
                let glue = root
                    .join(".ludenea/build/wasm")
                    .join(glue_url.trim_start_matches("/game/"));
                assert!(glue.is_file(), "{}", glue.display());
            }
            (ProjectReply::Native, Some((to_game, from_game))) => {
                to_game
                    .send(r#"{"type":"request","id":1,"command":{"type":"get_hash"}}"#.to_string())
                    .unwrap();
                let line = from_game.recv_timeout(Duration::from_secs(60)).unwrap();
                assert!(line.contains(r#""type":"response""#), "{line}");
            }
            other => panic!("{other:?}"),
        }
        // No GameExited arrived while waiting: the previous game is still running.
        while let Ok(event) = events.try_recv() {
            assert!(
                !matches!(event, ProjectEvent::GameExited { .. }),
                "{mode:?}"
            );
        }
    }
}
