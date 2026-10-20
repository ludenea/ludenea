//! Interpolation between two render frames, GPU-free: what to draw a fraction of the way from
//! one tick to the next. Display only: nothing here feeds back into the simulation.

use std::collections::{BTreeMap, BTreeSet};

use ludenea_draw::frame::{CameraView, RenderFrame};
use ludenea_ecs::Entity;
use ludenea_math::{Rot, Vec2, clamp};

/// `current`, with sprite and tilemap positions, sprite rotations and the camera blended from
/// `previous` by `alpha`: 0 is `previous`, 1 (or NaN) is `current`, and anything outside is
/// clamped. An entity missing from `previous`, or listed in `current.teleported`, is drawn
/// where `current` has it; so is a camera that only one of the frames has.
pub fn interpolate(previous: &RenderFrame, current: &RenderFrame, alpha: f32) -> RenderFrame {
    let alpha = if alpha.is_nan() {
        1.0
    } else {
        clamp(alpha, 0.0, 1.0)
    };
    let teleported: BTreeSet<Entity> = current.teleported.iter().copied().collect();
    let mut frame = current.clone();

    let sprites: BTreeMap<Entity, _> = previous
        .sprites
        .iter()
        .map(|item| (item.entity, item))
        .collect();
    for item in &mut frame.sprites {
        if let Some(before) = sprites.get(&item.entity)
            && !teleported.contains(&item.entity)
        {
            item.position = Vec2::lerp(before.position, item.position, alpha);
            item.rotation = slerp(before.rotation, item.rotation, alpha);
        }
    }

    let tilemaps: BTreeMap<Entity, Vec2> = previous
        .tilemaps
        .iter()
        .map(|item| (item.entity, item.position))
        .collect();
    for item in &mut frame.tilemaps {
        if let Some(&before) = tilemaps.get(&item.entity)
            && !teleported.contains(&item.entity)
        {
            item.position = Vec2::lerp(before, item.position, alpha);
        }
    }

    if let (Some(before), Some(camera)) = (&previous.camera, &mut frame.camera) {
        *camera = CameraView {
            center: Vec2::lerp(before.center, camera.center, alpha),
            view: Vec2::lerp(before.view, camera.view, alpha),
        };
    }
    frame
}

/// The rotation `alpha` of the way from `from` to `to` along the shorter arc, unit length.
fn slerp(from: Rot, to: Rot, alpha: f32) -> Rot {
    let arc = from.inv_mul(to).angle();
    from.mul(Rot::from_angle(arc * alpha)).normalize()
}

#[cfg(test)]
mod tests {
    use ludenea_draw::Color;
    use ludenea_draw::frame::{CameraView, RenderFrame, SpriteItem, TilemapItem};
    use ludenea_ecs::{AssetId, Entity, World};
    use ludenea_math::{PI, Rot, Vec2};

    use super::interpolate;

    fn sprite(entity: Entity, position: Vec2, rotation: Rot) -> SpriteItem {
        SpriteItem {
            entity,
            layer: 0,
            order: 0,
            texture: AssetId(1),
            frame: 0,
            position,
            rotation,
            flip_x: false,
            flip_y: false,
            tint: Color(0xFFFFFFFF),
        }
    }

    fn tilemap(entity: Entity, position: Vec2) -> TilemapItem {
        TilemapItem {
            entity,
            layer: 0,
            order: 0,
            tileset: AssetId(2),
            position,
            cell_size: Vec2::new(1.0, 1.0),
            width: 1,
            height: 1,
            tiles: vec![Some(0)],
        }
    }

    fn frame(tick: u64, sprites: Vec<SpriteItem>, tilemaps: Vec<TilemapItem>) -> RenderFrame {
        RenderFrame {
            tick,
            camera: None,
            layers: Vec::new(),
            sprites,
            tilemaps,
            teleported: Vec::new(),
            debug: Default::default(),
            warnings: Vec::new(),
        }
    }

    /// Within the precision of `ludenea_math`'s angle approximations.
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-2
    }

    #[test]
    fn halfway_blends_positions() {
        let mut world = World::new();
        let (a, b) = (world.spawn(), world.spawn());
        let previous = frame(
            1,
            vec![sprite(a, Vec2::new(0.0, 0.0), Rot::IDENTITY)],
            vec![tilemap(b, Vec2::new(-4.0, 2.0))],
        );
        let current = frame(
            2,
            vec![sprite(a, Vec2::new(2.0, -4.0), Rot::IDENTITY)],
            vec![tilemap(b, Vec2::new(0.0, 0.0))],
        );
        let blended = interpolate(&previous, &current, 0.5);
        assert_eq!(blended.tick, 2);
        assert_eq!(blended.sprites[0].position, Vec2::new(1.0, -2.0));
        assert_eq!(blended.tilemaps[0].position, Vec2::new(-2.0, 1.0));
        // Alpha is clamped: past 1 is `current`, below 0 is `previous`, NaN is `current`.
        assert_eq!(
            interpolate(&previous, &current, 3.0).sprites[0].position,
            Vec2::new(2.0, -4.0)
        );
        assert_eq!(
            interpolate(&previous, &current, -1.0).sprites[0].position,
            Vec2::new(0.0, 0.0)
        );
        assert_eq!(
            interpolate(&previous, &current, f32::NAN).sprites[0].position,
            Vec2::new(2.0, -4.0)
        );
    }

    #[test]
    fn rotation_takes_the_shorter_arc() {
        let entity = World::new().spawn();
        // From 170 to -170 degrees: the short way crosses 180, not 0.
        let degrees = PI / 180.0;
        let previous = frame(
            1,
            vec![sprite(entity, Vec2::ZERO, Rot::from_angle(170.0 * degrees))],
            Vec::new(),
        );
        let current = frame(
            2,
            vec![sprite(
                entity,
                Vec2::ZERO,
                Rot::from_angle(-170.0 * degrees),
            )],
            Vec::new(),
        );
        let rotation = interpolate(&previous, &current, 0.5).sprites[0].rotation;
        assert!(
            close(rotation.cos, -1.0) && close(rotation.sin, 0.0),
            "{rotation:?}"
        );
        let quarter = interpolate(&previous, &current, 0.25).sprites[0].rotation;
        assert!(close(quarter.angle().abs(), 175.0 * degrees), "{quarter:?}");
        assert!(close(
            quarter.cos * quarter.cos + quarter.sin * quarter.sin,
            1.0
        ));
    }

    #[test]
    fn teleported_entity_is_not_blended() {
        let mut world = World::new();
        let (a, b) = (world.spawn(), world.spawn());
        let previous = frame(
            1,
            vec![sprite(a, Vec2::new(0.0, 0.0), Rot::IDENTITY)],
            vec![tilemap(b, Vec2::new(0.0, 0.0))],
        );
        let mut current = frame(
            2,
            vec![sprite(a, Vec2::new(10.0, 0.0), Rot::from_angle(1.0))],
            vec![tilemap(b, Vec2::new(0.0, 10.0))],
        );
        current.teleported = vec![a, b];
        let blended = interpolate(&previous, &current, 0.5);
        assert_eq!(blended.sprites, current.sprites);
        assert_eq!(blended.tilemaps, current.tilemaps);
    }

    #[test]
    fn new_entity_appears_at_its_current_position() {
        let mut world = World::new();
        let (old, new, map) = (world.spawn(), world.spawn(), world.spawn());
        let previous = frame(
            1,
            vec![sprite(old, Vec2::new(0.0, 0.0), Rot::IDENTITY)],
            Vec::new(),
        );
        let current = frame(
            2,
            vec![
                sprite(old, Vec2::new(2.0, 0.0), Rot::IDENTITY),
                sprite(new, Vec2::new(5.0, 5.0), Rot::from_angle(1.0)),
            ],
            vec![tilemap(map, Vec2::new(3.0, 3.0))],
        );
        let blended = interpolate(&previous, &current, 0.5);
        assert_eq!(blended.sprites[0].position, Vec2::new(1.0, 0.0));
        assert_eq!(blended.sprites[1], current.sprites[1]);
        assert_eq!(blended.tilemaps, current.tilemaps);
    }

    #[test]
    fn camera_is_blended() {
        let mut previous = frame(1, Vec::new(), Vec::new());
        previous.camera = Some(CameraView {
            center: Vec2::new(0.0, 0.0),
            view: Vec2::new(10.0, 10.0),
        });
        let mut current = frame(2, Vec::new(), Vec::new());
        current.camera = Some(CameraView {
            center: Vec2::new(4.0, -2.0),
            view: Vec2::new(20.0, 30.0),
        });
        assert_eq!(
            interpolate(&previous, &current, 0.25).camera,
            Some(CameraView {
                center: Vec2::new(1.0, -0.5),
                view: Vec2::new(12.5, 15.0),
            })
        );
        // A camera that appears or disappears is not blended.
        let without = frame(1, Vec::new(), Vec::new());
        assert_eq!(interpolate(&without, &current, 0.25).camera, current.camera);
        assert_eq!(interpolate(&previous, &without, 0.25).camera, None);
    }
}
