use std::collections::BTreeSet;
use std::path::Path;

use arena::{
    ArenaInput, FIGHTER_RADIUS, Fighter, HERO_IDLE_SHEET, JUMP_SPEED, RUN_SPEED, SpawnPoint,
    WARRIOR_FALL_SHEET, WARRIOR_IDLE_SHEET, WARRIOR_IDLE_TEXTURE, WARRIOR_JUMP_SHEET,
    WARRIOR_RUN_SHEET, asset_id,
};
use ludenea::app::RawGamepad;
use ludenea::app::project::project_assets;
use ludenea::prelude::*;

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/arena.scene.ron");
/// Within this distance, two positions computed along different float paths agree.
const EPSILON: f32 = 1e-4;

/// The arena scene for `players` players, plus a static box of half extents `half` at
/// `centre`: the scene has no colliders until its level is built.
fn arena(players: usize, boxes: &[(Vec2, Vec2)]) -> Simulation {
    let scene = std::fs::read_to_string(SCENE).expect("the scene is committed");
    let assets = project_assets(Path::new(SCENE), None).expect("the project's assets read");
    let mut simulation = arena::app().build().expect("the arena app builds");
    simulation
        .load_scene_with(&scene, &assets)
        .unwrap_or_else(|error| panic!("{error}"));
    simulation.set_players(players);
    let world = simulation.world_mut();
    for &(centre, half) in boxes {
        let entity = world.spawn();
        world
            .insert(
                entity,
                Pose {
                    position: centre,
                    rotation: Rot::IDENTITY,
                },
            )
            .unwrap();
        world.insert(entity, Body::fixed()).unwrap();
        world
            .insert(
                entity,
                Collider::new(Shape::Polygon(Polygon::make_box(half.x, half.y))),
            )
            .unwrap();
    }
    simulation
}

/// A floor 100 units wide whose top is at `top`.
fn floor(top: f32) -> (Vec2, Vec2) {
    (Vec2::new(0.0, top - 0.5), Vec2::new(50.0, 0.5))
}

fn input(run: f32, jump: bool) -> ArenaInput {
    ArenaInput { run, jump }
}

fn step(simulation: &mut Simulation, inputs: &[ArenaInput], ticks: usize) {
    for _ in 0..ticks {
        simulation.step_with(inputs).unwrap();
    }
}

/// Everything about one fighter, by player.
struct Seen {
    fighter: Fighter,
    position: Vec2,
    animation: SpriteAnimation,
    sprite: Sprite,
    body: Body,
    collider: Collider,
}

fn fighters(simulation: &Simulation) -> Vec<Seen> {
    let world = simulation.world();
    let mut seen: Vec<Seen> = world
        .entities()
        .filter_map(|entity| {
            Some(Seen {
                fighter: world.get::<Fighter>(entity)?.clone(),
                position: world.get::<Pose>(entity)?.position,
                animation: world.get::<SpriteAnimation>(entity)?.clone(),
                sprite: world.get::<Sprite>(entity)?.clone(),
                body: *world.get::<Body>(entity)?,
                collider: world.get::<Collider>(entity)?.clone(),
            })
        })
        .collect();
    seen.sort_by_key(|seen| seen.fighter.player);
    seen
}

fn fighter(simulation: &Simulation, player: u8) -> Seen {
    fighters(simulation)
        .into_iter()
        .find(|seen| seen.fighter.player == player)
        .unwrap_or_else(|| panic!("player {player} has no fighter"))
}

fn spawn_point(simulation: &Simulation, player: u8) -> Vec2 {
    let world = simulation.world();
    world
        .entities()
        .find(|&entity| {
            world
                .get::<SpawnPoint>(entity)
                .is_some_and(|point| point.player == player)
        })
        .and_then(|entity| Some(world.get::<Pose>(entity)?.position))
        .unwrap_or_else(|| panic!("the scene has no spawn point for player {player}"))
}

fn camera(simulation: &Simulation) -> (Vec2, Vec2) {
    let world = simulation.world();
    let entity = world
        .entities()
        .find(|&entity| world.get::<Camera2d>(entity).is_some())
        .expect("the scene has a camera");
    (
        world.get::<Pose>(entity).unwrap().position,
        world.get::<Camera2d>(entity).unwrap().view,
    )
}

#[test]
fn one_player_spawns_one_fighter() {
    let mut simulation = arena(1, &[floor(0.0)]);
    assert!(
        fighters(&simulation).is_empty(),
        "no fighter before the first tick"
    );
    step(&mut simulation, &[ArenaInput::default()], 3);
    let all = fighters(&simulation);
    assert_eq!(all.len(), 1);
    let seen = &all[0];
    assert_eq!(seen.fighter.player, 0);
    assert_eq!(seen.position, spawn_point(&simulation, 0));
    assert_eq!(seen.body.kind, BodyKind::Kinematic);
    assert_eq!(seen.body.linear_velocity, Vec2::ZERO);
    assert!(matches!(seen.collider.shape, Shape::Capsule(_)));
    assert_eq!(seen.sprite.layer, "fighters");
    assert_eq!(seen.animation.sheet.id, asset_id(WARRIOR_IDLE_SHEET));
    assert_eq!(seen.animation.clip, "idle");
    assert_eq!(
        seen.sprite.texture,
        asset_id(WARRIOR_IDLE_TEXTURE),
        "the scene's `FighterSheets` loads the sheets, so the animation plays"
    );
    assert!(seen.fighter.grounded);
}

#[test]
fn two_players_spawn_two_fighters() {
    let mut simulation = arena(2, &[floor(0.0)]);
    step(
        &mut simulation,
        &[ArenaInput::default(), ArenaInput::default()],
        1,
    );
    let all = fighters(&simulation);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].position, spawn_point(&simulation, 0));
    assert_eq!(all[1].position, spawn_point(&simulation, 1));
    assert_eq!(all[0].animation.sheet.id, asset_id(WARRIOR_IDLE_SHEET));
    assert_eq!(all[1].animation.sheet.id, asset_id(HERO_IDLE_SHEET));
    // Later ticks spawn no more.
    step(
        &mut simulation,
        &[ArenaInput::default(), ArenaInput::default()],
        5,
    );
    assert_eq!(fighters(&simulation).len(), 2);
}

#[test]
fn fighter_runs_and_turns_its_sprite() {
    let mut simulation = arena(1, &[floor(0.0)]);
    step(&mut simulation, &[ArenaInput::default()], 1);
    let start = fighter(&simulation, 0).position;

    step(&mut simulation, &[input(1.0, false)], 10);
    let right = fighter(&simulation, 0);
    assert!((right.position.x - (start.x + 10.0 * RUN_SPEED)).abs() < EPSILON);
    assert_eq!(right.position.y, start.y);
    assert_eq!(right.animation.clip, "run");
    assert_eq!(right.animation.sheet.id, asset_id(WARRIOR_RUN_SHEET));
    assert_eq!(right.fighter.facing, 1);
    assert!(!right.sprite.flip_x);

    step(&mut simulation, &[input(-0.5, false)], 4);
    let left = fighter(&simulation, 0);
    assert!((left.position.x - (right.position.x - 4.0 * 0.5 * RUN_SPEED)).abs() < EPSILON);
    assert_eq!(left.fighter.facing, -1);
    assert!(left.sprite.flip_x);

    step(&mut simulation, &[ArenaInput::default()], 1);
    let still = fighter(&simulation, 0);
    assert_eq!(still.position, left.position);
    assert_eq!(still.animation.clip, "idle");
    assert!(still.sprite.flip_x, "keeps facing its last run");

    // Out-of-range and non-finite runs cannot outrun the fighter or break its position.
    step(&mut simulation, &[input(f32::NAN, false)], 1);
    assert_eq!(fighter(&simulation, 0).position, still.position);
    step(&mut simulation, &[input(50.0, false)], 1);
    let fast = fighter(&simulation, 0);
    assert!((fast.position.x - (still.position.x + RUN_SPEED)).abs() < EPSILON);
}

#[test]
fn fighter_lands_on_a_collider_and_idles() {
    let mut simulation = arena(1, &[floor(-3.0)]);
    step(&mut simulation, &[ArenaInput::default()], 1);
    let falling = fighter(&simulation, 0);
    assert!(!falling.fighter.grounded);
    assert_eq!(falling.animation.clip, "fall");
    assert_eq!(falling.animation.sheet.id, asset_id(WARRIOR_FALL_SHEET));

    step(&mut simulation, &[ArenaInput::default()], 60);
    let landed = fighter(&simulation, 0);
    assert!(
        (landed.position.y - -3.0).abs() < EPSILON,
        "stands on the floor's top, not in it: {}",
        landed.position.y
    );
    assert!(landed.fighter.grounded);
    assert_eq!(landed.fighter.velocity, Vec2::ZERO);
    assert_eq!(landed.animation.clip, "idle");
    assert_eq!(landed.animation.sheet.id, asset_id(WARRIOR_IDLE_SHEET));
}

#[test]
fn jump_then_fall_switch_sheet_and_clip() {
    let mut simulation = arena(1, &[floor(0.0)]);
    step(&mut simulation, &[ArenaInput::default()], 2);
    let ground = fighter(&simulation, 0).position.y;

    step(&mut simulation, &[input(0.0, true)], 1);
    let jumped = fighter(&simulation, 0);
    let jump_tick = simulation.tick() - 1;
    assert!((jumped.position.y - (ground + JUMP_SPEED)).abs() < EPSILON);
    assert_eq!(jumped.animation.clip, "jump");
    assert_eq!(jumped.animation.sheet.id, asset_id(WARRIOR_JUMP_SHEET));
    assert_eq!(jumped.animation.started, jump_tick);
    assert!(!jumped.animation.looping);

    // Holding jump in the air does nothing, and the clip does not restart while rising.
    step(&mut simulation, &[input(0.0, true)], 5);
    let rising = fighter(&simulation, 0);
    assert_eq!(rising.animation.clip, "jump");
    assert_eq!(rising.animation.started, jump_tick);
    assert!(rising.fighter.velocity.y > 0.0);

    let mut ticks = 0;
    while fighter(&simulation, 0).fighter.velocity.y > 0.0 {
        step(&mut simulation, &[ArenaInput::default()], 1);
        ticks += 1;
        assert!(ticks < 100, "the jump peaks");
    }
    let falling = fighter(&simulation, 0);
    assert_eq!(falling.animation.clip, "fall");
    assert_eq!(falling.animation.sheet.id, asset_id(WARRIOR_FALL_SHEET));
    assert_eq!(falling.animation.started, simulation.tick() - 1);

    step(&mut simulation, &[ArenaInput::default()], 60);
    let landed = fighter(&simulation, 0);
    assert!((landed.position.y - ground).abs() < EPSILON);
    assert_eq!(landed.animation.clip, "idle");
}

#[test]
fn fighter_stops_at_a_wall() {
    let wall = (Vec2::new(-2.5, 2.0), Vec2::new(0.5, 2.0));
    let mut simulation = arena(1, &[floor(0.0), wall]);
    step(&mut simulation, &[ArenaInput::default()], 1);
    step(&mut simulation, &[input(1.0, false)], 60);
    let stopped = fighter(&simulation, 0);
    let face = -3.0 - FIGHTER_RADIUS;
    assert!(
        (stopped.position.x - face).abs() < EPSILON,
        "stops with its capsule against the wall: {}",
        stopped.position.x
    );
    assert_eq!(stopped.fighter.velocity.x, 0.0);
    assert_eq!(stopped.animation.clip, "idle");
    // It still turns away and leaves.
    step(&mut simulation, &[input(-1.0, false)], 1);
    let away = fighter(&simulation, 0);
    assert!((away.position.x - (face - RUN_SPEED)).abs() < EPSILON);
}

#[test]
fn camera_frames_both_fighters_and_keeps_its_minimum_view() {
    let mut simulation = arena(2, &[floor(0.0)]);
    let idle = [ArenaInput::default(), ArenaInput::default()];
    step(&mut simulation, &idle, 1);
    let (left, right) = (spawn_point(&simulation, 0), spawn_point(&simulation, 1));
    let (centre, view) = camera(&simulation);
    assert_eq!(centre, (left + right) * 0.5);
    assert_eq!(
        view,
        Vec2::new(24.0, 13.5),
        "close fighters keep the minimum view"
    );

    step(
        &mut simulation,
        &[input(-1.0, false), input(1.0, false)],
        50,
    );
    let all = fighters(&simulation);
    let (a, b) = (all[0].position, all[1].position);
    let (centre, view) = camera(&simulation);
    assert!((centre.x - (a.x + b.x) * 0.5).abs() < EPSILON);
    assert!((view.x - ((b.x - a.x) + 6.0)).abs() < EPSILON);
    assert!(view.x > 24.0);
    assert_eq!(view.y, 13.5);
}

#[test]
fn mapping_reads_keys_and_the_gamepad() {
    let simulation = arena::app().build().unwrap();
    let map = |raw: RawInput| {
        let bytes = simulation.map_raw_input(&[raw]).unwrap();
        postcard::from_bytes::<Vec<ArenaInput>>(&bytes)
            .unwrap()
            .remove(0)
    };
    let keys = |codes: &[&str]| RawInput {
        keys: codes.iter().map(|code| code.to_string()).collect(),
        gamepads: Vec::new(),
    };
    assert_eq!(map(keys(&["ArrowRight"])), input(1.0, false));
    assert_eq!(map(keys(&["KeyA"])), input(-1.0, false));
    assert_eq!(map(keys(&["KeyA", "KeyD"])), input(0.0, false));
    for jump in ["Space", "KeyW", "ArrowUp"] {
        assert_eq!(map(keys(&[jump])), input(0.0, true), "{jump}");
    }
    let pad = |axis: f32, pressed: &[usize]| {
        let mut buttons = vec![0.0; 16];
        for &button in pressed {
            buttons[button] = 1.0;
        }
        RawInput {
            keys: BTreeSet::from(["ArrowRight".to_string()]),
            gamepads: vec![RawGamepad {
                buttons,
                axes: vec![axis, 0.0],
            }],
        }
    };
    assert_eq!(map(pad(0.1, &[])), input(1.0, false), "dead zone");
    assert_eq!(map(pad(-1.0, &[0])), input(0.0, true), "stick and button 0");
    assert_eq!(map(pad(0.0, &[14])), input(0.0, false), "d-pad left");
    assert_eq!(map(pad(1.0, &[15])), input(1.0, false), "clamped to 1");
}

#[test]
fn same_hashes_at_one_two_and_eight_threads() {
    let run = |threads: usize| {
        let mut simulation = arena(2, &[floor(-2.0)]);
        simulation.set_threads(threads).unwrap();
        (0..600)
            .map(|tick| {
                simulation.step_with(&arena::scripted_inputs(tick)).unwrap();
                simulation.hash()
            })
            .collect::<Vec<_>>()
    };
    let one = run(1);
    assert_ne!(one[0], one[599], "the simulation advances");
    for threads in [2, 8] {
        let other = run(threads);
        let first = one.iter().zip(&other).position(|(a, b)| a != b);
        assert_eq!(first, None, "{threads} threads diverge at this tick");
    }
}
