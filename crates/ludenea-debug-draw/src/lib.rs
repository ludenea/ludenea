//! Debug draw primitives: what simulation code asks to be drawn over the world, grouped
//! in named layers. `mesh` turns a frame into triangles; `render` (feature `render`, browser
//! only) draws them for the editor's viewport.

use std::collections::BTreeMap;

use ludenea_ecs::Entity;
use ludenea_math::Vec2;
use serde::{Deserialize, Serialize};

pub mod mesh;
#[cfg(all(feature = "render", target_arch = "wasm32", target_os = "unknown"))]
pub mod render;

/// `0xRRGGBBAA`.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Color(pub u32);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Primitive {
    Line {
        a: Vec2,
        b: Vec2,
        color: Color,
    },
    Circle {
        center: Vec2,
        radius: f32,
        color: Color,
        fill: bool,
    },
    Polygon {
        points: Vec<Vec2>,
        color: Color,
        fill: bool,
    },
    Capsule {
        a: Vec2,
        b: Vec2,
        radius: f32,
        color: Color,
    },
    Label {
        at: Vec2,
        text: String,
        color: Color,
    },
}

/// A primitive and the entity it stands for, so selected entities can be highlighted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Item {
    pub entity: Option<Entity>,
    pub primitive: Primitive,
}

/// Primitives by layer name, in insertion order within a layer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DebugDrawFrame {
    pub layers: BTreeMap<String, Vec<Item>>,
}

impl DebugDrawFrame {
    pub fn push(&mut self, layer: &str, entity: Option<Entity>, primitive: Primitive) {
        self.layers
            .entry(layer.to_string())
            .or_default()
            .push(Item { entity, primitive });
    }
}
