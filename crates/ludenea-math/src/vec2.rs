use crate::{max, min};
use ludenea_schema::Schema;
use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

#[derive(Copy, Clone, Debug, PartialEq, Default, Serialize, Deserialize, Schema)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn dot(self, other: Vec2) -> f32 {
        self.x * other.x + self.y * other.y
    }

    pub fn cross(self, other: Vec2) -> f32 {
        self.x * other.y - self.y * other.x
    }

    /// `v x s`
    pub fn cross_vs(v: Vec2, s: f32) -> Vec2 {
        Vec2::new(s * v.y, -s * v.x)
    }

    /// `s x v`
    pub fn cross_sv(s: f32, v: Vec2) -> Vec2 {
        Vec2::new(-s * v.y, s * v.x)
    }

    pub fn left_perp(self) -> Vec2 {
        Vec2::new(-self.y, self.x)
    }

    pub fn right_perp(self) -> Vec2 {
        Vec2::new(self.y, -self.x)
    }

    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Returns `ZERO` when the length is below `f32::EPSILON`.
    pub fn normalize(self) -> Vec2 {
        let length = self.length();
        if length < f32::EPSILON {
            return Vec2::ZERO;
        }
        let inv = 1.0 / length;
        Vec2::new(self.x * inv, self.y * inv)
    }

    pub fn distance(self, other: Vec2) -> f32 {
        (other - self).length()
    }

    pub fn lerp(a: Vec2, b: Vec2, t: f32) -> Vec2 {
        Vec2::new((1.0 - t) * a.x + t * b.x, (1.0 - t) * a.y + t * b.y)
    }

    pub fn min(self, other: Vec2) -> Vec2 {
        Vec2::new(min(self.x, other.x), min(self.y, other.y))
    }

    pub fn max(self, other: Vec2) -> Vec2 {
        Vec2::new(max(self.x, other.x), max(self.y, other.y))
    }

    pub fn abs(self) -> Vec2 {
        Vec2::new(self.x.abs(), self.y.abs())
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, s: f32) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}

impl Mul<Vec2> for f32 {
    type Output = Vec2;
    fn mul(self, v: Vec2) -> Vec2 {
        Vec2::new(self * v.x, self * v.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, o: Vec2) {
        *self = *self + o;
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, o: Vec2) {
        *self = *self - o;
    }
}

impl MulAssign<f32> for Vec2 {
    fn mul_assign(&mut self, s: f32) {
        *self = *self * s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_of_zero_is_zero() {
        assert_eq!(Vec2::ZERO.normalize(), Vec2::ZERO);
    }

    #[test]
    fn cross_helpers_match_definitions() {
        assert_eq!(
            Vec2::cross_vs(Vec2::new(1.0, 2.0), 3.0),
            Vec2::new(6.0, -3.0)
        );
        assert_eq!(
            Vec2::cross_sv(3.0, Vec2::new(1.0, 2.0)),
            Vec2::new(-6.0, 3.0)
        );
        assert_eq!(Vec2::new(1.0, 2.0).cross(Vec2::new(3.0, 4.0)), -2.0);
    }
}
