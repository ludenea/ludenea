use std::collections::BTreeSet;

use ludenea::app::AssetTexts;
use ludenea::app::RawGamepad;
use ludenea::app::assets::asset_file;
use ludenea::app::players::{LiveDevices, automatic_claims, split};
use ludenea::ecs::AssetId;
use ludenea::prelude::*;
use ludenea::render::SpriteAnimation;
use swarm::{BUG_SHEET, Player, SwarmInput, Wanderer};

const SCENE: &str = include_str!("../assets/swarm.scene.ron");
const SHEET: &str = include_str!("../assets/bug.sheet.ron");

fn loaded(players: usize) -> Simulation {
    let mut simulation = swarm::app().build().expect("the swarm app builds");
    let sheet = AssetId(u128::from_str_radix(BUG_SHEET, 16).expect("a hexadecimal identifier"));
    let assets = AssetTexts::from([(sheet, asset_file("ludenea::SpriteSheet", SHEET))]);
    simulation
        .load_scene_with(SCENE, &assets)
        .unwrap_or_else(|error| panic!("{error}"));
    simulation.set_players(players);
    simulation
}

fn player_positions(simulation: &Simulation) -> Vec<(u8, Vec2)> {
    let world = simulation.world();
    let mut players: Vec<_> = world
        .entities()
        .filter_map(|entity| {
            let player = world.get::<Player>(entity)?;
            let pose = world.get::<Pose>(entity)?;
            Some((player.index, pose.position))
        })
        .collect();
    players.sort_by_key(|(index, _)| *index);
    players
}

#[test]
fn swarm_spawns_ten_thousand_animated_sprites() {
    let simulation = loaded(1);
    let world = simulation.world();
    let animated = world
        .entities()
        .filter(|&entity| {
            world.get::<Wanderer>(entity).is_some()
                && world
                    .get::<SpriteAnimation>(entity)
                    .is_some_and(|a| a.looping)
                && world.get::<Sprite>(entity).is_some()
        })
        .count();
    assert_eq!(animated, 10_000);
    assert_eq!(player_positions(&simulation).len(), 4);
}

#[test]
fn player_two_moves_with_the_right_half() {
    let mut simulation = loaded(2);
    let before = player_positions(&simulation);
    let devices = LiveDevices {
        keys: BTreeSet::from(["ArrowRight".to_string()]),
        gamepads: Vec::new(),
    };
    let raw = split(&devices, &automatic_claims(2));
    let bytes = simulation.map_players(&raw).unwrap();
    simulation.step_encoded(Some(&bytes)).unwrap();
    let after = player_positions(&simulation);
    assert_eq!(after[0].1, before[0].1, "player 0 holds the left half");
    assert!(after[1].1.x > before[1].1.x, "player 1 moves right");
    assert_eq!(after[1].1.y, before[1].1.y);
}

#[test]
fn absent_players_are_hidden() {
    let mut simulation = loaded(2);
    simulation.step().unwrap();
    let frame = simulation.render_frame(&BTreeSet::new()).unwrap();
    let world = simulation.world();
    let visible = |index: u8| {
        let entity = world
            .entities()
            .find(|&entity| {
                world
                    .get::<Player>(entity)
                    .is_some_and(|p| p.index == index)
            })
            .unwrap();
        let item = frame
            .sprites
            .iter()
            .find(|item| item.entity == entity)
            .unwrap();
        item.tint.0 & 0xFF != 0
    };
    assert_eq!(
        [visible(0), visible(1), visible(2), visible(3)],
        [true, true, false, false]
    );
}

#[test]
fn mapping_reads_keys_and_gamepad_and_normalizes() {
    let simulation = swarm::app().build().unwrap();
    let map = |raw: RawInput| {
        let bytes = simulation.map_raw_input(&[raw]).unwrap();
        postcard::from_bytes::<Vec<SwarmInput>>(&bytes)
            .unwrap()
            .remove(0)
            .direction
    };
    let keys = |codes: &[&str]| RawInput {
        keys: codes.iter().map(|code| code.to_string()).collect(),
        gamepads: Vec::new(),
    };
    let diagonal = map(keys(&["ArrowRight", "KeyW"]));
    assert!((diagonal.length() - 1.0).abs() < 1e-6);
    assert_eq!(map(keys(&["ArrowLeft", "ArrowRight"])), Vec2::ZERO);
    let pad = RawGamepad {
        buttons: vec![0.0; 16],
        axes: vec![0.0, -1.0, 0.0, 0.0],
    };
    let up = map(RawInput {
        keys: BTreeSet::new(),
        gamepads: vec![pad],
    });
    assert_eq!(up, Vec2::new(0.0, 1.0));
}

#[test]
// Swarm's systems are sequential: this proves the thread setting does not change the hashes,
// not parallel system scheduling.
fn same_hashes_at_one_and_eight_threads() {
    let run = |threads: usize| {
        let mut simulation = loaded(4);
        simulation.set_threads(threads).unwrap();
        (0..600)
            .map(|tick| {
                let direction = Vec2::new(if tick % 90 < 45 { 1.0 } else { -1.0 }, 0.5);
                simulation
                    .step_with(&vec![SwarmInput { direction }; 4])
                    .unwrap();
                simulation.hash()
            })
            .collect::<Vec<_>>()
    };
    let (one, eight) = (run(1), run(8));
    assert_ne!(one[0], one[599], "the simulation advances");
    assert!(
        one.windows(2).all(|pair| pair[0] != pair[1]),
        "every tick changes the state"
    );
    assert_eq!(one, eight);
}

#[test]
fn render_frame_lists_every_sprite() {
    let mut simulation = loaded(4);
    simulation.step().unwrap();
    let frame = simulation.render_frame(&BTreeSet::new()).unwrap();
    // 10,000 wanderers, 4 players and the 64 background sprites.
    assert_eq!(frame.sprites.len(), 10_068);
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
    let layers: Vec<_> = frame
        .layers
        .iter()
        .map(|l| (l.name.as_str(), l.parallax))
        .collect();
    assert_eq!(layers, [("background", 0.5), ("world", 1.0)]);
    let camera = frame.camera.expect("the scene has a camera");
    assert_eq!((camera.view.x, camera.view.y), (64.0, 36.0));
}

#[test]
fn committed_assets_are_the_generated_ones() {
    let folder = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
    for (name, bytes) in swarm::generated_files() {
        let committed = std::fs::read(format!("{folder}/{name}")).unwrap();
        assert!(
            committed == bytes,
            "{name} is stale; run `cargo run -p swarm --bin generate`"
        );
    }
    let again = swarm::generated_files();
    assert!(
        again == swarm::generated_files(),
        "generation is not reproducible"
    );
}
