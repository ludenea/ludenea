//! Scene queries over the colliders: raycasts and box overlaps.

use ludenea_ecs::{Entity, Query};
use ludenea_math::{Aabb, Transform, Vec2, clamp};

use crate::components::Collider;
use crate::shape::Shape;
use ludenea_app::pose::Pose;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RayHit {
    pub entity: Entity,
    pub point: Vec2,
    pub normal: Vec2,
    /// Position of the hit along the translation, in `[0, 1]`.
    pub fraction: f32,
}

/// A shape as its core (points, or a polygon or segment outline) grown by `radius`.
struct Core {
    points: Vec<Vec2>,
    /// Outline edges `(from, to, outward normal)`.
    edges: Vec<(Vec2, Vec2, Vec2)>,
    /// The outline encloses an area, so a point inside it is inside the shape.
    solid: bool,
    radius: f32,
}

impl Core {
    fn new(shape: &Shape) -> Core {
        match shape {
            Shape::Circle(circle) => Core {
                points: vec![circle.center],
                edges: Vec::new(),
                solid: false,
                radius: circle.radius,
            },
            Shape::Capsule(capsule) => {
                let (a, b) = (capsule.center1, capsule.center2);
                let axis = b - a;
                let edges = if axis.length_squared() > 0.0 {
                    let normal = axis.normalize().left_perp();
                    vec![(a, b, normal), (b, a, -normal)]
                } else {
                    Vec::new()
                };
                Core {
                    points: vec![a, b],
                    edges,
                    solid: false,
                    radius: capsule.radius,
                }
            }
            Shape::Polygon(polygon) => {
                let vertices = polygon.vertices();
                let edges = (0..vertices.len())
                    .map(|i| {
                        let next = vertices[(i + 1) % vertices.len()];
                        (vertices[i], next, polygon.normals()[i])
                    })
                    .collect();
                Core {
                    points: vertices.to_vec(),
                    edges,
                    solid: true,
                    radius: polygon.radius(),
                }
            }
        }
    }

    fn contains(&self, p: Vec2) -> bool {
        let in_core = self.solid && self.edges.iter().all(|&(a, _, n)| n.dot(p - a) <= 0.0);
        let radius_squared = self.radius * self.radius;
        in_core
            || self
                .points
                .iter()
                .any(|&q| (p - q).length_squared() <= radius_squared)
            || self
                .edges
                .iter()
                .any(|&(a, b, _)| segment_distance_squared(p, a, b) <= radius_squared)
    }

    /// First entry of the ray `origin + t * delta`, `t` in `[0, 1]`, as `(t, normal)`.
    /// A ray starting inside does not hit. The shape is a union of convex pieces (offset
    /// edges seen from outside and vertex discs), so the smallest entry is the hit.
    fn raycast(&self, origin: Vec2, delta: Vec2) -> Option<(f32, Vec2)> {
        if self.contains(origin) {
            return None;
        }
        let mut best: Option<(f32, Vec2)> = None;
        let mut offer = |t: f32, normal: Vec2| {
            if best.is_none_or(|(current, _)| t < current) {
                best = Some((t, normal));
            }
        };
        for &(a, b, normal) in &self.edges {
            if normal.dot(delta) >= 0.0 {
                continue;
            }
            let (a, b) = (a + normal * self.radius, b + normal * self.radius);
            if let Some(t) = ray_segment(origin, delta, a, b) {
                offer(t, normal);
            }
        }
        if self.radius > 0.0 {
            for &center in &self.points {
                if let Some(t) = ray_disc(origin - center, delta, self.radius) {
                    let point = origin + delta * t;
                    offer(t, (point - center).normalize());
                }
            }
        }
        best
    }
}

fn segment_distance_squared(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let edge = b - a;
    let length_squared = edge.length_squared();
    let t = if length_squared > 0.0 {
        (p - a).dot(edge) / length_squared
    } else {
        0.0
    };
    let t = clamp(t, 0.0, 1.0);
    (p - (a + edge * t)).length_squared()
}

/// Fraction at which the ray crosses the segment `a..b`.
fn ray_segment(origin: Vec2, delta: Vec2, a: Vec2, b: Vec2) -> Option<f32> {
    let edge = b - a;
    let denominator = delta.cross(edge);
    if denominator == 0.0 {
        return None;
    }
    let t = (a - origin).cross(edge) / denominator;
    let u = (a - origin).cross(delta) / denominator;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

/// Fraction at which the ray, starting outside the disc at `relative` from its center,
/// enters the disc.
fn ray_disc(relative: Vec2, delta: Vec2, radius: f32) -> Option<f32> {
    let a = delta.length_squared();
    let b = relative.dot(delta);
    let discriminant = b * b - a * (relative.length_squared() - radius * radius);
    if a == 0.0 || discriminant < 0.0 {
        return None;
    }
    let t = (-b - discriminant.sqrt()) / a;
    (0.0..=1.0).contains(&t).then_some(t)
}

/// The closest collider hit by the ray from `origin` along `translation`; ties go to the
/// lower entity. A ray starting inside a shape does not hit it.
pub fn raycast(
    colliders: &Query<(Entity, &Pose, &Collider)>,
    origin: Vec2,
    translation: Vec2,
) -> Option<RayHit> {
    let mut best: Option<RayHit> = None;
    for (entity, pose, collider) in colliders.iter() {
        let xf = Transform {
            position: pose.position,
            rotation: pose.rotation,
        };
        let core = Core::new(&collider.shape);
        let local_origin = xf.inv_apply(origin);
        let local_delta = xf.rotation.inv_rotate(translation);
        let Some((fraction, normal)) = core.raycast(local_origin, local_delta) else {
            continue;
        };
        let closer = best.is_none_or(|hit| {
            fraction < hit.fraction || (fraction == hit.fraction && entity < hit.entity)
        });
        if closer {
            best = Some(RayHit {
                entity,
                point: origin + translation * fraction,
                normal: xf.rotation.rotate(normal),
                fraction,
            });
        }
    }
    best
}

/// Whether the world point `p` is inside `shape` posed at `xf`.
pub(crate) fn shape_contains(shape: &Shape, xf: Transform, p: Vec2) -> bool {
    Core::new(shape).contains(xf.inv_apply(p))
}

/// Entities, ascending, whose collider bounding box overlaps `aabb`.
// ponytail: brute force over all colliders; reuse the broad phase when queries get hot
pub fn overlap_aabb(colliders: &Query<(Entity, &Pose, &Collider)>, aabb: Aabb) -> Vec<Entity> {
    colliders
        .iter()
        .filter(|(_, pose, collider)| {
            let xf = Transform {
                position: pose.position,
                rotation: pose.rotation,
            };
            collider.shape.compute_aabb(xf).overlaps(aabb)
        })
        .map(|(entity, _, _)| entity)
        .collect()
}
