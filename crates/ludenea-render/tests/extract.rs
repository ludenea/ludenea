use std::collections::{BTreeMap, BTreeSet};

use ludenea_app::input_log::InputLog;
use ludenea_app::pose::Pose;
use ludenea_app::trace::Trace;
use ludenea_app::{App, Sim, Simulation, Time};
use ludenea_draw::frame::{CameraItem, RenderFrame};
use ludenea_ecs::{AssetId, Commands, Entity, Handle, Query, Res};
use ludenea_math::{Rot, Vec2};
use ludenea_render::{
    Camera2d, Clip, RenderPlugin, Sprite, SpriteAnimation, SpriteSheet, Teleported, Tilemap,
};

const SHEET: AssetId = AssetId(0x51);

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(RenderPlugin)
        .render_layer("background", 0.5)
        .render_layer("world", 1.0);
    app.build().unwrap()
}

fn spawn(simulation: &mut Simulation, position: Option<Vec2>) -> Entity {
    let world = simulation.world_mut();
    let entity = world.spawn();
    if let Some(position) = position {
        world
            .insert(
                entity,
                Pose {
                    position,
                    rotation: Rot::IDENTITY,
                },
            )
            .unwrap();
    }
    entity
}

fn sprite(layer: &str, order: i32) -> Sprite {
    Sprite {
        layer: layer.into(),
        order,
        ..Sprite::default()
    }
}

fn frame(simulation: &Simulation) -> RenderFrame {
    simulation.render_frame(&BTreeSet::new()).unwrap()
}

#[test]
fn sprites_are_extracted_sorted_by_layer_order_and_entity() {
    let mut simulation = simulation();
    let mut place = |sprite: Sprite| {
        let entity = spawn(&mut simulation, Some(Vec2::new(1.0, 2.0)));
        simulation.world_mut().insert(entity, sprite).unwrap();
        entity
    };
    let a = place(sprite("world", 0));
    let b = place(sprite("background", 5));
    let c = place(sprite("world", -1));
    let d = place(sprite("", 5));
    let e = place(sprite("world", 0));
    let frame = frame(&simulation);
    let order: Vec<(Entity, u16, i32)> = frame
        .sprites
        .iter()
        .map(|item| (item.entity, item.layer, item.order))
        .collect();
    assert_eq!(
        order,
        [(b, 0, 5), (d, 0, 5), (c, 1, -1), (a, 1, 0), (e, 1, 0)]
    );
    assert_eq!(frame.sprites[0].position, Vec2::new(1.0, 2.0));
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
}

fn tilemap(layer: &str) -> Tilemap {
    Tilemap {
        tileset: AssetId(9),
        cell_size: Vec2::new(1.0, 1.0),
        width: 2,
        height: 1,
        tiles: vec![Some(3), None],
        layer: layer.into(),
        order: 2,
    }
}

#[test]
fn tilemaps_are_extracted_from_their_pose() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, Some(Vec2::new(-4.0, -3.0)));
    simulation
        .world_mut()
        .insert(entity, tilemap("background"))
        .unwrap();
    let frame = frame(&simulation);
    let [item] = frame.tilemaps.as_slice() else {
        panic!("expected one tilemap, got {:?}", frame.tilemaps);
    };
    assert_eq!(
        (item.entity, item.layer, item.order, item.tileset),
        (entity, 0, 2, AssetId(9))
    );
    assert_eq!(item.position, Vec2::new(-4.0, -3.0));
    assert_eq!(item.tiles, [Some(3), None]);
}

#[test]
fn tilemap_without_pose_warns() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, None);
    simulation
        .world_mut()
        .insert(entity, tilemap("world"))
        .unwrap();
    let frame = frame(&simulation);
    assert!(frame.tilemaps.is_empty());
    assert_eq!(
        frame.warnings,
        [format!(
            "entity {entity}: has a Tilemap but no Pose, so it is not drawn; add a Pose"
        )]
    );
}

#[test]
fn tilemap_on_an_undeclared_layer_is_skipped_with_a_warning_listing_layers() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, Some(Vec2::ZERO));
    simulation
        .world_mut()
        .insert(entity, tilemap("foreground"))
        .unwrap();
    let frame = frame(&simulation);
    assert!(frame.tilemaps.is_empty());
    assert_eq!(
        frame.warnings,
        [format!(
            "entity {entity}: tilemap layer `foreground` is not declared; declared layers: `background`, `world`"
        )]
    );
}

fn camera(simulation: &mut Simulation, position: Vec2, view: Vec2) -> Entity {
    let entity = spawn(simulation, Some(position));
    simulation
        .world_mut()
        .insert(entity, Camera2d { view })
        .unwrap();
    entity
}

#[test]
fn camera_comes_from_pose_and_view() {
    let mut simulation = simulation();
    assert_eq!(frame(&simulation).camera, None);
    let entity = camera(&mut simulation, Vec2::new(3.0, -1.0), Vec2::new(16.0, 9.0));
    let frame = frame(&simulation);
    assert_eq!(
        frame.camera,
        Some(CameraItem {
            entity,
            center: Vec2::new(3.0, -1.0),
            view: Vec2::new(16.0, 9.0),
        })
    );
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
}

#[test]
fn two_cameras_warn_and_keep_the_lowest_entity() {
    let mut simulation = simulation();
    let without_pose = spawn(&mut simulation, None);
    simulation
        .world_mut()
        .insert(
            without_pose,
            Camera2d {
                view: Vec2::new(1.0, 1.0),
            },
        )
        .unwrap();
    let first = camera(&mut simulation, Vec2::new(1.0, 0.0), Vec2::new(8.0, 8.0));
    let second = camera(&mut simulation, Vec2::new(2.0, 0.0), Vec2::new(4.0, 4.0));
    let frame = frame(&simulation);
    let used = frame.camera.unwrap();
    assert_eq!((used.entity, used.center), (first, Vec2::new(1.0, 0.0)));
    assert!(
        frame.warnings.iter().any(|warning| warning
            == &format!(
                "2 entities have a Camera2d and a Pose ({first}, {second}); {first} is used"
            )),
        "{:?}",
        frame.warnings
    );
}

#[test]
fn camera_without_pose_warns() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, None);
    simulation
        .world_mut()
        .insert(
            entity,
            Camera2d {
                view: Vec2::new(1.0, 1.0),
            },
        )
        .unwrap();
    let frame = frame(&simulation);
    assert_eq!(frame.camera, None);
    assert_eq!(
        frame.warnings,
        [format!(
            "entity {entity}: has a Camera2d but no Pose, so it is not used; add a Pose"
        )]
    );
}

#[test]
fn animation_without_sprite_warns() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, Some(Vec2::ZERO));
    let world = simulation.world_mut();
    world
        .insert_asset_ron(
            SHEET,
            "ludenea::SpriteSheet",
            "(texture: \"00000000000000000000000000000007\", clips: {\"idle\": (first: 0, count: 1, ticks_per_frame: 1)})",
        )
        .unwrap();
    world
        .insert(
            entity,
            SpriteAnimation {
                sheet: Handle::new(SHEET),
                clip: "idle".into(),
                started: 0,
                looping: true,
            },
        )
        .unwrap();
    let frame = frame(&simulation);
    assert_eq!(
        frame.warnings,
        [format!(
            "entity {entity}: has a SpriteAnimation but no Sprite, so it animates nothing; add a Sprite"
        )]
    );
}

#[test]
fn undeclared_layer_is_skipped_with_a_warning_listing_layers() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, Some(Vec2::ZERO));
    simulation
        .world_mut()
        .insert(entity, sprite("foreground", 0))
        .unwrap();
    let frame = frame(&simulation);
    assert!(frame.sprites.is_empty());
    assert_eq!(
        frame.warnings,
        [format!(
            "entity {entity}: sprite layer `foreground` is not declared; declared layers: `background`, `world`"
        )]
    );
}

#[test]
fn sprite_without_pose_warns() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, None);
    simulation
        .world_mut()
        .insert(entity, sprite("world", 0))
        .unwrap();
    let frame = frame(&simulation);
    assert!(frame.sprites.is_empty());
    assert_eq!(
        frame.warnings,
        [format!(
            "entity {entity}: has a Sprite but no Pose, so it is not drawn; add a Pose"
        )]
    );
}

#[test]
fn unknown_clip_warns_naming_known_clips() {
    let mut simulation = simulation();
    let sheet = SpriteSheet {
        texture: AssetId(7),
        clips: BTreeMap::from([
            (
                "idle".to_string(),
                Clip {
                    first: 0,
                    count: 1,
                    ticks_per_frame: 1,
                },
            ),
            (
                "run".to_string(),
                Clip {
                    first: 1,
                    count: 2,
                    ticks_per_frame: 1,
                },
            ),
        ]),
    };
    let entity = spawn(&mut simulation, Some(Vec2::ZERO));
    let world = simulation.world_mut();
    world
        .insert_asset_ron(
            SHEET,
            "ludenea::SpriteSheet",
            &ron::to_string(&sheet).unwrap(),
        )
        .unwrap();
    world.insert(entity, sprite("world", 0)).unwrap();
    world
        .insert(
            entity,
            SpriteAnimation {
                sheet: Handle::new(SHEET),
                clip: "jmup".into(),
                started: 0,
                looping: true,
            },
        )
        .unwrap();
    simulation.step().unwrap();
    let frame = frame(&simulation);
    assert_eq!(frame.sprites.len(), 1, "the sprite still draws");
    assert_eq!(
        frame.warnings,
        [format!(
            "entity {entity}: animation clip `jmup` is not in sprite sheet {SHEET}; its clips: `idle`, `run`"
        )]
    );
}

/// Moves every posed entity to x = 10 without travelling during the step of tick 1.
fn teleport_at_tick_1(
    time: Res<Time>,
    mut poses: Query<(Entity, &mut Pose)>,
    mut commands: Commands,
) {
    if time.tick != 1 {
        return;
    }
    for (entity, pose) in poses.iter_mut() {
        pose.position = Vec2::new(10.0, 0.0);
        commands.insert(entity, Teleported::this_step(&time));
    }
}

#[test]
fn teleported_is_listed_only_on_its_tick() {
    let mut app = App::new();
    app.plugin(RenderPlugin)
        .render_layer("world", 1.0)
        .system(Sim, teleport_at_tick_1);
    let mut simulation = app.build().unwrap();
    let entity = spawn(&mut simulation, Some(Vec2::ZERO));
    let mut listed = Vec::new();
    for _ in 0..4 {
        simulation.step().unwrap();
        let frame = frame(&simulation);
        listed.push((frame.tick, frame.teleported));
    }
    // The step of tick 1 teleports; the frame showing its result is the frame of tick 2.
    assert_eq!(
        listed,
        [(1, vec![]), (2, vec![entity]), (3, vec![]), (4, vec![])]
    );
}

/// A game extractor listing every posed entity as teleported, last entity first.
fn teleport_everything(world: &ludenea_ecs::World, frame: &mut RenderFrame) {
    let poses = Query::<(Entity, &Pose)>::new(world).unwrap();
    let mut entities: Vec<Entity> = poses.iter().map(|(entity, _)| entity).collect();
    entities.reverse();
    frame.teleported.extend(entities);
}

#[test]
fn teleported_keeps_what_an_earlier_extractor_added() {
    let mut app = App::new();
    app.extractor(teleport_everything)
        .plugin(RenderPlugin)
        .render_layer("world", 1.0);
    let mut simulation = app.build().unwrap();
    let first = spawn(&mut simulation, Some(Vec2::ZERO));
    let second = spawn(&mut simulation, Some(Vec2::ZERO));
    simulation
        .world_mut()
        .insert(second, Teleported { tick: 0 })
        .unwrap();
    assert_eq!(frame(&simulation).teleported, [first, second]);
}

const SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/boxes.scene.ron"
);
const INPUTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/boxes.inputs"
);
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/boxes.trace");

#[test]
fn plugin_does_not_change_boxes_hashes() {
    let mut app = boxes::app();
    app.plugin(RenderPlugin);
    let mut simulation = app.build().unwrap();
    simulation
        .load_scene(&std::fs::read_to_string(SCENE).unwrap())
        .unwrap();
    let log = InputLog::decode(&std::fs::read(INPUTS).unwrap()).unwrap();
    let golden = Trace::decode(&std::fs::read(GOLDEN).unwrap()).unwrap();
    let mut hashes = vec![simulation.hash()];
    for tick in 0..golden.hashes.len() as u64 - 1 {
        simulation.step_encoded(log.tick(tick)).unwrap();
        hashes.push(simulation.hash());
    }
    assert_eq!(hashes, golden.hashes);
}

#[test]
fn tilemap_whose_tiles_do_not_match_its_size_is_skipped_with_a_warning() {
    let mut simulation = simulation();
    let entity = spawn(&mut simulation, Some(Vec2::ZERO));
    let tilemap = Tilemap {
        tileset: AssetId(9),
        cell_size: Vec2::new(1.0, 1.0),
        width: 3,
        height: 2,
        tiles: vec![Some(0); 4],
        layer: "world".into(),
        order: 0,
    };
    simulation.world_mut().insert(entity, tilemap).unwrap();
    let frame = frame(&simulation);
    assert!(frame.tilemaps.is_empty());
    assert_eq!(
        frame.warnings,
        [format!(
            "entity {entity}: tilemap is 3 by 2 cells, so it needs 6 tiles, but it has 4; it is not drawn"
        )]
    );
}
