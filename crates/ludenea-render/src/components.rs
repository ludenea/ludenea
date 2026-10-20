//! The simulated render components: what an entity draws and how the camera looks at the
//! world. They are part of the state hash, so a replay shows the same frames.

use ludenea_draw::Color;
use ludenea_ecs::{AssetId, Component, Handle};
use ludenea_math::Vec2;
use serde::{Deserialize, Serialize};

use crate::sheet::SpriteSheet;

/// A textured quad drawn at the entity's `Pose`. `frame` indexes the frames of `texture`.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[component(name = "ludenea::Sprite")]
pub struct Sprite {
    pub texture: AssetId,
    pub frame: u32,
    /// The render layer by name; empty means the first layer the app declares.
    pub layer: String,
    /// The draw order within the layer, lowest first.
    pub order: i32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub tint: Color,
}

impl Default for Sprite {
    /// Frame 0 of texture 0, untinted (white), on the first layer.
    fn default() -> Self {
        Sprite {
            texture: AssetId(0),
            frame: 0,
            layer: String::new(),
            order: 0,
            flip_x: false,
            flip_y: false,
            tint: Color(0xFFFFFFFF),
        }
    }
}

/// Plays `clip` of `sheet` on the entity's `Sprite` from the tick `started`: the `animate`
/// system writes the sprite's texture and frame every tick.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[component(name = "ludenea::SpriteAnimation")]
pub struct SpriteAnimation {
    pub sheet: Handle<SpriteSheet>,
    pub clip: String,
    pub started: u64,
    /// Whether the clip starts again after its last frame, rather than holding it.
    pub looping: bool,
}

/// The camera, centred on the entity's `Pose`. Deserialization rejects what
/// `validate` refuses.
#[derive(Component, Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
#[component(name = "ludenea::Camera2d")]
#[serde(try_from = "Camera2dData", into = "Camera2dData")]
pub struct Camera2d {
    /// The minimum visible width and height, in world units: the camera's zoom.
    pub view: Vec2,
}

/// Why `Camera2d::validate` refuses a camera.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum CameraError {
    #[error("view must be finite and above 0, got ({}, {})", .0.x, .0.y)]
    View(Vec2),
}

impl Camera2d {
    pub fn validate(&self) -> Result<(), CameraError> {
        if !finite_and_positive(self.view) {
            return Err(CameraError::View(self.view));
        }
        Ok(())
    }
}

/// Serialization mirror of `Camera2d` so loading can validate.
#[derive(Serialize, Deserialize)]
struct Camera2dData {
    view: Vec2,
}

impl From<Camera2d> for Camera2dData {
    fn from(camera: Camera2d) -> Self {
        Camera2dData { view: camera.view }
    }
}

impl TryFrom<Camera2dData> for Camera2d {
    type Error = CameraError;

    fn try_from(data: Camera2dData) -> Result<Self, CameraError> {
        let camera = Camera2d { view: data.view };
        camera.validate()?;
        Ok(camera)
    }
}

/// A grid of tiles whose bottom-left corner is the entity's `Pose`. Each cell holds a
/// frame of `tileset`, or nothing. Deserialization rejects what `validate` refuses.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[component(name = "ludenea::Tilemap")]
#[serde(try_from = "TilemapData", into = "TilemapData")]
pub struct Tilemap {
    pub tileset: AssetId,
    pub cell_size: Vec2,
    pub width: u32,
    pub height: u32,
    /// Row-major from the bottom-left cell: `width * height` cells.
    pub tiles: Vec<Option<u32>>,
    /// The render layer by name; empty means the first layer the app declares.
    pub layer: String,
    /// The draw order within the layer, lowest first.
    pub order: i32,
}

/// Why `Tilemap::validate` refuses a tilemap.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TilemapError {
    #[error("cell_size must be finite and above 0, got ({}, {})", .0.x, .0.y)]
    CellSize(Vec2),
    #[error(
        "tiles has {found} entries, but width {width} by height {height} needs {expected}; give one entry per cell"
    )]
    TileCount {
        width: u32,
        height: u32,
        expected: u64,
        found: usize,
    },
}

impl Tilemap {
    pub fn validate(&self) -> Result<(), TilemapError> {
        if !finite_and_positive(self.cell_size) {
            return Err(TilemapError::CellSize(self.cell_size));
        }
        let expected = u64::from(self.width) * u64::from(self.height);
        if u64::try_from(self.tiles.len()) != Ok(expected) {
            return Err(TilemapError::TileCount {
                width: self.width,
                height: self.height,
                expected,
                found: self.tiles.len(),
            });
        }
        Ok(())
    }
}

/// Serialization mirror of `Tilemap` so loading can validate.
#[derive(Serialize, Deserialize)]
struct TilemapData {
    tileset: AssetId,
    cell_size: Vec2,
    width: u32,
    height: u32,
    tiles: Vec<Option<u32>>,
    layer: String,
    order: i32,
}

impl From<Tilemap> for TilemapData {
    fn from(map: Tilemap) -> Self {
        TilemapData {
            tileset: map.tileset,
            cell_size: map.cell_size,
            width: map.width,
            height: map.height,
            tiles: map.tiles,
            layer: map.layer,
            order: map.order,
        }
    }
}

impl TryFrom<TilemapData> for Tilemap {
    type Error = TilemapError;

    fn try_from(data: TilemapData) -> Result<Self, TilemapError> {
        let map = Tilemap {
            tileset: data.tileset,
            cell_size: data.cell_size,
            width: data.width,
            height: data.height,
            tiles: data.tiles,
            layer: data.layer,
            order: data.order,
        };
        map.validate()?;
        Ok(map)
    }
}

/// The entity moved without travelling: the render frame whose tick is `tick` is not
/// interpolated from the one before. A system teleporting an entity writes
/// `Time.tick + 1`: during a step, `Time.tick` counts the steps completed before it, and the
/// frame showing that step's result has the tick after it (`Simulation::tick` once the
/// step ends).
#[derive(Component, Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
#[component(name = "ludenea::Teleported")]
pub struct Teleported {
    pub tick: u64,
}

/// NaN fails the comparison.
fn finite_and_positive(v: Vec2) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.x > 0.0 && v.y > 0.0
}
