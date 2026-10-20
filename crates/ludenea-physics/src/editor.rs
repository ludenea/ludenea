//! Editor support: debug draw layers for colliders, contacts, and bounds, and picking.

use std::collections::BTreeSet;

use ludenea_draw::{Color, DebugDrawFrame, Primitive};
use ludenea_ecs::{Entity, Query, World};
use ludenea_math::{Aabb, Transform, Vec2};

use crate::components::{Body, BodyKind, Collider};
use crate::contacts::ContactCache;
use crate::queries::{overlap_aabb, shape_contains};
use crate::shape::Shape;
use crate::step::SPECULATIVE_DISTANCE;
use ludenea_app::pose::Pose;

const COLLIDERS: &str = "physics.colliders";
/// Contacts of the pairs the last step found touching, at the current poses: after an edit
/// moves a body, the points follow the new poses until the next step updates the pairs.
const CONTACTS: &str = "physics.contacts";
const AABBS: &str = "physics.aabbs";
pub(crate) const DEBUG_LAYERS: [&str; 3] = [COLLIDERS, CONTACTS, AABBS];

/// Entities with a pose and no collider are picked within this distance, in meters.
const PICK_RADIUS: f32 = 0.25;
/// Side of the box used to find the colliders under a point, in meters.
const PICK_BOX: f32 = 0.01;
const CONTACT_POINT_RADIUS: f32 = 0.03;
const CONTACT_NORMAL_LENGTH: f32 = 0.2;

const STATIC_COLOR: Color = Color(0x8C8C8CFF);
const KINEMATIC_COLOR: Color = Color(0x4D8CFFFF);
const DYNAMIC_COLOR: Color = Color(0x4DCC66FF);
const CONTACT_COLOR: Color = Color(0xFF4D4DFF);
const AABB_COLOR: Color = Color(0xFFA64DFF);

fn transform(pose: &Pose) -> Transform {
    Transform {
        position: pose.position,
        rotation: pose.rotation,
    }
}

pub(crate) fn draw(world: &World, layers: &BTreeSet<String>, frame: &mut DebugDrawFrame) {
    let Ok(colliders) = Query::<(Entity, &Pose, &Collider)>::new(world) else {
        return;
    };
    if layers.contains(COLLIDERS) {
        for (entity, pose, collider) in colliders.iter() {
            let kind = world.get::<Body>(entity).map(|body| body.kind);
            let color = match kind {
                Some(BodyKind::Dynamic) => DYNAMIC_COLOR,
                Some(BodyKind::Kinematic) => KINEMATIC_COLOR,
                Some(BodyKind::Static) | None => STATIC_COLOR,
            };
            let xf = transform(pose);
            frame.push(COLLIDERS, Some(entity), outline(&collider.shape, xf, color));
        }
    }
    if layers.contains(AABBS) {
        for (entity, pose, collider) in colliders.iter() {
            let Aabb { min, max } = collider.shape.compute_aabb(transform(pose));
            let points = vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
            let primitive = Primitive::Polygon {
                points,
                color: AABB_COLOR,
                fill: false,
            };
            frame.push(AABBS, Some(entity), primitive);
        }
    }
    if layers.contains(CONTACTS) {
        draw_contacts(world, &colliders, frame);
    }
}

/// Contact points and normals of the last step's touching pairs, recomputed from the
/// current poses (the cache keeps impulses, not positions).
fn draw_contacts(
    world: &World,
    colliders: &Query<(Entity, &Pose, &Collider)>,
    frame: &mut DebugDrawFrame,
) {
    let Some(cache) = world.resource::<ContactCache>() else {
        return;
    };
    for (&(lower, higher), contact) in &cache.pairs {
        if !contact.touching {
            continue;
        }
        let (Ok((_, pose_a, a)), Ok((_, pose_b, b))) =
            (colliders.get(lower), colliders.get(higher))
        else {
            continue;
        };
        let manifold = crate::manifold::collide(
            &a.shape,
            transform(pose_a),
            &b.shape,
            transform(pose_b),
            SPECULATIVE_DISTANCE,
        );
        for point in &manifold.points[..manifold.count] {
            frame.push(
                CONTACTS,
                None,
                Primitive::Circle {
                    center: point.point,
                    radius: CONTACT_POINT_RADIUS,
                    color: CONTACT_COLOR,
                    fill: true,
                },
            );
            frame.push(
                CONTACTS,
                None,
                Primitive::Line {
                    a: point.point,
                    b: point.point + manifold.normal * CONTACT_NORMAL_LENGTH,
                    color: CONTACT_COLOR,
                },
            );
        }
    }
}

// ponytail: the rounding radius of polygons is not drawn; add it if rounded polygons matter
fn outline(shape: &Shape, xf: Transform, color: Color) -> Primitive {
    match shape {
        Shape::Circle(circle) => Primitive::Circle {
            center: xf.apply(circle.center),
            radius: circle.radius,
            color,
            fill: false,
        },
        Shape::Capsule(capsule) => Primitive::Capsule {
            a: xf.apply(capsule.center1),
            b: xf.apply(capsule.center2),
            radius: capsule.radius,
            color,
        },
        Shape::Polygon(polygon) => Primitive::Polygon {
            points: polygon.vertices().iter().map(|&v| xf.apply(v)).collect(),
            color,
            fill: false,
        },
    }
}

pub(crate) fn pick(world: &World, point: Vec2) -> Vec<Entity> {
    let mut found = BTreeSet::new();
    if let Ok(colliders) = Query::<(Entity, &Pose, &Collider)>::new(world) {
        let half = Vec2::new(PICK_BOX * 0.5, PICK_BOX * 0.5);
        let probe = Aabb {
            min: point - half,
            max: point + half,
        };
        for entity in overlap_aabb(&colliders, probe) {
            if let Ok((_, pose, collider)) = colliders.get(entity)
                && shape_contains(&collider.shape, transform(pose), point)
            {
                found.insert(entity);
            }
        }
    }
    if let Ok(poses) = Query::<(Entity, &Pose)>::new(world) {
        for (entity, pose) in poses.iter() {
            if world.get::<Collider>(entity).is_none()
                && (pose.position - point).length_squared() <= PICK_RADIUS * PICK_RADIUS
            {
                found.insert(entity);
            }
        }
    }
    found.into_iter().collect()
}
