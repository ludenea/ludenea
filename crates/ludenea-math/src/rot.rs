use crate::{Vec2, atan2, sin_cos};
use serde::{Deserialize, Serialize};

/// Rotation stored as cosine and sine of the angle.
#[derive(Copy, Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Rot {
    pub cos: f32,
    pub sin: f32,
}

impl Rot {
    pub const IDENTITY: Rot = Rot { cos: 1.0, sin: 0.0 };

    pub fn from_angle(angle: f32) -> Rot {
        let (sin, cos) = sin_cos(angle);
        Rot { cos, sin }
    }

    pub fn angle(self) -> f32 {
        atan2(self.sin, self.cos)
    }

    pub fn rotate(self, v: Vec2) -> Vec2 {
        Vec2::new(
            self.cos * v.x - self.sin * v.y,
            self.sin * v.x + self.cos * v.y,
        )
    }

    pub fn inv_rotate(self, v: Vec2) -> Vec2 {
        Vec2::new(
            self.cos * v.x + self.sin * v.y,
            -self.sin * v.x + self.cos * v.y,
        )
    }

    /// `self * other`
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, other: Rot) -> Rot {
        Rot {
            cos: self.cos * other.cos - self.sin * other.sin,
            sin: self.sin * other.cos + self.cos * other.sin,
        }
    }

    /// `inverse(self) * other`
    pub fn inv_mul(self, other: Rot) -> Rot {
        Rot {
            cos: self.cos * other.cos + self.sin * other.sin,
            sin: self.cos * other.sin - self.sin * other.cos,
        }
    }

    /// Advances the rotation by a small angle (Box2D v3 `b2IntegrateRotation`).
    pub fn integrate(self, delta_angle: f32) -> Rot {
        Rot {
            cos: self.cos - delta_angle * self.sin,
            sin: self.sin + delta_angle * self.cos,
        }
        .normalize()
    }

    pub fn normalize(self) -> Rot {
        let mag = (self.sin * self.sin + self.cos * self.cos).sqrt();
        let inv = if mag > 0.0 { 1.0 / mag } else { 0.0 };
        Rot {
            cos: self.cos * inv,
            sin: self.sin * inv,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_then_inv_rotate_is_identity_within_1e6() {
        let v = Vec2::new(1.5, -2.5);
        for i in -20..20 {
            let r = Rot::from_angle(i as f32 * 0.3);
            let back = r.inv_rotate(r.rotate(v));
            assert!((back.x - v.x).abs() < 1e-6 && (back.y - v.y).abs() < 1e-6);
        }
    }

    #[test]
    fn integrate_keeps_unit_length_within_1e6() {
        let mut r = Rot::IDENTITY;
        for _ in 0..1000 {
            r = r.integrate(0.01);
        }
        assert!((r.cos * r.cos + r.sin * r.sin - 1.0).abs() < 1e-6);
    }
}
