use crate::Vec2;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb {
    pub fn union(self, other: Aabb) -> Aabb {
        Aabb {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// Touching edges count as overlapping.
    pub fn overlaps(self, other: Aabb) -> bool {
        self.min.x <= other.max.x
            && other.min.x <= self.max.x
            && self.min.y <= other.max.y
            && other.min.y <= self.max.y
    }

    pub fn contains(self, other: Aabb) -> bool {
        self.min.x <= other.min.x
            && self.min.y <= other.min.y
            && other.max.x <= self.max.x
            && other.max.y <= self.max.y
    }

    pub fn perimeter(self) -> f32 {
        2.0 * ((self.max.x - self.min.x) + (self.max.y - self.min.y))
    }

    pub fn fattened(self, margin: f32) -> Aabb {
        let m = Vec2::new(margin, margin);
        Aabb {
            min: self.min - m,
            max: self.max + m,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(x0: f32, y0: f32, x1: f32, y1: f32) -> Aabb {
        Aabb {
            min: Vec2::new(x0, y0),
            max: Vec2::new(x1, y1),
        }
    }

    #[test]
    fn aabb_overlap_touching_edges_counts_as_overlap() {
        assert!(aabb(1.0, 0.0, 2.0, 1.0).overlaps(aabb(0.0, 0.0, 1.0, 1.0)));
        assert!(!aabb(1.1, 0.0, 2.0, 1.0).overlaps(aabb(0.0, 0.0, 1.0, 1.0)));
    }
}
