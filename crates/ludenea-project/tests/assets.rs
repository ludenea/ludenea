//! Prefabs, data assets, binary files, and stored sessions through the project service.
//! `template_project_loads_with_prefab_and_data_asset` runs a real `cargo build`; run it with
//! `cargo test -p ludenea-project --test assets -- --ignored`.

use std::fs;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use ludenea_control::{
    Bytes, ClientMessage, Command, Document, Host, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, Reply,
    ServerMessage,
};
use ludenea_ecs::AssetId;
use ludenea_project::protocol::{
    AssetTexts, FailureCode, ProjectReply, ProjectRequest, ServiceMode,
};
use ludenea_project::service::ProjectService;
use ludenea_project::{AssetKind, BuildArtifact, BuildTarget, FileEntry, GameProcess, Project};
use tempfile::TempDir;

const BOXES_ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/boxes/assets");
/// The identifier in `examples/boxes/assets/spawner.ron.meta`.
const STATS: AssetId = AssetId(0xbef6_17c9_e94d_41c4_b0e0_7882_849c_ef18);

/// A project named `demo` without the engine, so its background build fails fast, holding
/// `files`.
fn project(dir: &TempDir, files: &[(&str, &str)]) -> ProjectService {
    let root = dir.path().join("demo");
    let manifest = [("ludenea.toml", "name = \"demo\"\nengine = \"0.1.0\"\n")];
    for (path, text) in manifest.iter().chain(files) {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let mut service = ProjectService::new(ServiceMode::Browser, None);
    let path = root.to_string_lossy().into_owned();
    service.handle(ProjectRequest::Open { path }).unwrap();
    service
}

fn meta(id: u128, kind: &str) -> String {
    format!("Meta(version: 1, id: \"{id:032x}\", kind: {kind})\n")
}

fn asset_texts(service: &mut ProjectService) -> Result<AssetTexts, String> {
    match service.handle(ProjectRequest::ReadAssetTexts) {
        Ok(ProjectReply::AssetTexts(texts)) => Ok(texts),
        Ok(other) => panic!("{other:?}"),
        Err(failure) => Err(failure.message),
    }
}

#[test]
fn create_asset_writes_file_and_meta() {
    let dir = TempDir::new().unwrap();
    let mut service = project(&dir, &[]);
    let kind = AssetKind::Data {
        r#type: "demo::Settings".to_string(),
    };
    let reply = service.handle(ProjectRequest::CreateAsset {
        path: "assets/settings.ron".to_string(),
        kind: kind.clone(),
        text: "(gravity_scale: 1.0)\n".to_string(),
    });
    let Ok(ProjectReply::File(entry)) = reply else {
        panic!("{reply:?}");
    };
    assert_eq!(entry.path, "assets/settings.ron");
    assert_eq!(entry.kind, kind);
    let root = dir.path().join("demo");
    assert_eq!(
        fs::read_to_string(root.join("assets/settings.ron")).unwrap(),
        "(gravity_scale: 1.0)\n"
    );
    let meta = fs::read_to_string(root.join("assets/settings.ron.meta")).unwrap();
    assert!(meta.contains(&entry.id.to_string()), "{meta}");
    assert!(meta.contains("Data(type:\"demo::Settings\")"), "{meta}");
    // Listed with the kind its meta gives, not the one its extension would.
    let ProjectReply::Files { entries } = service.handle(ProjectRequest::ListFiles).unwrap() else {
        panic!("ListFiles answers Files");
    };
    assert_eq!(entries, std::slice::from_ref(&entry));
    // An existing file is never replaced.
    let again = service
        .handle(ProjectRequest::CreateAsset {
            path: "assets/settings.ron".to_string(),
            kind,
            text: "()".to_string(),
        })
        .unwrap_err();
    assert_eq!(again.code, FailureCode::AlreadyExists);
    let ProjectReply::Files { entries } = service.handle(ProjectRequest::ListFiles).unwrap() else {
        panic!("ListFiles answers Files");
    };
    assert_eq!(entries, [entry]);
}

#[test]
fn create_asset_refuses_a_kind_its_extension_contradicts() {
    let dir = TempDir::new().unwrap();
    let mut service = project(&dir, &[]);
    let data = AssetKind::Data {
        r#type: "demo::Settings".to_string(),
    };
    for (path, kind) in [
        ("assets/settings.ron", AssetKind::Prefab),
        ("assets/ball.prefab.ron", data.clone()),
        ("assets/settings.txt", data),
        ("assets/level.ron", AssetKind::Scene),
    ] {
        let failure = service
            .handle(ProjectRequest::CreateAsset {
                path: path.to_string(),
                kind,
                text: "()".to_string(),
            })
            .unwrap_err();
        assert_eq!(failure.code, FailureCode::InvalidRequest, "{path}");
    }
    assert!(
        !dir.path().join("demo/assets").exists(),
        "nothing is written"
    );
}

/// An existing file or meta is refused and left as it was, and nothing else is written.
#[test]
fn create_asset_refuses_an_existing_file_or_meta() {
    let dir = TempDir::new().unwrap();
    let mut service = project(
        &dir,
        &[
            ("assets/a.prefab.ron", "mine"),
            ("assets/b.prefab.ron.meta", &meta(7, "Prefab")),
        ],
    );
    let root = dir.path().join("demo");
    for path in ["assets/a.prefab.ron", "assets/b.prefab.ron"] {
        let failure = service
            .handle(ProjectRequest::CreateAsset {
                path: path.to_string(),
                kind: AssetKind::Prefab,
                text: "Prefab(version: 1)".to_string(),
            })
            .unwrap_err();
        assert_eq!(failure.code, FailureCode::AlreadyExists, "{path}");
    }
    assert_eq!(
        fs::read_to_string(root.join("assets/a.prefab.ron")).unwrap(),
        "mine"
    );
    assert!(!root.join("assets/a.prefab.ron.meta").exists());
    assert!(!root.join("assets/b.prefab.ron").exists());
    assert_eq!(
        fs::read_to_string(root.join("assets/b.prefab.ron.meta")).unwrap(),
        meta(7, "Prefab")
    );
}

#[test]
fn asset_texts_wrap_data_assets_with_their_type_in_path_order() {
    let dir = TempDir::new().unwrap();
    let mut service = project(
        &dir,
        &[
            ("assets/b.prefab.ron", "Prefab(version: 1)"),
            ("assets/b.prefab.ron.meta", &meta(1, "Prefab")),
            ("assets/a/stats.ron", "(damage: 3)"),
            (
                "assets/a/stats.ron.meta",
                &meta(2, "Data(type: \"demo::Stats\")"),
            ),
            // Neither a prefab nor a data asset.
            ("assets/plain.ron", "()"),
            ("assets/plain.ron.meta", &meta(3, "Other")),
            ("scenes/main.scene.ron", "(entities: [])"),
            ("scenes/main.scene.ron.meta", &meta(4, "Scene")),
            // Not under `scenes/` or `assets/`.
            ("other/far.prefab.ron", "Prefab(version: 1)"),
            ("other/far.prefab.ron.meta", &meta(5, "Prefab")),
            // No meta yet.
            ("assets/new.prefab.ron", "Prefab(version: 1)"),
        ],
    );
    let texts = asset_texts(&mut service).unwrap();
    assert_eq!(
        texts.assets,
        [
            (
                AssetId(2),
                "AssetFile(type: \"demo::Stats\", value: (damage: 3))".to_string()
            ),
            (AssetId(1), "Prefab(version: 1)".to_string()),
        ]
    );
    assert_eq!(
        texts.paths,
        [
            (AssetId(2), "assets/a/stats.ron".to_string()),
            (AssetId(1), "assets/b.prefab.ron".to_string()),
        ]
    );
}

#[test]
fn an_unreadable_meta_leaves_out_only_its_own_asset() {
    let dir = TempDir::new().unwrap();
    let mut service = project(
        &dir,
        &[
            ("assets/good.prefab.ron", "Prefab(version: 1)"),
            ("assets/good.prefab.ron.meta", &meta(1, "Prefab")),
            ("assets/broken.prefab.ron", "Prefab(version: 1)"),
            ("assets/broken.prefab.ron.meta", "Meta("),
        ],
    );
    let texts = asset_texts(&mut service).unwrap();
    assert_eq!(
        texts.assets,
        [(AssetId(1), "Prefab(version: 1)".to_string())]
    );
}

#[test]
fn duplicate_identifiers_are_refused_naming_both_files() {
    let dir = TempDir::new().unwrap();
    let mut service = project(
        &dir,
        &[
            ("assets/a.prefab.ron", "Prefab(version: 1)"),
            ("assets/a.prefab.ron.meta", &meta(9, "Prefab")),
            ("scenes/b.prefab.ron", "Prefab(version: 1)"),
            ("scenes/b.prefab.ron.meta", &meta(9, "Prefab")),
        ],
    );
    let error = asset_texts(&mut service).unwrap_err();
    assert!(
        error.contains("assets/a.prefab.ron") && error.contains("scenes/b.prefab.ron"),
        "{error}"
    );
}

#[test]
fn stored_session_round_trips() {
    let dir = TempDir::new().unwrap();
    let mut service = project(&dir, &[]);
    let document = AssetId(0x2a);
    let load = |service: &mut ProjectService| match service
        .handle(ProjectRequest::LoadStoredSession { document })
        .unwrap()
    {
        ProjectReply::StoredSession { bytes } => bytes,
        other => panic!("{other:?}"),
    };
    assert_eq!(load(&mut service), None);
    let bytes = Bytes(vec![0, 1, 2, 255]);
    service
        .handle(ProjectRequest::StoreSession {
            document,
            bytes: bytes.clone(),
        })
        .unwrap();
    assert_eq!(load(&mut service), Some(bytes.clone()));
    assert!(
        dir.path()
            .join(format!("demo/.ludenea/sessions/{document}.session"))
            .is_file()
    );
    // Binary files travel the same way.
    service
        .handle(ProjectRequest::WriteBinary {
            path: "runs/a.replay".to_string(),
            bytes: bytes.clone(),
        })
        .unwrap();
    let read = service.handle(ProjectRequest::ReadBinary {
        path: "runs/a.replay".to_string(),
    });
    assert_eq!(read, Ok(ProjectReply::Bytes { bytes }));
}

#[test]
fn a_stored_session_set_aside_moves_to_the_first_free_rejected_file() {
    let dir = TempDir::new().unwrap();
    let mut service = project(&dir, &[]);
    let document = AssetId(0x2a);
    let set_aside = |service: &mut ProjectService| match service
        .handle(ProjectRequest::SetAsideStoredSession { document })
        .unwrap()
    {
        ProjectReply::SetAside { path } => path,
        other => panic!("{other:?}"),
    };
    let store = |service: &mut ProjectService, bytes: &[u8]| {
        service
            .handle(ProjectRequest::StoreSession {
                document,
                bytes: Bytes(bytes.to_vec()),
            })
            .unwrap()
    };
    assert_eq!(set_aside(&mut service), None);

    store(&mut service, b"first");
    let first = format!(".ludenea/sessions/{document}.1.rejected.session");
    assert_eq!(set_aside(&mut service), Some(first.clone()));
    assert_eq!(
        service.handle(ProjectRequest::LoadStoredSession { document }),
        Ok(ProjectReply::StoredSession { bytes: None })
    );
    // A later session is stored, then set aside next to the first, which it never replaces.
    store(&mut service, b"second");
    let second = format!(".ludenea/sessions/{document}.2.rejected.session");
    assert_eq!(set_aside(&mut service), Some(second.clone()));

    let read = |path: &str| fs::read(dir.path().join("demo").join(path)).unwrap();
    assert_eq!(read(&first), b"first");
    assert_eq!(read(&second), b"second");
    assert!(
        !dir.path()
            .join(format!("demo/.ludenea/sessions/{document}.session"))
            .exists()
    );
}

/// A set-aside copy that cannot be written, here into a sessions folder that refuses new
/// files, is an error that names it, and the stored session stays where it was.
#[cfg(unix)]
#[test]
fn a_set_aside_that_cannot_write_its_copy_keeps_the_stored_session() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let mut service = project(&dir, &[]);
    let document = AssetId(0x2a);
    service
        .handle(ProjectRequest::StoreSession {
            document,
            bytes: Bytes(b"kept".to_vec()),
        })
        .unwrap();
    let sessions = dir.path().join("demo/.ludenea/sessions");
    fs::set_permissions(&sessions, fs::Permissions::from_mode(0o555)).unwrap();

    let result = service.handle(ProjectRequest::SetAsideStoredSession { document });
    fs::set_permissions(&sessions, fs::Permissions::from_mode(0o755)).unwrap();

    let failure = result.unwrap_err();
    assert_eq!(failure.code, FailureCode::Io, "{failure:?}");
    assert!(
        failure
            .message
            .contains(&format!("{document}.1.rejected.session")),
        "{failure:?}"
    );
    assert_eq!(
        service.handle(ProjectRequest::LoadStoredSession { document }),
        Ok(ProjectReply::StoredSession {
            bytes: Some(Bytes(b"kept".to_vec()))
        })
    );
    let names: Vec<_> = fs::read_dir(&sessions)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [format!("{document}.session")]);
}

/// Review focus: a data asset broken on disk during a session is refused when the scene is
/// reloaded with it, with the asset's path, line, and column, and the session keeps the
/// previous value.
#[test]
fn broken_data_asset_is_refused_and_previous_value_kept() {
    let read = |name: &str| fs::read_to_string(Path::new(BOXES_ASSETS).join(name)).unwrap();
    let files: Vec<(String, String)> = [
        "spawner.scene.ron",
        "spawner.scene.ron.meta",
        "crate.prefab.ron",
        "crate.prefab.ron.meta",
        "spawner.ron",
        "spawner.ron.meta",
    ]
    .into_iter()
    .map(|name| (format!("assets/{name}"), read(name)))
    .collect();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let dir = TempDir::new().unwrap();
    let mut service = project(&dir, &files);
    let scene = read("spawner.scene.ron");
    let texts = asset_texts(&mut service).unwrap();
    let mut host = Host::new(boxes::app).unwrap();
    host.handle(Command::LoadScene {
        scene: scene.clone(),
        assets: texts.assets,
        document: Document::Scene,
    })
    .unwrap();
    let stats = |host: &mut Host| match host.handle(Command::GetAsset { id: STATS }).unwrap() {
        Reply::Asset { value, .. } => value,
        other => panic!("{other:?}"),
    };
    let before = stats(&mut host);

    let broken = [
        // A value of the wrong type, on the second line.
        (
            "(\n    every_ticks: \"often\",\n    impulse: (x: 4.0, y: 2.0),\n)\n",
            "2:17-2:18: Expected integer",
        ),
        // Not RON at all.
        (
            "(\n    every_ticks: 60,\n    impulse: (x: 4.0 y: 2.0),\n)\n",
            "3:22: Expected comma",
        ),
        // On one line, the common case: columns count from the file's own first character.
        (
            "(every_ticks: \"often\", impulse: (x: 4.0, y: 2.0))\n",
            "1:14-1:15: Expected integer",
        ),
        (
            "(every_ticks: 60 impulse: (x: 4.0, y: 2.0))\n",
            "1:18: Expected comma",
        ),
        // Columns count characters, so a non-ASCII one before the error counts once.
        (
            "(/* é */ every_ticks: 60 impulse: (x: 4.0, y: 2.0))\n",
            "1:26: Expected comma",
        ),
    ];
    for (text, position) in broken {
        fs::write(dir.path().join("demo/assets/spawner.ron"), text).unwrap();
        let texts = asset_texts(&mut service).unwrap();
        let error = host
            .handle(Command::ReplaceScene {
                scene: scene.clone(),
                assets: texts.assets,
            })
            .unwrap_err();
        // The game names the asset by identifier; the project names its file.
        let path = texts
            .paths
            .iter()
            .find(|(id, _)| error.message.contains(&id.to_string()))
            .map(|(_, path)| path.as_str());
        assert_eq!(path, Some("assets/spawner.ron"), "{}", error.message);
        assert!(
            error.message.contains(&format!(" is invalid: {position}")),
            "{}",
            error.message
        );
        assert_eq!(stats(&mut host), before);
    }
}

/// The template's game, built natively, loads its scene with the ball prefab placed twice
/// and the settings data asset.
#[test]
#[ignore]
fn template_project_loads_with_prefab_and_data_asset() {
    let dir = TempDir::new().unwrap();
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = Project::create(dir.path(), "demo", &engine).unwrap();
    let kinds: Vec<(String, AssetKind)> = project
        .files()
        .into_iter()
        .map(|FileEntry { path, kind, .. }| (path, kind))
        .collect();
    assert!(kinds.contains(&("assets/ball.prefab.ron".to_string(), AssetKind::Prefab)));
    let settings = AssetKind::Data {
        r#type: "demo::Settings".to_string(),
    };
    assert!(kinds.contains(&("assets/settings.ron".to_string(), settings)));
    let (sender, _events) = std::sync::mpsc::channel();
    let built = project.build(BuildTarget::Native, sender).join().unwrap();
    let Ok(BuildArtifact::Native { binary }) = built else {
        panic!("native build failed: {built:?}");
    };
    let mut service = ProjectService::new(ServiceMode::Browser, None);
    let path = project.root().to_string_lossy().into_owned();
    service.handle(ProjectRequest::Open { path }).unwrap();
    let texts = asset_texts(&mut service).unwrap();
    assert_eq!(texts.assets.len(), 2);

    let mut game = GameProcess::start(&binary).unwrap();
    let messages = game.take_messages().unwrap();
    let mut ask = |message: String| -> String {
        game.send(&message).unwrap();
        receive(&messages)
    };
    ask(serde_json::to_string(&ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        protocol_fingerprint: PROTOCOL_FINGERPRINT,
    })
    .unwrap());
    let load = serde_json::json!({
        "type": "request",
        "id": 1,
        "command": {
            "type": "load_scene",
            "scene": project.read_text("scenes/main.scene.ron").unwrap(),
            "assets": texts.assets,
            "document": "Scene",
        },
    });
    let reply = ask(load.to_string());
    let ServerMessage::Response { result, .. } = serde_json::from_str(&reply).unwrap() else {
        panic!("{reply}");
    };
    let Ok(Reply::Loaded { instances, .. }) = result else {
        panic!("{reply}");
    };
    assert_eq!(instances.len(), 2);
    game.stop();
}

/// The next reply of the game; the events it pushes beside replies (a load announces the new
/// state with `ticked`) are not replies.
fn receive(messages: &Receiver<String>) -> String {
    loop {
        let line = messages
            .recv_timeout(Duration::from_secs(60))
            .expect("the game answers");
        let message: serde_json::Value = serde_json::from_str(&line).expect("the game speaks JSON");
        if message["type"] != "event" {
            return line;
        }
    }
}
