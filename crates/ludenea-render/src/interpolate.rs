//! The rules of interpolation between two render frames, GPU-free: what blends from the
//! previous tick to the current one, and how. The renderer uploads both ticks' transforms
//! once per tick and blends them by `alpha` on every draw: in the shader for sprites, in the
//! views for the camera and tilemaps. Display only: nothing here feeds back into the
//! simulation.

use std::collections::{BTreeMap, BTreeSet};

use ludenea_draw::frame::{CameraItem, CameraView, RenderFrame, SpriteItem, TilemapItem};
use ludenea_ecs::Entity;
use ludenea_math::{Rot, Vec2, clamp};

/// `alpha` clamped to 0..=1, NaN as 1: 0 draws the previous tick, 1 the current one.
pub(crate) fn clamp_alpha(alpha: f32) -> f32 {
    if alpha.is_nan() {
        1.0
    } else {
        clamp(alpha, 0.0, 1.0)
    }
}

/// A sprite's transform at both ticks: its centre at the previous and the current tick, its
/// rotation at the previous tick (unit length), and the arc to the current one along the
/// shorter way. A sprite that does not blend has both centres and the rotation of `current`,
/// as given, and no arc.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpriteMotion {
    pub from: Vec2,
    pub to: Vec2,
    pub rotation: Rot,
    pub arc: f32,
}

impl SpriteMotion {
    /// The centre and rotation `alpha` of the way, as the shader computes them.
    #[cfg(test)]
    fn at(&self, alpha: f32) -> (Vec2, Rot) {
        let alpha = clamp_alpha(alpha);
        let center = Vec2::lerp(self.from, self.to, alpha);
        (center, self.rotation.mul(Rot::from_angle(self.arc * alpha)))
    }
}

/// What `current` blends from: the entities of `previous`, but not those teleported on
/// `current`'s tick. An entity missing from `previous` (or with no `previous` at all) is
/// drawn where `current` has it.
pub(crate) struct Pairing<'a> {
    sprites: BTreeMap<Entity, &'a SpriteItem>,
    tilemaps: BTreeMap<Entity, Vec2>,
    camera: Option<&'a CameraItem>,
    teleported: BTreeSet<Entity>,
}

impl<'a> Pairing<'a> {
    pub fn new(previous: Option<&'a RenderFrame>, current: &RenderFrame) -> Self {
        let (sprites, tilemaps) = match previous {
            Some(previous) => (
                previous
                    .sprites
                    .iter()
                    .map(|item| (item.entity, item))
                    .collect(),
                previous
                    .tilemaps
                    .iter()
                    .map(|item| (item.entity, item.position))
                    .collect(),
            ),
            None => Default::default(),
        };
        Pairing {
            sprites,
            tilemaps,
            camera: previous.and_then(|frame| frame.camera.as_ref()),
            teleported: current.teleported.iter().copied().collect(),
        }
    }

    /// The motion of `item`, a sprite of `current`.
    pub fn sprite(&self, item: &SpriteItem) -> SpriteMotion {
        match self.sprites.get(&item.entity) {
            Some(before) if !self.teleported.contains(&item.entity) => {
                let rotation = before.rotation.normalize();
                SpriteMotion {
                    from: before.position,
                    to: item.position,
                    rotation,
                    arc: rotation.inv_mul(item.rotation.normalize()).angle(),
                }
            }
            _ => SpriteMotion {
                from: item.position,
                to: item.position,
                rotation: item.rotation,
                arc: 0.0,
            },
        }
    }

    /// Where `item`, a tilemap of `current`, is at the previous tick and at the current one.
    pub fn tilemap(&self, item: &TilemapItem) -> (Vec2, Vec2) {
        match self.tilemaps.get(&item.entity) {
            Some(&before) if !self.teleported.contains(&item.entity) => (before, item.position),
            _ => (item.position, item.position),
        }
    }

    /// The camera of `current` at the previous tick and at the current one, or `None` when
    /// `current` has none. It does not blend when `previous` has no camera, a camera of
    /// another entity (a switched camera), or when it was teleported.
    pub fn camera(&self, current: &RenderFrame) -> Option<(CameraView, CameraView)> {
        let camera = current.camera.as_ref()?;
        let to = CameraView::from(camera);
        let from = match self.camera {
            Some(before)
                if before.entity == camera.entity && !self.teleported.contains(&camera.entity) =>
            {
                CameraView::from(before)
            }
            _ => to.clone(),
        };
        Some((from, to))
    }
}

/// The camera `alpha` (already clamped) of the way from `from` to `to`.
pub(crate) fn camera_at(from: &CameraView, to: &CameraView, alpha: f32) -> CameraView {
    CameraView {
        center: Vec2::lerp(from.center, to.center, alpha),
        view: Vec2::lerp(from.view, to.view, alpha),
    }
}

#[cfg(test)]
mod tests {
    use ludenea_draw::Color;
    use ludenea_draw::frame::{CameraItem, CameraView, RenderFrame, SpriteItem, TilemapItem};
    use ludenea_ecs::{AssetId, Entity, World};
    use ludenea_math::{PI, Rot, Vec2};

    use super::{Pairing, camera_at, clamp_alpha};

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
    fn alpha_is_clamped_and_nan_is_the_current_tick() {
        assert_eq!(clamp_alpha(0.25), 0.25);
        assert_eq!(clamp_alpha(3.0), 1.0);
        assert_eq!(clamp_alpha(-1.0), 0.0);
        assert_eq!(clamp_alpha(f32::NAN), 1.0);
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
        let pairing = Pairing::new(Some(&previous), &current);
        let motion = pairing.sprite(&current.sprites[0]);
        assert_eq!(motion.at(0.5).0, Vec2::new(1.0, -2.0));
        assert_eq!(motion.at(1.0), (Vec2::new(2.0, -4.0), Rot::IDENTITY));
        assert_eq!(motion.at(0.0).0, Vec2::new(0.0, 0.0));
        assert_eq!(
            pairing.tilemap(&current.tilemaps[0]),
            (Vec2::new(-4.0, 2.0), Vec2::new(0.0, 0.0))
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
        let motion = Pairing::new(Some(&previous), &current).sprite(&current.sprites[0]);
        assert!(close(motion.arc, 20.0 * degrees), "{motion:?}");
        let (_, rotation) = motion.at(0.5);
        assert!(
            close(rotation.cos, -1.0) && close(rotation.sin, 0.0),
            "{rotation:?}"
        );
        let (_, quarter) = motion.at(0.25);
        assert!(close(quarter.angle().abs(), 175.0 * degrees), "{quarter:?}");
        assert!(close(
            quarter.cos * quarter.cos + quarter.sin * quarter.sin,
            1.0
        ));
    }

    #[test]
    fn a_rotation_that_is_not_unit_length_blends_as_its_unit_rotation() {
        let entity = World::new().spawn();
        let previous = frame(
            1,
            vec![sprite(entity, Vec2::ZERO, Rot { cos: 2.0, sin: 0.0 })],
            Vec::new(),
        );
        let current = frame(
            2,
            vec![sprite(entity, Vec2::ZERO, Rot { cos: 0.0, sin: 3.0 })],
            Vec::new(),
        );
        let motion = Pairing::new(Some(&previous), &current).sprite(&current.sprites[0]);
        assert_eq!(motion.rotation, Rot::IDENTITY);
        assert!(close(motion.arc, PI / 2.0), "{motion:?}");
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
        let pairing = Pairing::new(Some(&previous), &current);
        let motion = pairing.sprite(&current.sprites[0]);
        assert_eq!(
            motion.at(0.5),
            (current.sprites[0].position, current.sprites[0].rotation)
        );
        assert_eq!(motion.arc, 0.0);
        assert_eq!(
            pairing.tilemap(&current.tilemaps[0]),
            (Vec2::new(0.0, 10.0), Vec2::new(0.0, 10.0))
        );
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
        for previous in [Some(&previous), None] {
            let pairing = Pairing::new(previous, &current);
            let motion = pairing.sprite(&current.sprites[1]);
            assert_eq!(
                motion.at(0.5),
                (Vec2::new(5.0, 5.0), current.sprites[1].rotation)
            );
            assert_eq!(
                pairing.tilemap(&current.tilemaps[0]),
                (Vec2::new(3.0, 3.0), Vec2::new(3.0, 3.0))
            );
        }
    }

    fn camera(entity: Entity, center: Vec2, view: Vec2) -> Option<CameraItem> {
        Some(CameraItem {
            entity,
            center,
            view,
        })
    }

    /// The camera of `current` blended from `previous` at `alpha`.
    fn blended(previous: &RenderFrame, current: &RenderFrame, alpha: f32) -> Option<CameraView> {
        Pairing::new(Some(previous), current)
            .camera(current)
            .map(|(from, to)| camera_at(&from, &to, alpha))
    }

    fn view(center: Vec2, view: Vec2) -> Option<CameraView> {
        Some(CameraView { center, view })
    }

    #[test]
    fn camera_is_blended() {
        let entity = World::new().spawn();
        let mut previous = frame(1, Vec::new(), Vec::new());
        previous.camera = camera(entity, Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
        let mut current = frame(2, Vec::new(), Vec::new());
        current.camera = camera(entity, Vec2::new(4.0, -2.0), Vec2::new(20.0, 30.0));
        assert_eq!(
            blended(&previous, &current, 0.25),
            view(Vec2::new(1.0, -0.5), Vec2::new(12.5, 15.0))
        );
        // A camera that appears or disappears is not blended.
        let without = frame(1, Vec::new(), Vec::new());
        assert_eq!(
            blended(&without, &current, 0.25),
            view(Vec2::new(4.0, -2.0), Vec2::new(20.0, 30.0))
        );
        assert_eq!(blended(&previous, &without, 0.25), None);
    }

    #[test]
    fn teleported_camera_is_not_blended() {
        let entity = World::new().spawn();
        let mut previous = frame(1, Vec::new(), Vec::new());
        previous.camera = camera(entity, Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
        let mut current = frame(2, Vec::new(), Vec::new());
        current.camera = camera(entity, Vec2::new(40.0, 0.0), Vec2::new(20.0, 20.0));
        current.teleported = vec![entity];
        assert_eq!(
            blended(&previous, &current, 0.5),
            view(Vec2::new(40.0, 0.0), Vec2::new(20.0, 20.0))
        );
    }

    #[test]
    fn switched_camera_is_not_blended() {
        let mut world = World::new();
        let (old, new) = (world.spawn(), world.spawn());
        let mut previous = frame(1, Vec::new(), Vec::new());
        previous.camera = camera(old, Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
        let mut current = frame(2, Vec::new(), Vec::new());
        current.camera = camera(new, Vec2::new(40.0, 0.0), Vec2::new(20.0, 20.0));
        assert_eq!(
            blended(&previous, &current, 0.5),
            view(Vec2::new(40.0, 0.0), Vec2::new(20.0, 20.0))
        );
    }
}
