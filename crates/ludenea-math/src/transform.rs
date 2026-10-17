use crate::{Rot, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Transform {
    pub position: Vec2,
    pub rotation: Rot,
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        position: Vec2::ZERO,
        rotation: Rot::IDENTITY,
    };

    pub fn apply(self, v: Vec2) -> Vec2 {
        self.rotation.rotate(v) + self.position
    }

    pub fn inv_apply(self, v: Vec2) -> Vec2 {
        self.rotation.inv_rotate(v - self.position)
    }

    /// `self * other`
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, other: Transform) -> Transform {
        Transform {
            position: self.rotation.rotate(other.position) + self.position,
            rotation: self.rotation.mul(other.rotation),
        }
    }

    /// `inverse(self) * other`
    pub fn inv_mul(self, other: Transform) -> Transform {
        Transform {
            position: self.rotation.inv_rotate(other.position - self.position),
            rotation: self.rotation.inv_mul(other.rotation),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_mul_inv_mul_round_trip() {
        let a = Transform {
            position: Vec2::new(1.0, 2.0),
            rotation: Rot::from_angle(0.7),
        };
        let b = Transform {
            position: Vec2::new(-3.0, 0.5),
            rotation: Rot::from_angle(-1.2),
        };
        let back = a.mul(a.inv_mul(b));
        assert!((back.position.x - b.position.x).abs() < 1e-5);
        assert!((back.position.y - b.position.y).abs() < 1e-5);
        assert!((back.rotation.cos - b.rotation.cos).abs() < 1e-5);
        assert!((back.rotation.sin - b.rotation.sin).abs() < 1e-5);
    }
}
