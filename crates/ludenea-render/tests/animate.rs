use std::collections::BTreeMap;

use ludenea_app::{App, Sim, Simulation, Time};
use ludenea_ecs::{AssetId, Entity, Handle, IntoSystemConfig, Query, Res};
use ludenea_render::{
    Animate, AnimationError, Clip, RenderPlugin, Sprite, SpriteAnimation, SpriteSheet,
};

const SHEET: AssetId = AssetId(0x51);
const TEXTURE: AssetId = AssetId(7);

fn clip(first: u32, count: u32, ticks_per_frame: u32) -> Clip {
    Clip {
        first,
        count,
        ticks_per_frame,
    }
}

fn sheet() -> SpriteSheet {
    SpriteSheet {
        texture: TEXTURE,
        clips: BTreeMap::from([
            ("idle".to_string(), clip(0, 2, 10)),
            ("run".to_string(), clip(2, 3, 4)),
        ]),
    }
}

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(RenderPlugin);
    let mut simulation = app.build().unwrap();
    simulation
        .world_mut()
        .insert_asset_ron(
            SHEET,
            "ludenea::SpriteSheet",
            &ron::to_string(&sheet()).unwrap(),
        )
        .unwrap();
    simulation
}

fn animation(clip: &str, started: u64, looping: bool) -> SpriteAnimation {
    SpriteAnimation {
        sheet: Handle::new(SHEET),
        clip: clip.into(),
        started,
        looping,
    }
}

fn spawn(simulation: &mut Simulation, animation: SpriteAnimation) -> Entity {
    let world = simulation.world_mut();
    let entity = world.spawn();
    world.insert(entity, Sprite::default()).unwrap();
    world.insert(entity, animation).unwrap();
    entity
}

fn sprite(simulation: &Simulation, entity: Entity) -> Sprite {
    simulation.world().get::<Sprite>(entity).unwrap().clone()
}

#[test]
fn looping_clip_wraps() {
    let run = clip(2, 3, 4);
    let frames: Vec<u32> = [0, 3, 4, 11, 12, 13, 24]
        .into_iter()
        .map(|elapsed| run.frame_at(elapsed, true))
        .collect();
    assert_eq!(frames, [2, 2, 3, 4, 2, 2, 2]);
}

#[test]
fn one_shot_clip_holds_its_last_frame() {
    let run = clip(2, 3, 4);
    let frames: Vec<u32> = [0, 4, 8, 11, 12, 1_000, u64::MAX]
        .into_iter()
        .map(|elapsed| run.frame_at(elapsed, false))
        .collect();
    assert_eq!(frames, [2, 3, 4, 4, 4, 4, 4]);
}

#[test]
fn a_tick_before_the_start_counts_as_the_first_frame() {
    let animation = animation("run", 10, true);
    assert_eq!(animation.frame(&sheet(), 3), Ok(2));
    assert_eq!(animation.frame(&sheet(), 14), Ok(3));
}

#[test]
fn animation_writes_sprite_frame_and_texture_each_tick() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, animation("run", 0, true));
    let mut frames = Vec::new();
    for _ in 0..13 {
        simulation.step().unwrap();
        let sprite = sprite(&simulation, entity);
        assert_eq!(sprite.texture, TEXTURE);
        frames.push(sprite.frame);
    }
    // The step of tick t shows the frame of tick t.
    assert_eq!(frames, [2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 2]);
}

#[test]
fn unknown_clip_keeps_the_last_frame() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, animation("run", 0, true));
    for _ in 0..6 {
        simulation.step().unwrap();
    }
    let before = sprite(&simulation, entity);
    assert_eq!(before.frame, 3);
    simulation
        .world_mut()
        .get_mut::<SpriteAnimation>(entity)
        .unwrap()
        .clip = "jump".into();
    for _ in 0..10 {
        simulation.step().unwrap();
        assert_eq!(sprite(&simulation, entity), before);
    }
    assert_eq!(
        animation("jump", 0, true).frame(&sheet(), 0),
        Err(AnimationError::UnknownClip {
            clip: "jump".into(),
            known: vec!["idle".into(), "run".into()],
        })
    );
}

fn hashes(threads: usize) -> Vec<u64> {
    let mut simulation = simulation();
    simulation.set_threads(threads).unwrap();
    for index in 0..200u64 {
        let clip = if index % 3 == 0 { "idle" } else { "run" };
        spawn(&mut simulation, animation(clip, index % 17, index % 2 == 0));
    }
    (0..600)
        .map(|_| {
            simulation.step().unwrap();
            simulation.hash()
        })
        .collect()
}

#[test]
fn animation_is_deterministic_across_thread_counts() {
    assert_eq!(hashes(1), hashes(8));
}

/// Switches every animation to `idle` from tick 6, as a game reacting to input would.
fn stop_running(time: Res<Time>, mut animations: Query<&mut SpriteAnimation>) {
    if time.tick == 6 {
        for animation in animations.iter_mut() {
            animation.clip = "idle".into();
            animation.started = time.tick;
        }
    }
}

#[test]
fn clip_switched_before_animate_shows_in_the_same_tick() {
    let frames = |before: bool| {
        let mut app = App::new();
        // Registered after the plugin: without `.before(Animate)` it runs after `animate`.
        app.plugin(RenderPlugin);
        if before {
            app.system(Sim, stop_running.before(Animate));
        } else {
            app.system(Sim, stop_running);
        }
        let mut simulation = app.build().unwrap();
        simulation
            .world_mut()
            .insert_asset_ron(
                SHEET,
                "ludenea::SpriteSheet",
                &ron::to_string(&sheet()).unwrap(),
            )
            .unwrap();
        let entity = spawn(&mut simulation, animation("run", 0, true));
        (0..8)
            .map(|_| {
                simulation.step().unwrap();
                sprite(&simulation, entity).frame
            })
            .collect::<Vec<u32>>()
    };
    // `run` shows frame 3 at ticks 4 to 7; `idle` starts on frame 0.
    assert_eq!(frames(true), [2, 2, 2, 2, 3, 3, 0, 0]);
    assert_eq!(frames(false), [2, 2, 2, 2, 3, 3, 3, 0]);
}

#[test]
fn animate_runs_after_the_physics_step_when_physics_is_added() {
    let mut app = App::new();
    app.plugin(RenderPlugin)
        .plugin(ludenea_physics::PhysicsPlugin);
    let systems: Vec<&str> = app
        .build()
        .unwrap()
        .systems()
        .into_iter()
        .map(|system| system.name)
        .collect();
    let position = |suffix: &str| {
        systems
            .iter()
            .position(|name| name.ends_with(suffix))
            .unwrap_or_else(|| panic!("no system `{suffix}` in {systems:?}"))
    };
    assert!(
        position("physics_step") < position("animate"),
        "{systems:?}"
    );
}
