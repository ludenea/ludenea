use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use ludenea::cli::assets::project_assets;
use ludenea::control::{
    BranchId, Bytes, Command, ConsoleLevel, Document, Event, FieldTarget, Hash64, Host,
    PathSegment, Reply, Timeline, Topic,
};
use ludenea::draw::DebugDrawFrame;
use ludenea::ecs::{AssetId, Entity};
use ludenea::prelude::{App, Sim};
use serde_json::json;

const SPAWNER_SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/spawner.scene.ron"
);
/// The identifier in `crate.prefab.ron.meta`.
const PREFAB: AssetId = AssetId(0x38b3_6be4_f8a1_40d6_95e5_cdac_24c8_2f90);

fn ok(host: &mut Host, command: Command) -> Reply {
    host.handle(command).unwrap()
}

/// A boxes host on the spawner scene, with its prefab and data assets, and the load
/// addresses of its entities.
fn spawner_host() -> (Host, Vec<(Entity, Vec<u32>)>) {
    let mut host = Host::new(boxes::app).unwrap();
    let assets = spawner_assets();
    let Reply::Loaded { load_addresses, .. } = ok(
        &mut host,
        Command::LoadScene {
            scene: fs::read_to_string(SPAWNER_SCENE).unwrap(),
            assets,
            document: Document::Scene,
        },
    ) else {
        panic!("load_scene replies loaded");
    };
    (host, load_addresses)
}

fn spawner_assets() -> Vec<(AssetId, String)> {
    project_assets(Path::new(SPAWNER_SCENE), None)
        .unwrap()
        .texts
        .into_iter()
        .collect()
}

fn field_path(names: &[&str]) -> Vec<PathSegment> {
    names
        .iter()
        .map(|name| PathSegment::Field {
            name: name.to_string(),
        })
        .collect()
}

fn step(host: &mut Host, ticks: u32) {
    ok(
        host,
        Command::Step {
            ticks,
            record_hashes: false,
        },
    );
}

fn seek(host: &mut Host, tick: u64) {
    ok(host, Command::Seek { tick });
}

fn timeline(host: &mut Host) -> Timeline {
    match ok(host, Command::GetTimeline) {
        Reply::Timeline(timeline) => timeline,
        other => panic!("{other:?}"),
    }
}

fn hash(host: &mut Host) -> Hash64 {
    match ok(host, Command::GetHash) {
        Reply::Hash { hash, .. } => hash,
        other => panic!("{other:?}"),
    }
}

fn spawn_crate(host: &mut Host) {
    ok(
        host,
        Command::Spawn {
            components: [("boxes::Crate".to_string(), json!(null))].into(),
        },
    );
}

fn layers(host: &Host) -> Vec<String> {
    host.game_info().debug_layers
}

fn draw(host: &mut Host) -> DebugDrawFrame {
    let layers = layers(host);
    match ok(host, Command::GetDebugDraw { layers }) {
        Reply::DebugDraw { frame, .. } => frame,
        other => panic!("{other:?}"),
    }
}

fn ghost(host: &mut Host, tick: u64) -> (DebugDrawFrame, u64, bool) {
    let layers = layers(host);
    match ok(host, Command::GetGhostDraw { tick, layers }) {
        Reply::GhostDraw {
            frame,
            tick,
            lagging,
        } => (frame, tick, lagging),
        other => panic!("{other:?}"),
    }
}

/// Advances while paused until the ghost stops moving, returning every event.
fn settle(host: &mut Host) -> Vec<Event> {
    let mut events = Vec::new();
    for _ in 0..100 {
        let batch = host.advance(0.0);
        let moved = batch
            .iter()
            .any(|event| matches!(event, Event::GhostAdvanced { .. }));
        events.extend(batch);
        if !moved && !timeline(host).resimulating {
            return events;
        }
    }
    panic!("the ghost never settled: {events:?}");
}

fn errors(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Console {
                level: ConsoleLevel::Error | ConsoleLevel::Warning,
                text,
            } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Moves the entity at load address `address` to `x`.
fn move_to(host: &mut Host, addresses: &[(Entity, Vec<u32>)], address: &[u32], x: f64) {
    let (entity, _) = addresses
        .iter()
        .find(|(_, at)| at == address)
        .expect("an entity at that address");
    ok(
        host,
        Command::SetField {
            target: FieldTarget::Component {
                entity: *entity,
                component: "ludenea::Pose".to_string(),
            },
            path: field_path(&["position", "x"]),
            value: json!(x),
        },
    );
}

/// The root runs to tick 200; the current branch forks at tick 30, moving the first crate,
/// and runs to tick 40.
fn forked_host() -> Host {
    let (mut host, addresses) = spawner_host();
    step(&mut host, 200);
    seek(&mut host, 30);
    move_to(&mut host, &addresses, &[5, 0], -8.0);
    step(&mut host, 10);
    host
}

/// The debug drawing of `branch` at `tick`, switching there and back.
fn draw_of(host: &mut Host, branch: BranchId, tick: u64) -> DebugDrawFrame {
    let back = timeline(host);
    ok(host, Command::SwitchBranch { branch });
    seek(host, tick);
    let frame = draw(host);
    ok(
        host,
        Command::SwitchBranch {
            branch: back.branch,
        },
    );
    seek(host, back.current);
    frame
}

#[test]
fn compare_defaults_to_parent_on_fork() {
    let (mut host, _) = spawner_host();
    step(&mut host, 40);
    assert_eq!(timeline(&mut host).compare, None);
    seek(&mut host, 20);
    spawn_crate(&mut host);
    let forked = timeline(&mut host);
    assert_ne!(forked.branch, BranchId::ROOT);
    assert_eq!(forked.compare, Some(BranchId::ROOT));

    ok(&mut host, Command::SetCompareBranch { branch: None });
    assert_eq!(timeline(&mut host).compare, None);
    let unknown = host
        .handle(Command::SetCompareBranch {
            branch: Some(BranchId(42)),
        })
        .unwrap_err();
    assert!(unknown.message.contains("42"), "{unknown:?}");
    ok(
        &mut host,
        Command::SetCompareBranch {
            branch: Some(forked.branch),
        },
    );
    // The current branch itself: the ghost shows its parent.
    assert_eq!(timeline(&mut host).compare, Some(BranchId::ROOT));
}

/// The root runs to tick 40; branch 1 forks from it at tick 20 and branch 2 from branch 1
/// at tick 10, and is current.
fn three_generations() -> Host {
    let (mut host, _) = spawner_host();
    step(&mut host, 40);
    seek(&mut host, 20);
    spawn_crate(&mut host);
    step(&mut host, 10);
    seek(&mut host, 10);
    spawn_crate(&mut host);
    assert_eq!(timeline(&mut host).branch, BranchId(2));
    host
}

fn switch(host: &mut Host, branch: u32) -> Option<BranchId> {
    ok(
        host,
        Command::SwitchBranch {
            branch: BranchId(branch),
        },
    );
    timeline(host).compare
}

#[test]
fn compare_follows_the_parent_of_the_current_branch() {
    let mut host = three_generations();
    assert_eq!(timeline(&mut host).compare, Some(BranchId(1)));
    assert_eq!(switch(&mut host, 1), Some(BranchId::ROOT));
    assert_eq!(switch(&mut host, 0), None);
    assert_eq!(switch(&mut host, 2), Some(BranchId(1)));
}

#[test]
fn chosen_compare_survives_switches_and_falls_back_while_current() {
    let mut host = three_generations();
    ok(
        &mut host,
        Command::SetCompareBranch {
            branch: Some(BranchId::ROOT),
        },
    );
    assert_eq!(timeline(&mut host).compare, Some(BranchId::ROOT));
    assert_eq!(switch(&mut host, 1), Some(BranchId::ROOT));
    // The chosen branch is current: the ghost shows the parent, here none.
    assert_eq!(switch(&mut host, 0), None);
    assert_eq!(switch(&mut host, 2), Some(BranchId::ROOT));
}

#[test]
fn ghost_draws_compare_branch_at_current_tick() {
    let mut host = forked_host();
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Ghost,
        },
    );
    let events = settle(&mut host);
    assert!(
        events.contains(&Event::GhostAdvanced { tick: 40 }),
        "{events:?}"
    );
    let (frame, tick, lagging) = ghost(&mut host, 40);
    assert_eq!((tick, lagging), (40, false));
    assert_ne!(
        frame,
        draw(&mut host),
        "the root's crate stays where it was"
    );
    assert_eq!(frame, draw_of(&mut host, BranchId::ROOT, 40));
    assert!(errors(&events).is_empty(), "{events:?}");
}

#[test]
fn ghost_reports_lagging_then_catches_up() {
    let mut host = forked_host();
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Ghost,
        },
    );
    // Nothing ran yet: no frame, lagging.
    assert_eq!(ghost(&mut host, 40), (DebugDrawFrame::default(), 40, true));
    settle(&mut host);
    assert!(!ghost(&mut host, 40).2);

    // At speed 8 play runs 32 ticks per advance; the ghost follows 4 of them.
    ok(&mut host, Command::SetSpeed { speed: 8.0 });
    ok(&mut host, Command::Seek { tick: 30 });
    ok(&mut host, Command::Play);
    host.advance(1.0);
    host.advance(1.0);
    ok(&mut host, Command::Pause);
    let current = timeline(&mut host).current;
    assert!(current > 50, "{current}");
    let (frame, tick, lagging) = ghost(&mut host, current);
    assert!(lagging);
    assert!(tick < current, "{tick} < {current}");
    assert!(!frame.layers.is_empty(), "the last frame it has");

    settle(&mut host);
    let (_, tick, lagging) = ghost(&mut host, current);
    assert_eq!((tick, lagging), (current, false));
}

#[test]
fn ghost_never_changes_main_hashes() {
    let run = |ghost: bool| {
        let mut host = forked_host();
        if ghost {
            ok(
                &mut host,
                Command::Subscribe {
                    topic: Topic::Ghost,
                },
            );
        }
        seek(&mut host, 30);
        ok(&mut host, Command::Play);
        let mut hashes = Vec::new();
        let mut events = Vec::new();
        for _ in 0..20 {
            events.extend(host.advance(1.0 / 60.0));
            hashes.push(hash(&mut host));
        }
        ok(&mut host, Command::Pause);
        events.extend(settle(&mut host));
        let advanced = events
            .iter()
            .any(|event| matches!(event, Event::GhostAdvanced { .. }));
        assert_eq!(advanced, ghost, "{events:?}");
        assert!(errors(&events).is_empty(), "{events:?}");
        let Reply::Session {
            hashes: recorded, ..
        } = ok(&mut host, Command::ExportSession)
        else {
            panic!("export_session replies session");
        };
        (hashes, recorded)
    };
    assert_eq!(run(true), run(false));
}

#[test]
fn ghost_outside_range_is_empty() {
    let (mut host, _) = spawner_host();
    step(&mut host, 20);
    seek(&mut host, 10);
    spawn_crate(&mut host);
    step(&mut host, 30);
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Ghost,
        },
    );
    settle(&mut host);
    // The root ends at tick 20.
    assert_eq!(ghost(&mut host, 40), (DebugDrawFrame::default(), 40, false));
    ok(&mut host, Command::Play);
    let mut events = Vec::new();
    for _ in 0..5 {
        events.extend(host.advance(1.0 / 60.0));
    }
    ok(&mut host, Command::Pause);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::GhostAdvanced { .. })),
        "past the end the ghost stays as it is: {events:?}"
    );
    seek(&mut host, 15);
    settle(&mut host);
    let (frame, tick, lagging) = ghost(&mut host, 15);
    assert_eq!((tick, lagging), (15, false));
    assert!(!frame.layers.is_empty());
}

#[test]
fn compare_branch_with_loaded_address_edit_matches_its_recorded_hashes() {
    let (mut host, addresses) = spawner_host();
    step(&mut host, 10);
    // An edit by load address on the root, which the compare simulation resolves.
    move_to(&mut host, &addresses, &[5, 0], -3.0);
    step(&mut host, 30);
    seek(&mut host, 20);
    spawn_crate(&mut host);
    assert_eq!(timeline(&mut host).compare, Some(BranchId::ROOT));
    // From the tick 0 checkpoint, the ghost replays the edit at tick 10.
    seek(&mut host, 12);
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Ghost,
        },
    );
    let events = settle(&mut host);
    assert!(errors(&events).is_empty(), "{events:?}");
    assert_eq!(timeline(&mut host).compare, Some(BranchId::ROOT));
    let (frame, tick, lagging) = ghost(&mut host, 12);
    assert_eq!((tick, lagging), (12, false));
    assert_eq!(frame, draw_of(&mut host, BranchId::ROOT, 12));
}

/// The spawner scene without its spawner: only its instances reach the crate prefab, which
/// a replay's resolved scene no longer reaches.
fn instances_only_replay() -> Bytes {
    let (mut host, _) = spawner_host();
    let scene: String = fs::read_to_string(SPAWNER_SCENE)
        .unwrap()
        .lines()
        .filter(|line| !line.contains("boxes::Spawner"))
        .map(|line| format!("{line}\n"))
        .collect();
    ok(
        &mut host,
        Command::ReplaceScene {
            scene,
            assets: spawner_assets(),
        },
    );
    step(&mut host, 10);
    spawn_crate(&mut host);
    step(&mut host, 30);
    let Reply::Replay { replay } = ok(&mut host, Command::ExportReplay { branch: None }) else {
        panic!("export_replay replies replay");
    };
    replay
}

fn replay_host(replay: Bytes) -> Host {
    let mut host = Host::new(boxes::app).unwrap();
    ok(&mut host, Command::LoadReplay { replay });
    let events = settle(&mut host);
    assert!(!diverged(&events), "{events:?}");
    host
}

fn diverged(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::Diverged { .. }))
}

#[test]
fn compare_branch_of_a_replay_keeps_its_assets() {
    let mut host = replay_host(instances_only_replay());
    seek(&mut host, 20);
    spawn_crate(&mut host);
    seek(&mut host, 5);
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Ghost,
        },
    );
    let events = settle(&mut host);
    assert!(errors(&events).is_empty(), "{events:?}");
    assert_eq!(timeline(&mut host).compare, Some(BranchId::ROOT));
    assert_eq!(ghost(&mut host, 5).1, 5);
    assert!(!ghost(&mut host, 5).2);
}

/// Moves the ground at tick 0, a scene edit that reloads the scene.
fn move_ground(host: &mut Host) {
    seek(host, 0);
    let ground = match ok(
        host,
        Command::QueryEntities {
            filter: "With<ludenea::Body>".to_string(),
            offset: 0,
            limit: 1,
        },
    ) {
        Reply::Entities { rows, .. } => rows[0].entity,
        other => panic!("{other:?}"),
    };
    ok(
        host,
        Command::SetField {
            target: FieldTarget::Component {
                entity: ground,
                component: "ludenea::Pose".to_string(),
            },
            path: field_path(&["position", "y"]),
            value: json!(-1.0),
        },
    );
}

#[test]
fn exported_replay_session_keeps_its_assets_on_resume() {
    let mut opened = replay_host(instances_only_replay());
    let Reply::Session {
        scene,
        saved_scene,
        inputs,
        edits,
        undo,
        hashes,
        assets,
        saved_assets,
        branches,
        document,
        keeps_assets,
    } = ok(&mut opened, Command::ExportSession)
    else {
        panic!("export_session replies session");
    };
    assert!(keeps_assets);
    let mut resumed = Host::new(boxes::app).unwrap();
    ok(
        &mut resumed,
        Command::LoadSession {
            scene,
            saved_scene,
            inputs,
            edits,
            undo,
            expected: hashes,
            assets,
            saved_assets,
            branches,
            document,
            keeps_assets,
        },
    );
    let events = settle(&mut resumed);
    assert!(!diverged(&events), "{events:?}");

    move_ground(&mut opened);
    move_ground(&mut resumed);
    assert_eq!(hash(&mut resumed), hash(&mut opened));
    assert!(
        resumed.handle(Command::GetAsset { id: PREFAB }).is_ok(),
        "the prefab stays loaded"
    );
}

/// Whether `panic_when_armed` panics.
static ARMED: AtomicBool = AtomicBool::new(false);

fn panic_when_armed() {
    if ARMED.load(Ordering::Relaxed) {
        panic!("boom in the ghost");
    }
}

fn armed_app() -> App {
    let mut app = boxes::app();
    app.system(Sim, panic_when_armed);
    app
}

#[test]
fn a_panic_in_the_compare_simulation_stops_the_ghost_only() {
    let mut host = Host::new(armed_app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(10, 3),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    step(&mut host, 40);
    seek(&mut host, 20);
    spawn_crate(&mut host);
    let before = (timeline(&mut host).current, hash(&mut host));
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Ghost,
        },
    );
    // Paused, only the compare simulation runs game code.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    ARMED.store(true, Ordering::Relaxed);
    let events = host.advance(0.0);
    ARMED.store(false, Ordering::Relaxed);
    std::panic::set_hook(hook);
    let errors = errors(&events);
    assert!(
        matches!(&errors[..], [text] if text.contains("boom in the ghost")),
        "{events:?}"
    );
    assert_eq!(timeline(&mut host).compare, None);
    assert_eq!((timeline(&mut host).current, hash(&mut host)), before);
    assert_eq!(ghost(&mut host, 20), (DebugDrawFrame::default(), 20, false));
    // The main session goes on.
    step(&mut host, 5);
}

/// Whether `draw_panics_when_armed` panics.
static DRAW_ARMED: AtomicBool = AtomicBool::new(false);

fn draw_panics_when_armed(
    _: &ludenea::ecs::World,
    _: &std::collections::BTreeSet<String>,
    _: &mut DebugDrawFrame,
) {
    if DRAW_ARMED.load(Ordering::Relaxed) {
        panic!("boom in the drawer");
    }
}

fn draw_armed_app() -> App {
    let mut app = boxes::app();
    app.debug_draw(&["boom"], draw_panics_when_armed);
    app
}

#[test]
fn a_panicking_debug_drawer_fails_the_draw_and_stops_the_ghost_only() {
    let mut host = Host::new(draw_armed_app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(10, 3),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    step(&mut host, 40);
    seek(&mut host, 20);
    spawn_crate(&mut host);
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Ghost,
        },
    );
    settle(&mut host);
    let layers = layers(&host);

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    DRAW_ARMED.store(true, Ordering::Relaxed);
    let drawn = host.handle(Command::GetDebugDraw {
        layers: layers.clone(),
    });
    let ghost_drawn = host.handle(Command::GetGhostDraw { tick: 20, layers });
    DRAW_ARMED.store(false, Ordering::Relaxed);
    std::panic::set_hook(hook);

    for result in [drawn, ghost_drawn] {
        let error = result.unwrap_err();
        assert_eq!(error.code, ludenea::control::ErrorCode::SystemPanicked);
        assert!(error.message.contains("boom in the drawer"), "{error:?}");
    }
    assert_eq!(timeline(&mut host).compare, None);
    // The session goes on.
    step(&mut host, 5);
    draw(&mut host);
}

fn pick_panics(_: &ludenea::ecs::World, _: ludenea::prelude::Vec2) -> Vec<Entity> {
    panic!("boom in the picker");
}

fn pick_panics_app() -> App {
    let mut app = boxes::app();
    app.picker(pick_panics);
    app
}

#[test]
fn a_panicking_picker_fails_the_pick_and_keeps_the_session() {
    let mut host = Host::new(pick_panics_app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(10, 3),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    step(&mut host, 5);

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let picked = host.handle(Command::Pick {
        point: serde_json::from_value(json!({ "x": 0.0, "y": 0.0 })).unwrap(),
    });
    std::panic::set_hook(hook);

    let error = picked.unwrap_err();
    assert_eq!(error.code, ludenea::control::ErrorCode::SystemPanicked);
    assert!(error.message.contains("boom in the picker"), "{error:?}");
    // The session goes on.
    step(&mut host, 5);
    draw(&mut host);
}

thread_local! {
    /// Whether `Brittle` panics as it loads on this thread: each test arms its own.
    static BRITTLE_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Loads while `BRITTLE_ARMED` is off and panics while it is on, as a `try_from` that asserts.
#[derive(ludenea_ecs::Component, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "BrittleValue")]
struct Brittle(u32);

/// What `Brittle` loads from, of the same shape.
#[derive(serde::Deserialize)]
struct BrittleValue(u32);

impl TryFrom<BrittleValue> for Brittle {
    type Error = String;

    fn try_from(value: BrittleValue) -> Result<Self, String> {
        assert!(!BRITTLE_ARMED.get(), "the brittle value broke");
        Ok(Brittle(value.0))
    }
}

/// A host whose compare branch holds a `Brittle` value in its checkpoint at tick 16, at
/// tick 20 on a branch forked there.
fn brittle_host() -> Host {
    let app = || {
        let mut app = boxes::app();
        app.component::<Brittle>();
        app
    };
    let mut host = Host::new(app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(10, 3),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    ok(
        &mut host,
        Command::Spawn {
            components: [("compare::Brittle".to_string(), json!(1))].into(),
        },
    );
    step(&mut host, 40);
    seek(&mut host, 20);
    spawn_crate(&mut host);
    host.drain_events();
    host
}

#[test]
fn a_panic_restoring_the_compare_simulation_stops_the_ghost_only() {
    let mut host = brittle_host();
    let before = (timeline(&mut host).current, hash(&mut host));
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Ghost,
        },
    );
    // Paused, only the compare simulation restores a checkpoint.
    BRITTLE_ARMED.set(true);
    let events = host.advance(0.0);
    BRITTLE_ARMED.set(false);

    assert!(
        matches!(&errors(&events)[..], [text] if text.starts_with("the ghost stopped: the game panicked at tick ")
            && text.contains("the brittle value broke")),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Paused { .. })),
        "{events:?}"
    );
    assert_eq!(timeline(&mut host).compare, None);
    assert_eq!((timeline(&mut host).current, hash(&mut host)), before);
    assert!(errors(&host.advance(0.0)).is_empty());
    step(&mut host, 5);
}

#[test]
fn a_panic_restoring_the_divergence_simulations_stops_the_report_only() {
    let mut host = brittle_host();
    let before = (timeline(&mut host).current, hash(&mut host));
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Divergence,
        },
    );
    BRITTLE_ARMED.set(true);
    let events = host.advance(0.0);
    BRITTLE_ARMED.set(false);

    assert!(
        matches!(&errors(&events)[..], [text] if text.starts_with("the divergence report stopped: the game panicked at tick ")
            && text.contains("the brittle value broke")),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Paused { .. })),
        "{events:?}"
    );
    assert_eq!((timeline(&mut host).current, hash(&mut host)), before);
    step(&mut host, 5);
}

#[test]
fn a_divergence_report_that_keeps_failing_stops_until_the_runs_change() {
    let mut host = brittle_host();
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Divergence,
        },
    );
    BRITTLE_ARMED.set(true);
    let first = host.advance(0.0);
    let second = host.advance(0.0);
    BRITTLE_ARMED.set(false);

    assert_eq!(errors(&first).len(), 1, "{first:?}");
    assert!(errors(&second).is_empty(), "{second:?}");
    // A change to the runs tries again.
    spawn_crate(&mut host);
    host.drain_events();
    BRITTLE_ARMED.set(true);
    let events = host.advance(0.0);
    BRITTLE_ARMED.set(false);
    assert!(
        matches!(&errors(&events)[..], [text] if text.starts_with("the divergence report stopped: ")),
        "{events:?}"
    );
}
