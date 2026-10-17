use crate::shape::{Shape, ShapeError};
use ludenea_ecs::{Component, Resource};
use ludenea_math::{Rot, Vec2};
use serde::{Deserialize, Serialize};

/// Body origin in world space.
#[derive(Component, Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Default)]
#[component(name = "ludenea::Pose")]
pub struct Pose {
    pub position: Vec2,
    pub rotation: Rot,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Static,
    Kinematic,
    Dynamic,
}

#[derive(Component, Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
#[component(name = "ludenea::Body")]
pub struct Body {
    pub kind: BodyKind,
    pub linear_velocity: Vec2,
    pub angular_velocity: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub gravity_scale: f32,
}

impl Body {
    fn of_kind(kind: BodyKind) -> Self {
        Body {
            kind,
            linear_velocity: Vec2::ZERO,
            angular_velocity: 0.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: 1.0,
        }
    }

    pub fn dynamic() -> Self {
        Self::of_kind(BodyKind::Dynamic)
    }

    pub fn fixed() -> Self {
        Self::of_kind(BodyKind::Static)
    }

    pub fn kinematic() -> Self {
        Self::of_kind(BodyKind::Kinematic)
    }
}

/// One collider per entity. Deserialization validates the shape.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[component(name = "ludenea::Collider")]
#[serde(try_from = "ColliderData", into = "ColliderData")]
pub struct Collider {
    pub shape: Shape,
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
}

impl Collider {
    pub fn new(shape: Shape) -> Self {
        Collider {
            shape,
            density: 1.0,
            friction: 0.6,
            restitution: 0.0,
        }
    }
}

/// Serialization mirror of `Collider` so loading can validate.
#[derive(Serialize, Deserialize)]
struct ColliderData {
    shape: Shape,
    density: f32,
    friction: f32,
    restitution: f32,
}

impl From<Collider> for ColliderData {
    fn from(c: Collider) -> Self {
        ColliderData {
            shape: c.shape,
            density: c.density,
            friction: c.friction,
            restitution: c.restitution,
        }
    }
}

impl TryFrom<ColliderData> for Collider {
    type Error = ShapeError;

    fn try_from(data: ColliderData) -> Result<Self, ShapeError> {
        data.shape.validate()?;
        Ok(Collider {
            shape: data.shape,
            density: data.density,
            friction: data.friction,
            restitution: data.restitution,
        })
    }
}

#[derive(Resource, Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
#[resource(name = "ludenea::PhysicsSettings")]
pub struct PhysicsSettings {
    pub gravity: Vec2,
    pub substeps: u32,
    pub contact_hertz: f32,
    pub contact_damping_ratio: f32,
    pub contact_push_max_velocity: f32,
    pub restitution_threshold: f32,
}

impl Default for PhysicsSettings {
    fn default() -> Self {
        PhysicsSettings {
            gravity: Vec2::new(0.0, -10.0),
            substeps: 4,
            contact_hertz: 30.0,
            contact_damping_ratio: 10.0,
            contact_push_max_velocity: 3.0,
            restitution_threshold: 1.0,
        }
    }
}
