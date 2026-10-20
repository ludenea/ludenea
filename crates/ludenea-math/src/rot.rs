use crate::{Vec2, atan2, sin_cos};
use ludenea_schema::Schema;
use serde::{Deserialize, Serialize};

/// Rotation stored as cosine and sine of the angle, unit length (`cos² + sin² = 1`).
/// `from_angle`, `integrate`, and `normalize` give `IDENTITY` for an input they cannot turn
/// into a unit rotation; code that writes the fields directly keeps them unit length.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, Schema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Rot {
    pub cos: f32,
    pub sin: f32,
}

impl Default for Rot {
    fn default() -> Self {
        Rot::IDENTITY
    }
}

impl Rot {
    pub const IDENTITY: Rot = Rot { cos: 1.0, sin: 0.0 };

    /// As `sin_cos`: `IDENTITY` when the angle is not finite or so large that the
    /// approximation overflows, and a unit rotation that does not follow the angle when it is
    /// too large to reduce precisely.
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
    #[expect(
        clippy::should_implement_trait,
        reason = "composition is a named method beside `inv_mul`, not the `*` operator"
    )]
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

    /// Scales to unit length; `IDENTITY` when the magnitude is zero or not finite (a NaN
    /// or infinite component, or one so large that its square overflows), which has no
    /// direction to keep.
    pub fn normalize(self) -> Rot {
        let mag = (self.sin * self.sin + self.cos * self.cos).sqrt();
        if !(mag > 0.0 && mag.is_finite()) {
            return Rot::IDENTITY;
        }
        let inv = 1.0 / mag;
        Rot {
            cos: self.cos * inv,
            sin: self.sin * inv,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn defaults_are_identity() {
        assert_eq!(super::Rot::default(), super::Rot::IDENTITY);
        assert_eq!(crate::Transform::default(), crate::Transform::IDENTITY);
    }

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

    #[test]
    fn normalize_gives_identity_for_a_zero_or_non_finite_magnitude() {
        for (cos, sin) in [
            (0.0, 0.0),
            (f32::NAN, 0.5),
            (f32::INFINITY, 0.0),
            (3e38, 3e38),
        ] {
            assert_eq!(Rot { cos, sin }.normalize(), Rot::IDENTITY, "{cos}, {sin}");
        }
    }

    #[test]
    fn from_angle_gives_identity_for_an_angle_that_is_not_finite() {
        for angle in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(Rot::from_angle(angle), Rot::IDENTITY, "{angle}");
        }
    }

    #[test]
    fn from_angle_of_a_finite_angle_too_large_to_reduce_is_a_unit_rotation() {
        // Reduced to -65536, outside [-PI, PI]: unit length, yet not the identity.
        assert_eq!(
            Rot::from_angle(1e12),
            Rot {
                cos: 0.70710677,
                sin: 0.70710677
            }
        );
        // The approximation overflows, which gives the identity.
        for angle in [1e30, f32::MAX, f32::MIN] {
            assert_eq!(Rot::from_angle(angle), Rot::IDENTITY, "{angle}");
        }
    }

    #[test]
    fn integrate_by_a_non_finite_angle_gives_identity() {
        assert_eq!(Rot::IDENTITY.integrate(f32::NAN), Rot::IDENTITY);
        assert_eq!(Rot::from_angle(0.5).integrate(f32::INFINITY), Rot::IDENTITY);
    }
}
