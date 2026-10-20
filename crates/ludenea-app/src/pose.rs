//! Where an entity is: the one position component the simulation, the physics and the
//! renderer share, so an entity without a body can be placed and drawn.

use ludenea_ecs::Component;
use ludenea_math::{Rot, Vec2};
use serde::{Deserialize, Serialize};

/// Tolerance on `cos² + sin² = 1` when validating a rotation.
const UNIT_ROTATION_TOLERANCE: f32 = 1e-4;

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
