use crate::mass::mass_data;
use crate::shape::Shape;
use ludenea_ecs::schema::Schema;
use ludenea_ecs::{Component, Resource};
use ludenea_math::{Rot, Vec2};
use serde::{Deserialize, Serialize};

/// Tolerance on `cos² + sin² = 1` when validating a rotation.
const UNIT_ROTATION_TOLERANCE: f32 = 1e-4;

/// Largest magnitude of each gravity component `PhysicsSettings` accepts, in m/s².
pub const MAX_GRAVITY: f32 = 1e6;

/// Largest magnitude of `Body::gravity_scale`, so that a body's gravity stays within
/// `MAX_GRAVITY * MAX_GRAVITY_SCALE` and its velocity finite.
pub const MAX_GRAVITY_SCALE: f32 = 1e3;

/// Largest `Collider::density`, in kg/m²: denser colliders overflow the contact solver.
pub const MAX_DENSITY: f32 = 1e6;

/// Most solver substeps per tick that `PhysicsSettings` accepts.
pub const MAX_SUBSTEPS: u32 = 64;

/// Largest `PhysicsSettings::contact_damping_ratio`, a hundred times the default: a larger
/// ratio overflows the contact softness.
pub const MAX_CONTACT_DAMPING_RATIO: f32 = 1e3;

/// Largest `Collider::friction`: a larger coefficient overflows the friction mix.
pub const MAX_FRICTION: f32 = 1e3;

/// Largest `Collider::restitution`: a larger coefficient overflows the bounce impulse.
pub const MAX_RESTITUTION: f32 = 1e3;

/// Smallest mass, in kg, of a `Collider` whose density is not 0: the inverse of a smaller
/// mass overflows the contact solver.
pub const MIN_MASS: f32 = 1e-9;

/// Body origin in world space. Deserialization rejects what `validate` refuses: a
/// non-finite position or a rotation that is not unit length.
#[derive(Component, Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Default)]
#[component(name = "ludenea::Pose")]
#[serde(try_from = "PoseData", into = "PoseData")]
pub struct Pose {
    pub position: Vec2,
    pub rotation: Rot,
}

/// Why `Pose::validate` refuses a pose.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum PoseError {
    #[error("position must be finite, got ({}, {})", .0.x, .0.y)]
    Position(Vec2),
    #[error("rotation must be finite with cos² + sin² = 1, got cos {} and sin {}", .0.cos, .0.sin)]
    Rotation(Rot),
}

impl Pose {
    /// Checks what loading and the physics step require: a finite position and a finite,
    /// unit-length rotation (within `1e-4`).
    pub fn validate(&self) -> Result<(), PoseError> {
        let Rot { cos, sin } = self.rotation;
        if !(self.position.x.is_finite() && self.position.y.is_finite()) {
            return Err(PoseError::Position(self.position));
        }
        if !(cos.is_finite() && sin.is_finite())
            || (cos * cos + sin * sin - 1.0).abs() > UNIT_ROTATION_TOLERANCE
        {
            return Err(PoseError::Rotation(self.rotation));
        }
        Ok(())
    }
}

/// Serialization mirror of `Pose` so loading can validate.
#[derive(Serialize, Deserialize)]
struct PoseData {
    position: Vec2,
    rotation: Rot,
}

impl From<Pose> for PoseData {
    fn from(p: Pose) -> Self {
        PoseData {
            position: p.position,
            rotation: p.rotation,
        }
    }
}

impl TryFrom<PoseData> for Pose {
    type Error = PoseError;

    fn try_from(data: PoseData) -> Result<Self, PoseError> {
        let pose = Pose {
            position: data.position,
            rotation: data.rotation,
        };
        pose.validate()?;
        Ok(pose)
    }
}

#[derive(Serialize, Deserialize, Schema, Copy, Clone, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Static,
    Kinematic,
    Dynamic,
}

/// Deserialization rejects non-finite values, negative damping, and a `gravity_scale`
/// beyond `±MAX_GRAVITY_SCALE`.
#[derive(Component, Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
#[component(name = "ludenea::Body")]
#[serde(try_from = "BodyData", into = "BodyData")]
pub struct Body {
    pub kind: BodyKind,
    pub linear_velocity: Vec2,
    pub angular_velocity: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub gravity_scale: f32,
}

/// Serialization mirror of `Body` so loading can validate.
#[derive(Serialize, Deserialize)]
struct BodyData {
    kind: BodyKind,
    linear_velocity: Vec2,
    angular_velocity: f32,
    linear_damping: f32,
    angular_damping: f32,
    gravity_scale: f32,
}

impl From<Body> for BodyData {
    fn from(b: Body) -> Self {
        BodyData {
            kind: b.kind,
            linear_velocity: b.linear_velocity,
            angular_velocity: b.angular_velocity,
            linear_damping: b.linear_damping,
            angular_damping: b.angular_damping,
            gravity_scale: b.gravity_scale,
        }
    }
}

impl TryFrom<BodyData> for Body {
    type Error = String;

    fn try_from(data: BodyData) -> Result<Self, String> {
        let velocity = data.linear_velocity;
        if !(velocity.x.is_finite() && velocity.y.is_finite() && data.angular_velocity.is_finite())
        {
            return Err("velocities must be finite".to_string());
        }
        if !(non_negative(data.linear_damping) && non_negative(data.angular_damping)) {
            return Err("damping must be finite and zero or more".to_string());
        }
        if !within(data.gravity_scale, MAX_GRAVITY_SCALE) {
            return Err(format!(
                "gravity_scale must be finite and within ±{MAX_GRAVITY_SCALE:?}, got {:?}",
                data.gravity_scale
            ));
        }
        Ok(Body {
            kind: data.kind,
            linear_velocity: data.linear_velocity,
            angular_velocity: data.angular_velocity,
            linear_damping: data.linear_damping,
            angular_damping: data.angular_damping,
            gravity_scale: data.gravity_scale,
        })
    }
}

/// Finite and `>= 0` (NaN fails both).
fn non_negative(value: f32) -> bool {
    value.is_finite() && value >= 0.0
}

/// Within `±limit` (NaN fails).
fn within(value: f32, limit: f32) -> bool {
    -limit <= value && value <= limit
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

/// One collider per entity. Deserialization validates the shape and rejects non-finite or
/// negative density, friction, and restitution, values above `MAX_DENSITY`, `MAX_FRICTION`,
/// and `MAX_RESTITUTION`, and a density other than 0 that gives a mass below `MIN_MASS` or
/// an infinite one.
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
    type Error = String;

    fn try_from(data: ColliderData) -> Result<Self, String> {
        data.shape.validate().map_err(|error| error.to_string())?;
        if !(non_negative(data.density)
            && non_negative(data.friction)
            && non_negative(data.restitution))
        {
            return Err(
                "density, friction, and restitution must be finite and zero or more".to_string(),
            );
        }
        for (name, value, limit) in [
            ("density", data.density, MAX_DENSITY),
            ("friction", data.friction, MAX_FRICTION),
            ("restitution", data.restitution, MAX_RESTITUTION),
        ] {
            if value > limit {
                return Err(format!("{name} must be at most {limit:?}, got {value:?}"));
            }
        }
        let mass = mass_data(&data.shape, data.density).mass;
        if data.density > 0.0 && !(MIN_MASS <= mass && mass.is_finite()) {
            return Err(format!(
                "mass must be finite and at least {MIN_MASS:?} kg unless density is 0, got \
                 {mass:?} from density {:?}",
                data.density
            ));
        }
        Ok(Collider {
            shape: data.shape,
            density: data.density,
            friction: data.friction,
            restitution: data.restitution,
        })
    }
}

/// How the physics step solves. Deserialization rejects a gravity component that is not
/// finite or beyond `±MAX_GRAVITY`, `substeps` outside 1 to `MAX_SUBSTEPS`, a
/// `contact_damping_ratio` above `MAX_CONTACT_DAMPING_RATIO`, and the other values when they
/// are not finite or below zero.
#[derive(Resource, Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
#[resource(name = "ludenea::PhysicsSettings")]
#[serde(try_from = "PhysicsSettingsData", into = "PhysicsSettingsData")]
pub struct PhysicsSettings {
    pub gravity: Vec2,
    /// Solver substeps per tick; the step brings a value that game code writes outside 1 to
    /// `MAX_SUBSTEPS` back within it.
    pub substeps: u32,
    pub contact_hertz: f32,
    /// The step brings a value that game code writes above `MAX_CONTACT_DAMPING_RATIO` down
    /// to it.
    pub contact_damping_ratio: f32,
    pub contact_push_max_velocity: f32,
    pub restitution_threshold: f32,
}

/// Serialization mirror of `PhysicsSettings` so loading can validate.
#[derive(Serialize, Deserialize)]
struct PhysicsSettingsData {
    gravity: Vec2,
    substeps: u32,
    contact_hertz: f32,
    contact_damping_ratio: f32,
    contact_push_max_velocity: f32,
    restitution_threshold: f32,
}

impl From<PhysicsSettings> for PhysicsSettingsData {
    fn from(s: PhysicsSettings) -> Self {
        PhysicsSettingsData {
            gravity: s.gravity,
            substeps: s.substeps,
            contact_hertz: s.contact_hertz,
            contact_damping_ratio: s.contact_damping_ratio,
            contact_push_max_velocity: s.contact_push_max_velocity,
            restitution_threshold: s.restitution_threshold,
        }
    }
}

impl TryFrom<PhysicsSettingsData> for PhysicsSettings {
    type Error = String;

    fn try_from(data: PhysicsSettingsData) -> Result<Self, String> {
        let gravity = data.gravity;
        if !(within(gravity.x, MAX_GRAVITY) && within(gravity.y, MAX_GRAVITY)) {
            return Err(format!(
                "gravity must be finite with each component within ±{MAX_GRAVITY:?}, got ({:?}, {:?})",
                gravity.x, gravity.y
            ));
        }
        if !(1..=MAX_SUBSTEPS).contains(&data.substeps) {
            return Err(format!(
                "substeps must be within 1 to {MAX_SUBSTEPS}, got {}",
                data.substeps
            ));
        }
        for (name, value) in [
            ("contact_hertz", data.contact_hertz),
            ("contact_damping_ratio", data.contact_damping_ratio),
            ("contact_push_max_velocity", data.contact_push_max_velocity),
            ("restitution_threshold", data.restitution_threshold),
        ] {
            if !non_negative(value) {
                return Err(format!(
                    "{name} must be finite and zero or more, got {value:?}"
                ));
            }
        }
        if data.contact_damping_ratio > MAX_CONTACT_DAMPING_RATIO {
            return Err(format!(
                "contact_damping_ratio must be at most {MAX_CONTACT_DAMPING_RATIO:?}, got {:?}",
                data.contact_damping_ratio
            ));
        }
        Ok(PhysicsSettings {
            gravity,
            substeps: data.substeps,
            contact_hertz: data.contact_hertz,
            contact_damping_ratio: data.contact_damping_ratio,
            contact_push_max_velocity: data.contact_push_max_velocity,
            restitution_threshold: data.restitution_threshold,
        })
    }
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
