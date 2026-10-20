//! The render frame: everything a renderer draws for one tick, extracted from the simulation.
//! It is plain data, so the control protocol can carry it and a renderer needs no world.

use ludenea_ecs::{AssetId, Entity};
use ludenea_math::{Rot, Vec2};
use serde::{Deserialize, Serialize};

use crate::{Color, DebugDrawFrame};

/// What a renderer draws for one tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RenderFrame {
    pub tick: u64,
    pub camera: Option<CameraItem>,
    /// The app's render layers, in draw order: `SpriteItem::layer` indexes this list.
    pub layers: Vec<LayerInfo>,
    pub sprites: Vec<SpriteItem>,
    pub tilemaps: Vec<TilemapItem>,
    /// The entities that moved without travelling this tick, so they are not interpolated.
    pub teleported: Vec<Entity>,
    pub debug: DebugDrawFrame,
    pub warnings: Vec<String>,
}

/// The camera of a frame: the entity it belongs to, so a renderer can tell a camera that
/// moved from one that was switched or teleported, and where it looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CameraItem {
    pub entity: Entity,
    pub center: Vec2,
    /// The minimum visible width and height, in world units.
    pub view: Vec2,
}

/// Where a camera looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CameraView {
    pub center: Vec2,
    /// The minimum visible width and height, in world units.
    pub view: Vec2,
}

impl From<&CameraItem> for CameraView {
    fn from(camera: &CameraItem) -> Self {
        CameraView {
            center: camera.center,
            view: camera.view,
        }
    }
}

/// A render layer. Its draw order is its index in `RenderFrame::layers`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LayerInfo {
    pub name: String,
    /// How much the layer moves with the camera: 1.0 moves with the world, 0.0 is fixed to
    /// the screen.
    pub parallax: f32,
}

/// One sprite to draw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SpriteItem {
    pub entity: Entity,
    pub layer: u16,
    /// The draw order within the layer, lowest first.
    pub order: i32,
    pub texture: AssetId,
    pub frame: u32,
    pub position: Vec2,
    pub rotation: Rot,
    pub flip_x: bool,
    pub flip_y: bool,
    pub tint: Color,
}

/// One tilemap to draw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TilemapItem {
    pub entity: Entity,
    pub layer: u16,
    /// The draw order within the layer, lowest first.
    pub order: i32,
    pub tileset: AssetId,
    /// The bottom-left corner of the bottom-left cell.
    pub position: Vec2,
    pub cell_size: Vec2,
    pub width: u32,
    pub height: u32,
    /// The tileset frame of each cell, `None` for an empty cell, row-major from the
    /// bottom-left cell.
    pub tiles: Vec<Option<u32>>,
}

impl RenderFrame {
    /// The index of the layer named `name`, what `SpriteItem::layer` holds.
    pub fn layer_index(&self, name: &str) -> Option<u16> {
        self.layers
            .iter()
            .position(|layer| layer.name == name)
            .and_then(|index| u16::try_from(index).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;

    fn entity(index: u32, generation: u32) -> Entity {
        serde_json::from_value(serde_json::json!({ "index": index, "generation": generation }))
            .unwrap()
    }

    fn frame() -> RenderFrame {
        let sprite = entity(3, 1);
        let mut debug = DebugDrawFrame::default();
        debug.push(
            "physics.colliders",
            Some(sprite),
            Primitive::Line {
                a: Vec2::ZERO,
                b: Vec2::new(1.0, 2.0),
                color: Color(0x112233FF),
            },
        );
        RenderFrame {
            tick: 42,
            camera: Some(CameraItem {
                entity: entity(5, 0),
                center: Vec2::new(1.5, -2.0),
                view: Vec2::new(16.0, 9.0),
            }),
            layers: vec![
                LayerInfo {
                    name: "background".into(),
                    parallax: 0.5,
                },
                LayerInfo {
                    name: "world".into(),
                    parallax: 1.0,
                },
            ],
            sprites: vec![SpriteItem {
                entity: sprite,
                layer: 1,
                order: -2,
                texture: AssetId(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
                frame: 7,
                position: Vec2::new(0.25, 4.0),
                rotation: Rot::IDENTITY,
                flip_x: true,
                flip_y: false,
                tint: Color(0xFF8000C0),
            }],
            tilemaps: vec![TilemapItem {
                entity: entity(4, 0),
                layer: 0,
                order: 0,
                tileset: AssetId(9),
                position: Vec2::new(-8.0, -8.0),
                cell_size: Vec2::new(1.0, 1.0),
                width: 2,
                height: 1,
                tiles: vec![Some(0), None],
            }],
            teleported: vec![sprite],
            debug,
            warnings: vec!["a warning".into()],
        }
    }

    #[test]
    fn render_frame_round_trips_through_json() {
        let frame = frame();
        let json = serde_json::to_string(&frame).unwrap();
        let back: RenderFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn layer_index_finds_declared_layers() {
        let frame = frame();
        assert_eq!(frame.layer_index("background"), Some(0));
        assert_eq!(frame.layer_index("world"), Some(1));
        assert_eq!(frame.layer_index("foreground"), None);
    }
}
