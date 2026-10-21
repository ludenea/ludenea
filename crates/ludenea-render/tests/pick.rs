//! What a click hits and what a selection spans, GPU-free: sprites by their drawn quad and
//! tilemaps by their painted cells, each seen through its layer's view.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use ludenea_app::meta::Filter;
use ludenea_draw::Color;
use ludenea_draw::frame::{
    CameraItem, CameraView, LayerInfo, RenderFrame, SpriteItem, TilemapItem,
};
use ludenea_ecs::{AssetId, Entity, World};
use ludenea_math::{Rot, Vec2};
use ludenea_render::atlas::Region;
use ludenea_render::camera::{Size, fit, screen_to_world, world_to_clip};
use ludenea_render::pick::{bounds, pick};

/// 64 by 64 pixels showing 4 by 4 world units around the camera: 16 pixels per unit, the
/// camera's centre at pixel (32, 32), +y up.
const SIZE: Size = Size {
    width: 64,
    height: 64,
};
const TEXTURE: AssetId = AssetId(1);

thread_local! {
    static ENTITIES: RefCell<World> = RefCell::new(World::new());
}

/// A new entity, each after the previous one in entity order.
fn entity() -> Entity {
    ENTITIES.with_borrow_mut(World::spawn)
}

fn camera(center: Vec2) -> CameraView {
    CameraView {
        center,
        view: Vec2::new(4.0, 4.0),
    }
}

/// The pixel where world `point` is drawn on a layer moving with the world, the camera at
/// the origin.
fn screen(point: Vec2) -> Vec2 {
    Vec2::new(32.0 + 16.0 * point.x, 32.0 - 16.0 * point.y)
}

/// Frame 0 of `TEXTURE`: `width` by `height` pixels at 16 per unit, its pivot at `pivot`.
fn regions(width: u32, height: u32, pivot: (u32, u32)) -> BTreeMap<(AssetId, u32), Region> {
    BTreeMap::from([(
        (TEXTURE, 0),
        Region {
            page: 0,
            x: 1,
            y: 1,
            width,
            height,
            pivot,
            pixels_per_unit: 16,
            filter: Filter::Nearest,
        },
    )])
}

fn sprite(entity: Entity, position: Vec2) -> SpriteItem {
    SpriteItem {
        entity,
        layer: 1,
        order: 0,
        texture: TEXTURE,
        frame: 0,
        position,
        rotation: Rot::IDENTITY,
        flip_x: false,
        flip_y: false,
        tint: Color(0xFFFFFFFF),
    }
}

/// A 2 by 2 tilemap from (-1, -1) whose bottom-left and top-right cells are painted.
fn tilemap(entity: Entity) -> TilemapItem {
    TilemapItem {
        entity,
        layer: 1,
        order: 0,
        tileset: AssetId(2),
        position: Vec2::new(-1.0, -1.0),
        cell_size: Vec2::new(1.0, 1.0),
        width: 2,
        height: 2,
        tiles: vec![Some(0), None, None, Some(3)],
    }
}

/// A frame of a background layer of `parallax` and the world layer (parallax 1).
fn frame(parallax: f32, sprites: Vec<SpriteItem>, tilemaps: Vec<TilemapItem>) -> RenderFrame {
    RenderFrame {
        tick: 0,
        camera: Some(CameraItem {
            entity: entity(),
            center: Vec2::ZERO,
            view: Vec2::new(4.0, 4.0),
        }),
        layers: vec![
            LayerInfo {
                name: "background".into(),
                parallax,
            },
            LayerInfo {
                name: "world".into(),
                parallax: 1.0,
            },
        ],
        sprites,
        tilemaps,
        teleported: Vec::new(),
        debug: Default::default(),
        warnings: Vec::new(),
    }
}

/// What a click at world `point` hits, the camera at the origin.
fn at(
    frame: &RenderFrame,
    regions: &BTreeMap<(AssetId, u32), Region>,
    point: (f32, f32),
) -> Vec<Entity> {
    let point = Vec2::new(point.0, point.1);
    pick(frame, regions, screen(point), SIZE, camera(Vec2::ZERO))
}

#[test]
fn pick_hits_a_sprite_by_its_drawn_quad() {
    // 32 by 16 pixels at 16 per unit, pivot at the centre: 2 by 1 units around (0, 0).
    let regions = regions(32, 16, (16, 8));
    let hero = entity();
    let frame = frame(1.0, vec![sprite(hero, Vec2::ZERO)], Vec::new());

    for point in [(0.0, 0.0), (0.9, 0.0), (-0.9, 0.4), (0.0, -0.4)] {
        assert_eq!(at(&frame, &regions, point), [hero], "{point:?}");
    }
    for point in [(1.1, 0.0), (-1.1, 0.0), (0.0, 0.6), (0.0, -0.6)] {
        assert_eq!(at(&frame, &regions, point), [], "{point:?}");
    }
}

#[test]
fn pick_respects_flip_pivot_and_rotation() {
    // 32 by 16 pixels, the pivot on the bottom-left corner: the quad spans 0 to 2 units right
    // of the position and 0 to 1 up.
    let regions = regions(32, 16, (0, 16));
    let hero = entity();
    let pick_with = |change: &dyn Fn(&mut SpriteItem), point| {
        let mut item = sprite(hero, Vec2::ZERO);
        change(&mut item);
        !at(&frame(1.0, vec![item], Vec::new()), &regions, point).is_empty()
    };

    let unchanged = |_: &mut SpriteItem| {};
    assert!(pick_with(&unchanged, (1.5, 0.5)));
    assert!(!pick_with(&unchanged, (-0.5, 0.5)));

    // Flipped, the quad is mirrored around the pivot.
    let flip_x = |item: &mut SpriteItem| item.flip_x = true;
    assert!(pick_with(&flip_x, (-1.5, 0.5)));
    assert!(!pick_with(&flip_x, (1.5, 0.5)));
    let flip_y = |item: &mut SpriteItem| item.flip_y = true;
    assert!(pick_with(&flip_y, (1.5, -0.5)));
    assert!(!pick_with(&flip_y, (1.5, 0.5)));

    // A quarter turn counterclockwise around the pivot: 0 to 2 units up and 0 to 1 left.
    let turned = |item: &mut SpriteItem| item.rotation = Rot { cos: 0.0, sin: 1.0 };
    assert!(pick_with(&turned, (-0.5, 1.5)));
    assert!(!pick_with(&turned, (1.5, 0.5)));
}

#[test]
fn pick_returns_the_topmost_first() {
    let regions = regions(16, 16, (8, 8));
    let (first, second, third, fourth, elsewhere) =
        (entity(), entity(), entity(), entity(), entity());
    let mut over_the_background = sprite(first, Vec2::new(-0.5, -0.5));
    over_the_background.order = 0;
    let mut below_on_the_world = sprite(second, Vec2::new(-0.5, -0.5));
    below_on_the_world.order = -1;
    // On the layer drawn first, whatever its order.
    let mut background = sprite(third, Vec2::new(-0.5, -0.5));
    background.layer = 0;
    background.order = 9;
    // The tilemap's painted cell under (-0.5, -0.5), at the same layer and order as `first`,
    // drawn after it as its entity comes later.
    let mut cells = tilemap(fourth);
    cells.order = 0;
    let far = sprite(elsewhere, Vec2::new(10.0, 10.0));
    let frame = frame(
        1.0,
        vec![background, over_the_background, far, below_on_the_world],
        vec![cells],
    );

    assert_eq!(
        at(&frame, &regions, (-0.5, -0.5)),
        [fourth, first, second, third]
    );
}

#[test]
fn pick_on_a_parallax_layer_uses_its_view() {
    let regions = regions(16, 16, (8, 8));
    let (fixed, moving) = (entity(), entity());
    let mut on_the_screen = sprite(fixed, Vec2::ZERO);
    on_the_screen.layer = 0;
    let frame = frame(
        0.0,
        vec![on_the_screen, sprite(moving, Vec2::ZERO)],
        Vec::new(),
    );
    // The camera 2 units right: the layer fixed to the screen still shows its sprite in the
    // middle, while the world's sprite is 2 units left of it.
    let camera = camera(Vec2::new(2.0, 0.0));

    let middle = Vec2::new(32.0, 32.0);
    assert_eq!(
        pick(&frame, &regions, middle, SIZE, camera.clone()),
        [fixed]
    );
    let left = Vec2::new(4.0, 32.0);
    assert_eq!(pick(&frame, &regions, left, SIZE, camera), [moving]);
}

#[test]
fn pick_hits_a_painted_tile_and_misses_an_empty_cell() {
    let regions = BTreeMap::new();
    let cells = entity();
    let malformed = entity();
    // One tile for four cells: not drawn, so never hit.
    let mut short = tilemap(malformed);
    short.tiles.truncate(1);
    let frame = frame(1.0, Vec::new(), vec![tilemap(cells), short]);

    assert_eq!(at(&frame, &regions, (-0.5, -0.5)), [cells]);
    assert_eq!(at(&frame, &regions, (0.5, 0.5)), [cells]);
    assert_eq!(at(&frame, &regions, (0.5, -0.5)), [], "empty cell");
    assert_eq!(at(&frame, &regions, (-0.5, 0.5)), [], "empty cell");
    assert_eq!(at(&frame, &regions, (1.5, 0.5)), [], "outside the grid");
    assert_eq!(at(&frame, &regions, (-1.5, -0.5)), [], "outside the grid");
}

#[test]
fn missing_texture_placeholder_is_pickable() {
    // No region at all: the sprite is drawn as the 1 by 1 placeholder around its position.
    let regions = BTreeMap::new();
    let hero = entity();
    let frame = frame(1.0, vec![sprite(hero, Vec2::new(1.0, 1.0))], Vec::new());

    assert_eq!(at(&frame, &regions, (1.4, 0.6)), [hero]);
    assert_eq!(at(&frame, &regions, (1.6, 1.0)), []);
    assert_eq!(at(&frame, &regions, (1.0, 0.4)), []);
}

#[test]
fn bounds_cover_sprites_and_tilemaps_of_parallax_one() {
    // 2 by 1 units around the position.
    let regions = regions(32, 16, (16, 8));
    let (wide, turned, cells, distant, unselected) =
        (entity(), entity(), entity(), entity(), entity());
    let mut standing = sprite(turned, Vec2::new(0.0, 5.0));
    standing.rotation = Rot { cos: 0.0, sin: 1.0 };
    // On the background layer, of parallax 0.5: no single world box, so left out.
    let mut behind = sprite(distant, Vec2::new(100.0, 100.0));
    behind.layer = 0;
    let frame = frame(
        0.5,
        vec![
            sprite(wide, Vec2::new(3.0, 1.0)),
            standing,
            behind,
            sprite(unselected, Vec2::new(-50.0, 0.0)),
        ],
        vec![tilemap(cells)],
    );

    let selected = BTreeSet::from([wide, turned, cells, distant]);
    assert_eq!(
        bounds(&frame, &regions, &selected),
        Some((Vec2::new(-1.0, -1.0), Vec2::new(4.0, 6.0)))
    );
    assert_eq!(
        bounds(&frame, &regions, &BTreeSet::from([cells])),
        Some((Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0)))
    );
    assert_eq!(bounds(&frame, &regions, &BTreeSet::from([distant])), None);
    assert_eq!(bounds(&frame, &regions, &BTreeSet::new()), None);
}

#[test]
fn screen_to_world_inverts_world_to_clip() {
    // Twice as wide as tall, so the 4 by 4 view widens to 8 by 4.
    let size = Size {
        width: 64,
        height: 32,
    };
    let camera = CameraView {
        center: Vec2::new(3.0, -1.0),
        view: Vec2::new(4.0, 4.0),
    };
    let visible = fit(camera.view, 2.0);
    for parallax in [0.0, 0.5, 1.0] {
        for (x, y) in [(0.0, 0.0), (10.0, 20.0), (32.0, 16.0), (64.0, 32.0)] {
            let world = screen_to_world(Vec2::new(x, y), size, camera.clone(), parallax);
            let clip = world_to_clip(world, camera.clone(), visible, parallax);
            let expected = [2.0 * x / 64.0 - 1.0, 1.0 - 2.0 * y / 32.0];
            for axis in 0..2 {
                assert!(
                    (clip[axis] - expected[axis]).abs() < 1e-6,
                    "({x}, {y}) at parallax {parallax}: {clip:?} != {expected:?}"
                );
            }
        }
    }
    // The middle of the screen is the camera's centre seen through the parallax.
    let middle = screen_to_world(Vec2::new(32.0, 16.0), size, camera.clone(), 0.5);
    assert_eq!(middle, Vec2::new(1.5, -0.5));
}
