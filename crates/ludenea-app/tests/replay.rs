use ludenea_app::edit::{Edit, EditLog, EntityRef, TimedEdit};
use ludenea_app::replay::{
    REPLAY_CHECKPOINT_INTERVAL, REPLAY_VERSION, Replay, ReplayError, ReplayOutcome, play_replay,
    record_replay,
};
use ludenea_app::{AssetTexts, InputLog};
use ludenea_ecs::{AssetId, Snapshot};

const A: u128 = 0xa;
const B: u128 = 0xb;
const C: u128 = 0xc;

/// The game the replays are recorded with.
mod game {
    use ludenea_app::{App, Inputs, Sim, Simulation};
    use ludenea_ecs::schema::Schema;
    use ludenea_ecs::{Asset, Component, Handle, Query, Runtime};
    use serde::{Deserialize, Serialize};

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
    #[component(name = "replay::Counter")]
    pub struct Counter {
        pub value: u32,
    }

    #[derive(Asset, Serialize, Deserialize, Clone, Debug, PartialEq)]
    #[asset(name = "replay::Stats")]
    pub struct Stats {
        pub damage: u32,
        pub next: Option<Handle<Stats>>,
    }

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
    #[component(name = "replay::Armed")]
    pub struct Armed(pub Handle<Stats>);

    #[derive(Schema, Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
    pub enum Command {
        #[default]
        Idle,
        Jump,
    }

    fn count(inputs: Runtime<Inputs<Command>>, mut counters: Query<&mut Counter>) {
        let step = match inputs.player(0) {
            Command::Idle => 1,
            Command::Jump => 5,
        };
        for counter in counters.iter_mut() {
            counter.value += step;
        }
    }

    pub fn simulation() -> Simulation {
        let mut app = App::new();
        app.input::<Command>()
            .component::<Counter>()
            .component::<Armed>()
            .asset::<Stats>()
            .system(Sim, count);
        app.build().unwrap()
    }
}

/// The game after `Counter` gained an optional field.
mod added_field {
    use ludenea_app::{App, Inputs, Sim, Simulation};
    use ludenea_ecs::{Asset, Component, Handle, Query, Runtime};
    use serde::{Deserialize, Serialize};

    use super::game::Command;

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
    #[component(name = "replay::Counter")]
    pub struct Counter {
        pub value: u32,
        pub bonus: Option<u32>,
    }

    #[derive(Asset, Serialize, Deserialize, Clone, Debug, PartialEq)]
    #[asset(name = "replay::Stats")]
    pub struct Stats {
        pub damage: u32,
        pub next: Option<Handle<Stats>>,
    }

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
    #[component(name = "replay::Armed")]
    pub struct Armed(pub Handle<Stats>);

    fn count(inputs: Runtime<Inputs<Command>>, mut counters: Query<&mut Counter>) {
        let step = match inputs.player(0) {
            Command::Idle => 1,
            Command::Jump => 5,
        };
        for counter in counters.iter_mut() {
            counter.value += step;
        }
    }

    pub fn simulation() -> Simulation {
        let mut app = App::new();
        app.input::<Command>()
            .component::<Counter>()
            .component::<Armed>()
            .asset::<Stats>()
            .system(Sim, count);
        app.build().unwrap()
    }
}

/// The game after the input type lost its `Jump` variant.
mod changed_input {
    use ludenea_app::{App, Simulation};
    use ludenea_ecs::schema::Schema;
    use serde::{Deserialize, Serialize};

    pub use super::game::{Armed, Counter, Stats};

    #[derive(Schema, Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
    pub enum Command {
        #[default]
        Idle,
    }

    pub fn simulation() -> Simulation {
        let mut app = App::new();
        app.input::<Command>()
            .component::<Counter>()
            .component::<Armed>()
            .asset::<Stats>();
        app.build().unwrap()
    }
}

use game::{Command, Counter};

fn counter(id: u32, value: u32) -> String {
    format!("(id: {id}, components: {{\"replay::Counter\": (value: {value})}}),")
}

fn scene(entities: &str, instances: &str) -> String {
    format!(
        "Scene(version: 2, next_id: 10, resources: {{}}, entities: [{entities}], instances: [{instances}])"
    )
}

fn prefab(entities: &str) -> String {
    format!("Prefab(version: 1, next_id: 10, entities: [{entities}], instances: [])")
}

fn instance(id: u32, prefab: u128) -> String {
    format!(
        "(id: {id}, prefab: \"{}\", overrides: [], added: [], removed: []),",
        AssetId(prefab)
    )
}

fn texts(assets: &[(u128, String)]) -> AssetTexts {
    assets
        .iter()
        .map(|(id, text)| (AssetId(*id), text.clone()))
        .collect()
}

/// `Idle` every tick but a `Jump` every 7th.
fn inputs(ticks: u64) -> InputLog {
    let mut log = InputLog::new(1);
    for tick in 0..ticks {
        let command = if tick % 7 == 3 {
            Command::Jump
        } else {
            Command::Idle
        };
        log.record(&[command]);
    }
    log
}

/// A replay of a scene with one counter and a prefab instance holding another.
fn recorded(ticks: u64, edits: &EditLog) -> Replay {
    let assets = texts(&[(A, prefab(&counter(1, 10)))]);
    record_replay(
        &mut game::simulation(),
        &scene(&counter(3, 0), &instance(5, A)),
        &assets,
        &inputs(ticks),
        edits,
        ticks,
    )
    .unwrap()
}

fn play(replay: &Replay) -> ludenea_app::replay::ReplayReport {
    play_replay(&mut game::simulation(), replay, |_, _| {}).unwrap()
}

fn counter_bytes(value: u32) -> Vec<u8> {
    postcard::to_allocvec(&Counter { value }).unwrap()
}

#[test]
fn replay_round_trips() {
    let replay = recorded(20, &EditLog::new());
    assert_eq!(replay.version, REPLAY_VERSION);
    assert_eq!(replay.engine, env!("CARGO_PKG_VERSION"));
    assert_eq!(replay.hashes.len(), 21);
    assert_eq!(Replay::decode(&replay.encode()).unwrap(), replay);

    let mut other = replay.clone();
    other.version = 7;
    let error = Replay::decode(&other.encode()).unwrap_err();
    assert!(matches!(
        error,
        ReplayError::Version {
            found: 7,
            expected: REPLAY_VERSION
        }
    ));
}

#[test]
fn recorded_replay_plays_back_matched() {
    let replay = recorded(60, &EditLog::new());
    let mut ticks = Vec::new();
    let mut sim = game::simulation();
    let report = play_replay(&mut sim, &replay, |tick, total| ticks.push((tick, total))).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    assert!(!report.fingerprint_differs);
    assert_eq!(ticks.len(), 60);
    assert_eq!(ticks.last(), Some(&(60, 60)));
    assert_eq!(sim.hash(), replay.hashes[60]);
    assert_eq!(replay.fingerprint, sim.schema_fingerprint());
    assert_eq!(sim.snapshot().fingerprint, sim.schema_fingerprint());
}

#[test]
fn replay_with_instance_edit_plays_back() {
    let edits = EditLog {
        edits: vec![TimedEdit {
            tick: 50,
            edit: Edit::Insert {
                entity: EntityRef::Loaded(vec![5, 1]),
                component: "replay::Counter".to_string(),
                value: counter_bytes(1000),
            },
        }],
        ..EditLog::new()
    };
    let replay = recorded(80, &edits);
    let without_edit = recorded(80, &EditLog::new());
    assert_eq!(replay.hashes[50], without_edit.hashes[50]);
    assert_ne!(replay.hashes[51], without_edit.hashes[51]);

    let recorded_edits = EditLog::decode(&replay.edits).unwrap();
    let mut sim = game::simulation();
    sim.load_scene_with(
        &scene(&counter(3, 0), &instance(5, A)),
        &texts(&[(A, prefab(&counter(1, 10)))]),
    )
    .unwrap();
    let inside = sim.entity_at_address(&[5, 1]).unwrap();
    assert!(matches!(
        &recorded_edits.edits[0].edit,
        Edit::Insert { entity: EntityRef::Runtime(entity), .. } if *entity == inside
    ));

    assert_eq!(play(&replay).outcome, ReplayOutcome::Matched);
}

#[test]
fn replay_tick_0_assets_include_instance_prefabs() {
    let assets = texts(&[
        (A, prefab(&counter(1, 10))),
        (B, prefab(&format!("{}{}", counter(1, 20), counter(2, 30)))),
    ]);
    let replay = record_replay(
        &mut game::simulation(),
        &scene("", &format!("{}{}", instance(4, A), instance(6, B))),
        &assets,
        &inputs(30),
        &EditLog::new(),
        30,
    )
    .unwrap();
    let ids: Vec<AssetId> = replay.assets.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![AssetId(A), AssetId(B)]);
    assert!(!replay.scene.contains("instances"));
    assert_eq!(play(&replay).outcome, ReplayOutcome::Matched);
}

#[test]
fn asset_loaded_by_a_later_edit_is_not_in_tick_0_assets() {
    let stats = |id: u128, damage: u32| {
        (
            id,
            format!("AssetFile(type: \"replay::Stats\", value: (damage: {damage}, next: None))"),
        )
    };
    let armed = format!(
        "(id: 2, components: {{\"replay::Armed\": (\"{}\")}}),",
        AssetId(B)
    );
    let edits = EditLog {
        edits: vec![TimedEdit {
            tick: 10,
            edit: Edit::LoadAsset {
                id: AssetId(C),
                asset: "replay::Stats".to_string(),
                value: postcard::to_allocvec(&game::Stats {
                    damage: 9,
                    next: None,
                })
                .unwrap(),
            },
        }],
        ..EditLog::new()
    };
    let replay = record_replay(
        &mut game::simulation(),
        &scene(&format!("{}{armed}", counter(1, 0)), ""),
        &texts(&[stats(B, 3), stats(C, 4)]),
        &inputs(30),
        &edits,
        30,
    )
    .unwrap();
    let ids: Vec<AssetId> = replay.assets.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![AssetId(B)]);
    assert_eq!(play(&replay).outcome, ReplayOutcome::Matched);
}

#[test]
fn altered_hash_reports_first_divergence() {
    let mut replay = recorded(60, &EditLog::new());
    let got = replay.hashes[30];
    replay.hashes[30] ^= 1;
    replay.hashes[40] ^= 1;
    assert_eq!(
        play(&replay).outcome,
        ReplayOutcome::Diverged {
            tick: 30,
            expected: got ^ 1,
            got
        }
    );
}

#[test]
fn changed_input_type_fails_with_decode_naming_the_tick() {
    let mut log = InputLog::new(1);
    for tick in 0..200 {
        let command = if tick == 120 {
            Command::Jump
        } else {
            Command::Idle
        };
        log.record(&[command]);
    }
    let replay = record_replay(
        &mut game::simulation(),
        &scene(&counter(3, 0), ""),
        &AssetTexts::new(),
        &log,
        &EditLog::new(),
        200,
    )
    .unwrap();
    let error = play_replay(&mut changed_input::simulation(), &replay, |_, _| {}).unwrap_err();
    match error {
        ReplayError::Decode { what, reason } => {
            assert_eq!(what, "input of tick 120");
            assert!(!reason.is_empty());
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
}

#[test]
fn added_optional_component_field_still_plays_and_flags_fingerprint() {
    let replay = recorded(60, &EditLog::new());
    let mut sim = added_field::simulation();
    assert_ne!(sim.schema_fingerprint(), replay.fingerprint);
    let report = play_replay(&mut sim, &replay, |_, _| {}).unwrap();
    assert!(report.fingerprint_differs);
    // The new field is part of every hash of a counter.
    assert!(matches!(
        report.outcome,
        ReplayOutcome::Diverged { tick: 0, .. }
    ));
    assert_eq!(sim.tick(), 0);
    assert!(matches!(report.diverged_snapshots, Some((None, _))));
    let entity = sim.entity_at_address(&[3]).unwrap();
    assert_eq!(
        sim.world()
            .get::<added_field::Counter>(entity)
            .map(|counter| counter.clone()),
        Some(added_field::Counter {
            value: 0,
            bonus: None
        })
    );
}

#[test]
fn changed_component_fails_with_decode_naming_the_edit() {
    let edits = EditLog {
        edits: vec![TimedEdit {
            tick: 30,
            edit: Edit::Insert {
                entity: EntityRef::Loaded(vec![3]),
                component: "replay::Counter".to_string(),
                value: counter_bytes(7),
            },
        }],
        ..EditLog::new()
    };
    let replay = recorded(60, &edits);
    let error = play_replay(&mut added_field::simulation(), &replay, |_, _| {}).unwrap_err();
    assert!(
        matches!(&error, ReplayError::Decode { what, .. } if what == "edit of tick 30"),
        "{error:?}"
    );
}

#[test]
fn broken_asset_fails_with_decode_naming_the_asset() {
    let mut replay = recorded(10, &EditLog::new());
    replay.assets[0].1 = "AssetFile(type: \"ludenea::Prefab\", value: (entities: 3))".to_string();
    let error = play_replay(&mut game::simulation(), &replay, |_, _| {}).unwrap_err();
    let expected = format!("asset {}", AssetId(A));
    assert!(
        matches!(&error, ReplayError::Decode { what, .. } if *what == expected),
        "{error:?}"
    );
}

#[test]
fn divergence_stops_there_with_both_snapshots() {
    let mut replay = recorded(REPLAY_CHECKPOINT_INTERVAL + 40, &EditLog::new());
    for diverged in [30, REPLAY_CHECKPOINT_INTERVAL + 20] {
        let mut altered = replay.clone();
        altered.hashes[diverged as usize] ^= 1;
        let mut sim = game::simulation();
        let report = play_replay(&mut sim, &altered, |_, _| {}).unwrap();
        assert!(matches!(
            report.outcome,
            ReplayOutcome::Diverged { tick, .. } if tick == diverged
        ));
        assert_eq!(sim.tick(), diverged);
        let (previous, at) = report.diverged_snapshots.unwrap();
        assert_eq!(at, sim.snapshot().encode());
        let mut fresh = game::simulation();
        fresh
            .restore(&Snapshot::decode(&previous.unwrap()).unwrap())
            .unwrap();
        assert_eq!(fresh.tick(), diverged - 1);
        assert_eq!(fresh.hash(), replay.hashes[diverged as usize - 1]);
    }
    replay.hashes.truncate(5);
    let report = play(&replay);
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    assert_eq!(report.diverged_snapshots, None);
}

#[test]
fn edit_that_no_longer_applies_is_an_edit_error() {
    let edits = EditLog {
        edits: vec![TimedEdit {
            tick: 5,
            edit: Edit::Despawn {
                entity: EntityRef::Loaded(vec![5, 1]),
            },
        }],
        ..EditLog::new()
    };
    let mut replay = recorded(20, &edits);
    let mut log = EditLog::decode(&replay.edits).unwrap();
    let mut again = log.edits[0].clone();
    again.tick = 6;
    log.edits.push(again);
    replay.edits = log.encode();
    let error = play_replay(&mut game::simulation(), &replay, |_, _| {}).unwrap_err();
    assert!(
        matches!(error, ReplayError::Edit { tick: 6, .. }),
        "{error:?}"
    );
}

#[test]
fn checkpoints_every_1024_ticks() {
    let ticks = 2 * REPLAY_CHECKPOINT_INTERVAL + 10;
    let replay = recorded(ticks, &EditLog::new());
    let at: Vec<u64> = replay.checkpoints.iter().map(|(tick, _)| *tick).collect();
    assert_eq!(at, vec![1024, 2048]);
    let mut sim = game::simulation();
    for (tick, bytes) in &replay.checkpoints {
        let snapshot = Snapshot::decode(bytes).unwrap();
        assert_eq!(snapshot.fingerprint, sim.schema_fingerprint());
        sim.restore(&snapshot).unwrap();
        assert_eq!(sim.hash(), replay.hashes[*tick as usize]);
    }
}

#[test]
fn replay_without_checkpoints_regenerates_them() {
    let ticks = 2 * REPLAY_CHECKPOINT_INTERVAL + 10;
    let replay = recorded(ticks, &EditLog::new());

    let mut without = replay.clone();
    without.checkpoints.clear();
    let report = play(&without);
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    assert_eq!(report.checkpoints, replay.checkpoints);

    let mut corrupt = replay.clone();
    corrupt.checkpoints[0].1 = vec![2, 0];
    let report = play(&corrupt);
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    assert_eq!(report.checkpoints, replay.checkpoints);

    // A checkpoint of another tick decodes but fails the hash check.
    let mut misplaced = replay.clone();
    misplaced.checkpoints[1].1 = replay.checkpoints[0].1.clone();
    let report = play(&misplaced);
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    assert_eq!(report.checkpoints, replay.checkpoints);
}
