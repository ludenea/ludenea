use ludenea_math::{Aabb, Transform, Vec2};
use serde::{Deserialize, Serialize};

pub const LINEAR_SLOP: f32 = 0.005;
pub const MAX_POLYGON_VERTICES: usize = 8;

/// Tolerance when checking stored normals against their edges on load.
const NORMAL_TOLERANCE: f32 = 1e-4;

#[derive(Debug, thiserror::Error)]
pub enum ShapeError {
    #[error("a polygon needs at least 3 distinct points; add more points")]
    TooFewPoints,
    #[error("a polygon hull has at most {MAX_POLYGON_VERTICES} vertices; simplify the point set")]
    TooManyPoints,
    #[error("the points are collinear and enclose no area; move at least one point off the line")]
    Collinear,
    #[error("the radius must be finite and positive (zero or more for polygons); fix the radius")]
    NonPositiveRadius,
    #[error("invalid shape: {0}")]
    InvalidPolygon(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Circle(Circle),
    Capsule(Capsule),
    Polygon(Polygon),
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    pub center: Vec2,
    pub radius: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capsule {
    pub center1: Vec2,
    pub center2: Vec2,
    pub radius: f32,
}

/// Convex polygon with an optional rounding radius. Every field is stored and
/// serialized, and loading validates without recomputing, so a restored or
/// loaded polygon is bit-identical to the one that was saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PolygonData", into = "PolygonData")]
pub struct Polygon {
    vertices: [Vec2; MAX_POLYGON_VERTICES],
    normals: [Vec2; MAX_POLYGON_VERTICES],
    count: usize,
    centroid: Vec2,
    radius: f32,
}

/// Serialization mirror of `Polygon` so loading can validate.
#[derive(Serialize, Deserialize)]
struct PolygonData {
    vertices: [Vec2; MAX_POLYGON_VERTICES],
    normals: [Vec2; MAX_POLYGON_VERTICES],
    count: usize,
    centroid: Vec2,
    radius: f32,
}

impl From<Polygon> for PolygonData {
    fn from(p: Polygon) -> Self {
        PolygonData {
            vertices: p.vertices,
            normals: p.normals,
            count: p.count,
            centroid: p.centroid,
            radius: p.radius,
        }
    }
}

impl TryFrom<PolygonData> for Polygon {
    type Error = ShapeError;

    fn try_from(data: PolygonData) -> Result<Self, ShapeError> {
        let invalid = |message: &str| Err(ShapeError::InvalidPolygon(message.to_string()));
        if !(3..=MAX_POLYGON_VERTICES).contains(&data.count) {
            return invalid("polygon vertex count must be between 3 and 8");
        }
        if !data.radius.is_finite() || data.radius < 0.0 {
            return Err(ShapeError::NonPositiveRadius);
        }
        let all_finite = data
            .vertices
            .iter()
            .chain(&data.normals)
            .chain([&data.centroid])
            .all(|v| v.x.is_finite() && v.y.is_finite());
        if !all_finite {
            return invalid("polygon values must be finite");
        }
        let n = data.count;
        let unused = Vec2::ZERO;
        if data.vertices[n..]
            .iter()
            .chain(&data.normals[n..])
            .any(|v| *v != unused)
        {
            return invalid("unused polygon slots must be zero");
        }
        for i in 0..n {
            let a = data.vertices[i];
            let edge = data.vertices[(i + 1) % n] - a;
            let expected = edge.right_perp().normalize();
            let normal = data.normals[i];
            if (normal.x - expected.x).abs() > NORMAL_TOLERANCE
                || (normal.y - expected.y).abs() > NORMAL_TOLERANCE
            {
                return invalid("polygon normals do not match their edges");
            }
            // Strictly convex and counter-clockwise: every other vertex is
            // strictly left of every edge.
            for j in (0..n).filter(|j| *j != i && *j != (i + 1) % n) {
                if edge.cross(data.vertices[j] - a) <= 0.0 {
                    return invalid("polygon must be strictly convex and counter-clockwise");
                }
            }
        }
        Ok(Polygon {
            vertices: data.vertices,
            normals: data.normals,
            count: n,
            centroid: data.centroid,
            radius: data.radius,
        })
    }
}

impl Polygon {
    /// Builds the canonical convex hull of `points`: gift wrapping, start at the
    /// lowest x (then lowest y), counter-clockwise, near points welded and
    /// near-collinear vertices dropped. Idempotent on its own output.
    pub fn new(points: &[Vec2], radius: f32) -> Result<Polygon, ShapeError> {
        if !radius.is_finite() || radius < 0.0 {
            return Err(ShapeError::NonPositiveRadius);
        }
        if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return Err(ShapeError::InvalidPolygon(
                "points must be finite".to_string(),
            ));
        }
        let hull = convex_hull(points)?;
        Ok(finish(&hull, radius))
    }

    pub fn make_box(half_width: f32, half_height: f32) -> Polygon {
        finish(
            &[
                Vec2::new(-half_width, -half_height),
                Vec2::new(half_width, -half_height),
                Vec2::new(half_width, half_height),
                Vec2::new(-half_width, half_height),
            ],
            0.0,
        )
    }

    /// Two-vertex rounded polygon used to collide capsules.
    #[allow(dead_code)] // consumed by the contact manifolds
    pub(crate) fn from_segment(c1: Vec2, c2: Vec2, radius: f32) -> Polygon {
        finish(&[c1, c2], radius)
    }

    pub fn vertices(&self) -> &[Vec2] {
        &self.vertices[..self.count]
    }

    pub fn normals(&self) -> &[Vec2] {
        &self.normals[..self.count]
    }

    pub fn centroid(&self) -> Vec2 {
        self.centroid
    }

    pub fn radius(&self) -> f32 {
        self.radius
    }
}

/// Computes normals and centroid. The one place a `Polygon` is assembled.
fn finish(vertices: &[Vec2], radius: f32) -> Polygon {
    let count = vertices.len();
    let mut polygon = Polygon {
        vertices: [Vec2::ZERO; MAX_POLYGON_VERTICES],
        normals: [Vec2::ZERO; MAX_POLYGON_VERTICES],
        count,
        centroid: Vec2::ZERO,
        radius,
    };
    polygon.vertices[..count].copy_from_slice(vertices);
    for i in 0..count {
        let edge = vertices[(i + 1) % count] - vertices[i];
        polygon.normals[i] = edge.right_perp().normalize();
    }
    polygon.centroid = if count == 2 {
        (vertices[0] + vertices[1]) * 0.5
    } else {
        centroid(vertices)
    };
    polygon
}

fn centroid(vertices: &[Vec2]) -> Vec2 {
    const INV3: f32 = 1.0 / 3.0;
    let origin = vertices[0];
    let mut center = Vec2::ZERO;
    let mut area = 0.0;
    for i in 1..vertices.len() - 1 {
        let e1 = vertices[i] - origin;
        let e2 = vertices[i + 1] - origin;
        let triangle_area = 0.5 * e1.cross(e2);
        area += triangle_area;
        center += (e1 + e2) * (triangle_area * INV3);
    }
    origin + center * (1.0 / area)
}

fn convex_hull(points: &[Vec2]) -> Result<Vec<Vec2>, ShapeError> {
    let weld_squared = (0.5 * LINEAR_SLOP) * (0.5 * LINEAR_SLOP);
    let mut unique: Vec<Vec2> = Vec::with_capacity(points.len());
    for p in points {
        if unique
            .iter()
            .all(|q| (*p - *q).length_squared() >= weld_squared)
        {
            unique.push(*p);
        }
    }
    if unique.len() < 3 {
        return Err(ShapeError::TooFewPoints);
    }

    let mut start = 0;
    for (i, p) in unique.iter().enumerate() {
        let s = unique[start];
        if p.x < s.x || (p.x == s.x && p.y < s.y) {
            start = i;
        }
    }

    // Gift wrapping: always take the most clockwise candidate so the rest stay
    // on the left; among collinear candidates take the farthest.
    let mut hull = vec![unique[start]];
    let mut current = start;
    for _ in 0..unique.len() {
        let mut next = if current == 0 { 1 } else { 0 };
        for (i, p) in unique.iter().enumerate() {
            if i == current || i == next {
                continue;
            }
            let from = unique[current];
            let to_next = unique[next] - from;
            let to_p = *p - from;
            let cross = to_next.cross(to_p);
            if cross < 0.0 || (cross == 0.0 && to_p.length_squared() > to_next.length_squared()) {
                next = i;
            }
        }
        if next == start {
            break;
        }
        hull.push(unique[next]);
        current = next;
    }

    // Drop vertices within the slop of the line through their neighbours.
    while hull.len() >= 3 {
        let n = hull.len();
        let flat = (0..n).find(|&i| {
            let a = hull[(i + n - 1) % n];
            let chord = hull[(i + 1) % n] - a;
            chord.cross(hull[i] - a).abs() <= LINEAR_SLOP * chord.length()
        });
        match flat {
            Some(i) => {
                hull.remove(i);
            }
            None => break,
        }
    }
    if hull.len() < 3 {
        return Err(ShapeError::Collinear);
    }
    if hull.len() > MAX_POLYGON_VERTICES {
        return Err(ShapeError::TooManyPoints);
    }

    // Canonical start vertex: lowest x, then lowest y.
    let mut first = 0;
    for (i, p) in hull.iter().enumerate() {
        let s = hull[first];
        if p.x < s.x || (p.x == s.x && p.y < s.y) {
            first = i;
        }
    }
    hull.rotate_left(first);
    Ok(hull)
}

impl Shape {
    pub fn compute_aabb(&self, xf: Transform) -> Aabb {
        match self {
            Shape::Circle(c) => {
                let center = xf.apply(c.center);
                let r = Vec2::new(c.radius, c.radius);
                Aabb {
                    min: center - r,
                    max: center + r,
                }
            }
            Shape::Capsule(c) => {
                let p1 = xf.apply(c.center1);
                let p2 = xf.apply(c.center2);
                let r = Vec2::new(c.radius, c.radius);
                Aabb {
                    min: p1.min(p2) - r,
                    max: p1.max(p2) + r,
                }
            }
            Shape::Polygon(p) => {
                let first = xf.apply(p.vertices[0]);
                let (mut lower, mut upper) = (first, first);
                for v in &p.vertices()[1..] {
                    let world = xf.apply(*v);
                    lower = lower.min(world);
                    upper = upper.max(world);
                }
                let r = Vec2::new(p.radius, p.radius);
                Aabb {
                    min: lower - r,
                    max: upper + r,
                }
            }
        }
    }

    pub fn validate(&self) -> Result<(), ShapeError> {
        let finite = |v: Vec2| v.x.is_finite() && v.y.is_finite();
        match self {
            Shape::Circle(c) => {
                if !(c.radius > 0.0 && c.radius.is_finite()) {
                    return Err(ShapeError::NonPositiveRadius);
                }
                if !finite(c.center) {
                    return Err(ShapeError::InvalidPolygon(
                        "circle center must be finite".into(),
                    ));
                }
            }
            Shape::Capsule(c) => {
                if !(c.radius > 0.0 && c.radius.is_finite()) {
                    return Err(ShapeError::NonPositiveRadius);
                }
                if !finite(c.center1) || !finite(c.center2) {
                    return Err(ShapeError::InvalidPolygon(
                        "capsule centers must be finite".into(),
                    ));
                }
                if (c.center2 - c.center1).length_squared() < LINEAR_SLOP * LINEAR_SLOP {
                    return Err(ShapeError::InvalidPolygon(
                        "capsule centers must be distinct".into(),
                    ));
                }
            }
            // A polygon is validated when built or loaded.
            Shape::Polygon(_) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(raw: &[(f32, f32)]) -> Vec<Vec2> {
        raw.iter().map(|&(x, y)| Vec2::new(x, y)).collect()
    }

    #[test]
    fn polygon_from_unordered_points_is_canonical_ccw_hull() {
        let p = Polygon::new(
            &pts(&[
                (1.0, 1.0),
                (0.5, 0.5),
                (-1.0, 1.0),
                (1.0, -1.0),
                (-1.0, -1.0),
            ]),
            0.0,
        )
        .unwrap();
        assert_eq!(
            p.vertices(),
            &pts(&[(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)])[..]
        );
        assert_eq!(p.normals()[0], Vec2::new(0.0, -1.0));
        assert_eq!(p.centroid(), Vec2::ZERO);
    }

    #[test]
    fn polygon_new_is_idempotent() {
        let p = Polygon::new(
            &pts(&[(0.3, 0.1), (2.0, 0.7), (1.1, 2.3), (-0.4, 1.9), (0.9, 1.0)]),
            0.1,
        )
        .unwrap();
        assert_eq!(Polygon::new(p.vertices(), p.radius()).unwrap(), p);
        let b = Polygon::make_box(1.5, 0.5);
        assert_eq!(Polygon::new(b.vertices(), b.radius()).unwrap(), b);
    }

    #[test]
    fn polygon_round_trip_is_bit_identical() {
        let p = Polygon::new(
            &pts(&[(0.3, 0.1), (2.0, 0.7), (1.1, 2.3), (-0.4, 1.9)]),
            0.05,
        )
        .unwrap();
        let bytes = postcard::to_allocvec(&p).unwrap();
        let back: Polygon = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, p);
        assert_eq!(postcard::to_allocvec(&back).unwrap(), bytes);
        let text = ron::to_string(&p).unwrap();
        let back: Polygon = ron::from_str(&text).unwrap();
        assert_eq!(postcard::to_allocvec(&back).unwrap(), bytes);
    }

    #[test]
    fn collinear_points_are_rejected() {
        let result = Polygon::new(&pts(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]), 0.0);
        assert!(matches!(result, Err(ShapeError::Collinear)));
    }

    #[test]
    fn two_points_are_too_few() {
        let result = Polygon::new(&pts(&[(0.0, 0.0), (1.0, 1.0)]), 0.0);
        assert!(matches!(result, Err(ShapeError::TooFewPoints)));
    }

    #[test]
    fn nine_hull_points_are_too_many() {
        let ring: Vec<(f32, f32)> = (0..9)
            .map(|i| {
                let a = i as f32 * 2.0 * ludenea_math::PI / 9.0;
                let (s, c) = ludenea_math::sin_cos(a);
                (c, s)
            })
            .collect();
        assert!(matches!(
            Polygon::new(&pts(&ring), 0.0),
            Err(ShapeError::TooManyPoints)
        ));
    }

    #[test]
    fn zero_radius_circle_is_rejected() {
        let shape = Shape::Circle(Circle {
            center: Vec2::ZERO,
            radius: 0.0,
        });
        assert!(matches!(
            shape.validate(),
            Err(ShapeError::NonPositiveRadius)
        ));
    }

    #[test]
    fn polygon_deserialization_validates() {
        let p = Polygon::make_box(1.0, 1.0);
        let good = ron::to_string(&p).unwrap();
        assert!(ron::from_str::<Polygon>(&good).is_ok());
        // Replace the middle vertices so three of them become collinear.
        let bad = ron::to_string(&PolygonData {
            vertices: {
                let mut v = [Vec2::ZERO; MAX_POLYGON_VERTICES];
                v[..3].copy_from_slice(&pts(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]));
                v
            },
            normals: [Vec2::new(0.0, -1.0); MAX_POLYGON_VERTICES],
            count: 3,
            centroid: Vec2::new(1.0, 0.0),
            radius: 0.0,
        })
        .unwrap();
        assert!(ron::from_str::<Polygon>(&bad).is_err());
    }

    #[test]
    fn collider_deserialization_validates() {
        use crate::Collider;
        let bad = Collider::new(Shape::Circle(Circle {
            center: Vec2::ZERO,
            radius: 0.0,
        }));
        let text = ron::to_string(&bad).unwrap();
        assert!(ron::from_str::<Collider>(&text).is_err());
    }

    #[test]
    fn aabb_of_rotated_box() {
        let xf = Transform {
            position: Vec2::new(1.0, 0.0),
            rotation: ludenea_math::Rot::from_angle(ludenea_math::PI / 2.0),
        };
        let aabb = Shape::Polygon(Polygon::make_box(2.0, 1.0)).compute_aabb(xf);
        assert!((aabb.max.x - 2.0).abs() < 1e-5 && (aabb.max.y - 2.0).abs() < 1e-5);
        assert!((aabb.min.x - 0.0).abs() < 1e-5 && (aabb.min.y + 2.0).abs() < 1e-5);
    }
}
