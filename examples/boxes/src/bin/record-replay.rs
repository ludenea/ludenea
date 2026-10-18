//! Records `golden/spawner.replay`: 3,000 ticks of the spawner scene, with its spawner stats
//! changed at tick 1,000 and a component inserted on a prefab instance at tick 2,000.

use std::fs;
use std::path::Path;

use boxes::SpawnerStats;
use ludenea::app::edit::{Edit, EditLog, EntityRef, TimedEdit};
use ludenea::app::replay::record_replay;
use ludenea::cli::assets::project_assets;
use ludenea::ecs::{Asset, AssetId, Component};
use ludenea::prelude::{Body, InputLog, Vec2};

const TICKS: u64 = 3_000;
/// The identifier in `assets/spawner.ron.meta`.
const STATS: u128 = 0xbef6_17c9_e94d_41c4_b0e0_7882_849c_ef18;

fn main() {
    let output = std::env::args()
        .nth(1)
        .expect("usage: record-replay <output.replay>");
    let scene_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/spawner.scene.ron"
    ));
    let scene = fs::read_to_string(scene_path).expect("cannot read the spawner scene");
    let assets = project_assets(scene_path, None).expect("cannot read the scene's assets");

    let mut simulation = boxes::app().build().expect("the boxes app builds");
    let world = simulation.world();
    let stats = SpawnerStats {
        every_ticks: 45,
        impulse: Vec2::new(-6.0, 3.0),
    };
    let body = Body {
        linear_velocity: Vec2::new(0.0, 12.0),
        ..Body::dynamic()
    };
    let stats_codec = world.asset_codec(SpawnerStats::NAME).expect("registered");
    let body_codec = world.component_codec(Body::NAME).expect("registered");
    let mut edits = EditLog::new();
    edits.edits.push(TimedEdit {
        tick: 1_000,
        edit: Edit::SetAsset {
            id: AssetId(STATS),
            value: (stats_codec.encode)(&stats),
        },
    });
    edits.edits.push(TimedEdit {
        tick: 2_000,
        edit: Edit::Insert {
            entity: EntityRef::Loaded(vec![6, 0]),
            component: Body::NAME.to_string(),
            value: (body_codec.encode)(&body),
        },
    });

    let mut inputs = InputLog::new(1);
    for _ in 0..TICKS {
        inputs.record(&[boxes::BoxesInput::default()]);
    }
    let replay = record_replay(&mut simulation, &scene, &assets, &inputs, &edits, TICKS)
        .expect("the spawner scenario records");
    fs::write(&output, replay.encode()).expect("cannot write the replay");
}
