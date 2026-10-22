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
    #[serde(with = "compact_asset")]
    #[cfg_attr(feature = "ts", ts(as = "AssetId"))]
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
    #[serde(with = "compact_asset")]
    #[cfg_attr(feature = "ts", ts(as = "AssetId"))]
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

/// An asset identifier in a frame: its 16 little-endian bytes in a binary format, against 33
/// for its text in postcard (a quarter of a sprite's encoding), and its hexadecimal text in a
/// readable one.
mod compact_asset {
    use ludenea_ecs::AssetId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(id: &AssetId, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            id.serialize(serializer)
        } else {
            id.0.to_le_bytes().serialize(serializer)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<AssetId, D::Error> {
        if deserializer.is_human_readable() {
            AssetId::deserialize(deserializer)
        } else {
            <[u8; 16]>::deserialize(deserializer).map(|bytes| AssetId(u128::from_le_bytes(bytes)))
        }
    }
}

impl RenderFrame {
    /// The index of the layer named `name`, what `SpriteItem::layer` holds.
    pub fn layer_index(&self, name: &str) -> Option<u16> {
        self.layers
            .iter()
            .position(|layer| layer.name == name)
            .and_then(|index| u16::try_from(index).ok())
    }

    /// Whether this frame is of the tick right after `previous`'s, so a renderer blends
    /// between them; after a gap (a step of several ticks, a seek, frames left undrawn) it
    /// shows this frame as is.
    pub fn follows(&self, previous: &RenderFrame) -> bool {
        previous.tick.checked_add(1) == Some(self.tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;

    #[test]
    fn only_the_very_next_tick_follows() {
        let at = |tick| RenderFrame { tick, ..frame() };
        assert!(at(43).follows(&at(42)));
        assert!(!at(44).follows(&at(42)), "a gap of a tick");
        assert!(!at(42).follows(&at(42)), "the same tick again");
        assert!(!at(0).follows(&at(42)), "a seek back");
        assert!(!at(0).follows(&at(u64::MAX)), "no wrap around");
    }

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
    fn render_frame_round_trips_through_postcard() {
        let frame = frame();
        let bytes = postcard::to_allocvec(&frame).unwrap();
        let back: RenderFrame = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn asset_ids_take_sixteen_bytes_in_postcard_and_stay_hexadecimal_in_json() {
        let mut frame = frame();
        let id = AssetId(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210);
        frame.sprites[0].texture = id;
        frame.tilemaps[0].tileset = id;
        let raw = id.0.to_le_bytes();
        let holds_raw = |bytes: &[u8]| bytes.windows(16).any(|window| window == raw);
        let sprite = postcard::to_allocvec(&frame.sprites[0]).unwrap();
        assert!(holds_raw(&sprite), "{sprite:?}");
        // Its other fields: entity 2, layer, order, frame, flips (one byte each), position and
        // rotation (four floats), tint (a varint of up to five bytes).
        assert!(
            sprite.len() <= 16 + 2 + 5 + 16 + 5,
            "{} bytes",
            sprite.len()
        );
        let tilemap = postcard::to_allocvec(&frame.tilemaps[0]).unwrap();
        assert!(holds_raw(&tilemap), "{tilemap:?}");

        let back: RenderFrame =
            postcard::from_bytes(&postcard::to_allocvec(&frame).unwrap()).unwrap();
        assert_eq!(back, frame);
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["sprites"][0]["texture"], id.to_string());
        assert_eq!(json["tilemaps"][0]["tileset"], id.to_string());
    }

    #[test]
    fn primitives_in_json_stay_tagged_with_their_type() {
        let json = serde_json::to_value(&frame().debug).unwrap();
        assert_eq!(
            json["layers"]["physics.colliders"][0]["primitive"],
            serde_json::json!({
                "type": "line",
                "a": { "x": 0.0, "y": 0.0 },
                "b": { "x": 1.0, "y": 2.0 },
                "color": 0x112233FF_u32,
            })
        );
    }

    #[test]
    fn layer_index_finds_declared_layers() {
        let frame = frame();
        assert_eq!(frame.layer_index("background"), Some(0));
        assert_eq!(frame.layer_index("world"), Some(1));
        assert_eq!(frame.layer_index("foreground"), None);
    }
}
