//! What a renderer draws, as plain data: the render frame (`frame`) a simulation extracts
//! each tick, and the debug draw primitives simulation code asks to be drawn over the world,
//! grouped in named layers. `mesh` turns a debug draw frame into triangles.

use std::collections::BTreeMap;

use ludenea_ecs::Entity;
use ludenea_ecs::schema::Schema;
use ludenea_math::Vec2;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod frame;
pub mod mesh;

/// `0xRRGGBBAA`.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, Schema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Color(pub u32);

/// A debug draw primitive. JSON tags it with a `type` field; the formats that are not human
/// readable, such as postcard, which cannot decode an internally tagged enum, tag it by
/// variant index.
// Its variants are declared three times: here, in `TaggedPrimitive` and in
// `IndexedPrimitive`. A change to one is a change to all three; the compiler names any that
// differs.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(tag = "type", rename_all = "snake_case"))]
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

/// How `Primitive` is written in the human readable formats. The compiler checks that its
/// variants and `IndexedPrimitive`'s are `Primitive`'s.
#[derive(Serialize, Deserialize)]
#[serde(remote = "Primitive", tag = "type", rename_all = "snake_case")]
enum TaggedPrimitive {
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

/// How `Primitive` is written in the formats that are not human readable.
#[derive(Serialize, Deserialize)]
#[serde(remote = "Primitive")]
enum IndexedPrimitive {
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

impl Serialize for Primitive {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            TaggedPrimitive::serialize(self, serializer)
        } else {
            IndexedPrimitive::serialize(self, serializer)
        }
    }
}

impl<'de> Deserialize<'de> for Primitive {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            TaggedPrimitive::deserialize(deserializer)
        } else {
            IndexedPrimitive::deserialize(deserializer)
        }
    }
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

/// The share of its alpha a color keeps in the ghost style, in percent.
pub const GHOST_OPACITY_PERCENT: u32 = 40;

impl Color {
    /// The same color at `percent` of its alpha.
    fn faded(self, percent: u32) -> Color {
        let alpha = (self.0 & 0xFF) * percent / 100;
        Color(self.0 & !0xFF | alpha)
    }
}

impl DebugDrawFrame {
    /// The frame in the ghost style, how the viewport draws another branch: the same
    /// primitives, outlines only, at reduced opacity.
    pub fn ghost(&self) -> DebugDrawFrame {
        let fade = |color: &mut Color| *color = color.faded(GHOST_OPACITY_PERCENT);
        let mut ghost = self.clone();
        for item in ghost.layers.values_mut().flatten() {
            match &mut item.primitive {
                Primitive::Circle { color, fill, .. } | Primitive::Polygon { color, fill, .. } => {
                    *fill = false;
                    fade(color);
                }
                Primitive::Line { color, .. }
                | Primitive::Capsule { color, .. }
                | Primitive::Label { color, .. } => fade(color),
            }
        }
        ghost
    }

    pub fn push(&mut self, layer: &str, entity: Option<Entity>, primitive: Primitive) {
        self.layers
            .entry(layer.to_string())
            .or_default()
            .push(Item { entity, primitive });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghost_keeps_primitives_as_faded_outlines() {
        let mut frame = DebugDrawFrame::default();
        let circle = |color, fill| Primitive::Circle {
            center: Vec2::ZERO,
            radius: 1.0,
            color: Color(color),
            fill,
        };
        let label = |color| Primitive::Label {
            at: Vec2::ZERO,
            text: "x".into(),
            color: Color(color),
        };
        frame.push("l", None, circle(0x112233FF, true));
        frame.push("l", None, label(0x44556680));
        let ghost = frame.ghost();
        assert_eq!(
            ghost.layers["l"]
                .iter()
                .map(|item| &item.primitive)
                .collect::<Vec<_>>(),
            [&circle(0x11223366, false), &label(0x44556633)]
        );
    }
}
