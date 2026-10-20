use std::collections::BTreeMap;

use ludenea_app::pose::Pose;
use ludenea_app::{App, AssetTexts, Simulation};
use ludenea_draw::Color;
use ludenea_ecs::{AssetId, Handle};
use ludenea_math::{Rot, Vec2};
use ludenea_render::{
    Camera2d, Clip, RenderPlugin, Sprite, SpriteAnimation, SpriteSheet, Teleported, Tilemap,
};

const SHEET: AssetId = AssetId(0x51);

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(RenderPlugin).render_layer("world", 1.0);
    app.build().unwrap()
}

fn scene(components: &str) -> String {
    format!(
        "Scene(version: 2, next_id: 1, resources: {{}}, entities: [(id: 0, components: {{{components}}})])"
    )
}

fn load_error(components: &str, assets: &AssetTexts) -> String {
    let Err(error) = simulation().load_scene_with(&scene(components), assets) else {
        panic!("the scene loaded");
    };
    error.to_string()
}

fn sheet_text(clip: &str) -> String {
    format!(
        "AssetFile(type: \"ludenea::SpriteSheet\", value: (texture: \"{:032x}\", clips: {{\"run\": {clip}}}))",
        7
    )
}

#[test]
fn render_components_keep_their_stable_names() {
    use ludenea_ecs::{Asset, Component};
    assert_eq!(Sprite::NAME, "ludenea::Sprite");
    assert_eq!(SpriteAnimation::NAME, "ludenea::SpriteAnimation");
    assert_eq!(SpriteSheet::NAME, "ludenea::SpriteSheet");
    assert_eq!(Camera2d::NAME, "ludenea::Camera2d");
    assert_eq!(Tilemap::NAME, "ludenea::Tilemap");
    assert_eq!(Teleported::NAME, "ludenea::Teleported");
}

#[test]
fn a_default_sprite_is_white_on_the_first_layer() {
    let sprite = Sprite::default();
    assert_eq!(sprite.tint, Color(0xFFFFFFFF));
    assert_eq!(sprite.layer, "");
    assert_eq!((sprite.frame, sprite.order), (0, 0));
}

#[test]
fn zero_camera_view_is_refused_at_load() {
    let error = load_error(
        "\"ludenea::Camera2d\": (view: (x: 0.0, y: 9.0))",
        &AssetTexts::new(),
    );
    assert!(error.contains("ludenea::Camera2d"), "{error}");
    assert!(error.contains("entity"), "{error}");
    assert!(error.contains("view"), "{error}");
    for view in ["(x: -1.0, y: 9.0)", "(x: 16.0, y: inf)", "(x: NaN, y: 9.0)"] {
        let error = load_error(
            &format!("\"ludenea::Camera2d\": (view: {view})"),
            &AssetTexts::new(),
        );
        assert!(error.contains("view must be finite and above 0"), "{error}");
    }
}

#[test]
fn tilemap_with_wrong_tile_count_is_refused_naming_the_counts() {
    let error = load_error(
        &format!(
            "\"ludenea::Tilemap\": (tileset: \"{:032x}\", cell_size: (x: 1.0, y: 1.0), width: 2, height: 2, tiles: [Some(0), None, Some(1)], layer: \"world\", order: 0)",
            9
        ),
        &AssetTexts::new(),
    );
    assert!(error.contains("ludenea::Tilemap"), "{error}");
    assert!(
        error.contains("tiles has 3 entries, but width 2 by height 2 needs 4"),
        "{error}"
    );
    let error = load_error(
        &format!(
            "\"ludenea::Tilemap\": (tileset: \"{:032x}\", cell_size: (x: 0.0, y: 1.0), width: 1, height: 1, tiles: [None], layer: \"world\", order: 0)",
            9
        ),
        &AssetTexts::new(),
    );
    assert!(
        error.contains("cell_size must be finite and above 0"),
        "{error}"
    );
}

#[test]
fn clip_with_zero_ticks_per_frame_is_refused() {
    let animation = format!(
        "\"ludenea::SpriteAnimation\": (sheet: \"{SHEET}\", clip: \"run\", started: 0, looping: true)"
    );
    let refused = |clip: &str| {
        let assets: AssetTexts = [(SHEET, sheet_text(clip))].into();
        load_error(&animation, &assets)
    };
    let error = refused("(first: 0, count: 4, ticks_per_frame: 0)");
    assert!(error.contains(&SHEET.to_string()), "{error}");
    assert!(
        error.contains("clip `run`: ticks_per_frame must be at least 1"),
        "{error}"
    );
    let error = refused("(first: 0, count: 0, ticks_per_frame: 4)");
    assert!(
        error.contains("clip `run`: count must be at least 1"),
        "{error}"
    );
}

#[test]
fn render_components_round_trip_through_a_scene() {
    let mut original = simulation();
    let sheet = SpriteSheet {
        texture: AssetId(7),
        clips: BTreeMap::from([(
            "run".to_string(),
            Clip {
                first: 2,
                count: 4,
                ticks_per_frame: 5,
            },
        )]),
    };
    let world = original.world_mut();
    world
        .insert_asset_ron(
            SHEET,
            "ludenea::SpriteSheet",
            &ron::to_string(&sheet).unwrap(),
        )
        .unwrap();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Pose {
                position: Vec2::new(1.0, -2.0),
                rotation: Rot::IDENTITY,
            },
        )
        .unwrap();
    let sprite = Sprite {
        texture: AssetId(7),
        frame: 3,
        layer: "world".into(),
        order: -4,
        flip_x: true,
        flip_y: false,
        tint: Color(0x80FF40C0),
    };
    let animation = SpriteAnimation {
        sheet: Handle::new(SHEET),
        clip: "run".into(),
        started: 12,
        looping: false,
    };
    let camera = Camera2d {
        view: Vec2::new(16.0, 9.0),
    };
    let tilemap = Tilemap {
        tileset: AssetId(9),
        cell_size: Vec2::new(0.5, 0.5),
        width: 2,
        height: 1,
        tiles: vec![Some(1), None],
        layer: "world".into(),
        order: 1,
    };
    world.insert(entity, sprite.clone()).unwrap();
    world.insert(entity, animation.clone()).unwrap();
    world.insert(entity, camera).unwrap();
    world.insert(entity, tilemap.clone()).unwrap();
    world.insert(entity, Teleported { tick: 3 }).unwrap();

    let saved = original.save_scene().unwrap();
    let mut loaded = simulation();
    loaded
        .load_scene_with(&saved, &original.assets_ron())
        .unwrap();
    assert_eq!(loaded.hash(), original.hash());
    let world = loaded.world();
    let entity = world.entities().next().unwrap();
    assert_eq!(*world.get::<Sprite>(entity).unwrap(), sprite);
    assert_eq!(*world.get::<SpriteAnimation>(entity).unwrap(), animation);
    assert_eq!(*world.get::<Camera2d>(entity).unwrap(), camera);
    assert_eq!(*world.get::<Tilemap>(entity).unwrap(), tilemap);
    assert_eq!(
        *world.get::<Teleported>(entity).unwrap(),
        Teleported { tick: 3 }
    );
}
