//! The physics system: gathers bodies, finds contacts, solves, and writes back.

use std::collections::BTreeMap;

use ludenea_app::{Parallel, Time};
use ludenea_ecs::{Entity, Query, Res, ResMut, Runtime};
use ludenea_math::{Rot, Transform, Vec2, max};

use crate::broad_phase::{Proxy, find_pairs};
use crate::components::{Body, BodyKind, Collider, PhysicsSettings, Pose};
use crate::contacts::{CachedContact, CachedPoint, ContactCache};
use crate::manifold::collide;
use crate::mass::mass_data;
use crate::shape::LINEAR_SLOP;
use crate::solver::{ConstraintPoint, ContactConstraint, RIGID, SolverBody, StepContext, solve};

const SPECULATIVE_DISTANCE: f32 = 4.0 * LINEAR_SLOP;

/// Inverse mass, inverse rotational inertia, and local center of mass. Non-dynamic bodies
/// are immovable about their origin; a dynamic body without mass gets mass 1 and locked
/// rotation.
fn mass_properties(kind: BodyKind, collider: Option<&Collider>) -> (f32, f32, Vec2) {
    if kind != BodyKind::Dynamic {
        return (0.0, 0.0, Vec2::ZERO);
    }
    let data = collider.map(|c| mass_data(&c.shape, c.density));
    let Some(data) = data.filter(|data| data.mass > 0.0) else {
        return (1.0, 0.0, Vec2::ZERO);
    };
    let inv_inertia = if data.rotational_inertia > 0.0 {
        1.0 / data.rotational_inertia
    } else {
        0.0
    };
    (1.0 / data.mass, inv_inertia, data.center)
}

/// Applies `impulse` at the center of mass; no effect on non-dynamic bodies.
pub fn apply_impulse(body: &mut Body, collider: &Collider, impulse: Vec2) {
    let (inv_mass, _, _) = mass_properties(body.kind, Some(collider));
    body.linear_velocity += impulse * inv_mass;
}

/// Per body data that the solver does not touch.
struct Gathered<'c> {
    entity: Entity,
    pose: Pose,
    local_center: Vec2,
    /// Center of mass at the start of the step.
    center: Vec2,
    collider: Option<&'c Collider>,
}

impl Gathered<'_> {
    fn transform(&self) -> Transform {
        Transform {
            position: self.pose.position,
            rotation: self.pose.rotation,
        }
    }
}

pub(crate) fn physics_step(
    time: Res<Time>,
    settings: Res<PhysicsSettings>,
    mut cache: ResMut<ContactCache>,
    parallel: Runtime<Parallel>,
    mut bodies: Query<(Entity, &mut Pose, &mut Body)>,
    colliders: Query<&Collider>,
) {
    let mut gathered = Vec::new();
    let mut solver_bodies = Vec::new();
    let mut proxies = Vec::new();
    for (entity, pose, body) in bodies.iter_mut() {
        let collider = colliders.get(entity).ok();
        let (inv_mass, inv_inertia, local_center) = mass_properties(body.kind, collider);
        let index = solver_bodies.len();
        let item = Gathered {
            entity,
            pose: *pose,
            local_center,
            center: pose.position + pose.rotation.rotate(local_center),
            collider,
        };
        if let Some(collider) = collider {
            proxies.push(Proxy {
                aabb: collider
                    .shape
                    .compute_aabb(item.transform())
                    .fattened(SPECULATIVE_DISTANCE),
                body: index as u32,
                kind: body.kind,
            });
        }
        // A velocity typed into a scene must not turn a static body into a conveyor belt.
        let moving = body.kind != BodyKind::Static;
        solver_bodies.push(SolverBody {
            kind: body.kind,
            linear_velocity: if moving {
                body.linear_velocity
            } else {
                Vec2::ZERO
            },
            angular_velocity: if moving { body.angular_velocity } else { 0.0 },
            delta_position: Vec2::ZERO,
            delta_rotation: Rot::IDENTITY,
            inv_mass,
            inv_inertia,
            linear_damping: body.linear_damping,
            angular_damping: body.angular_damping,
            gravity_scale: body.gravity_scale,
        });
        gathered.push(item);
    }

    let mut next = BTreeMap::new();
    let mut constraints = Vec::new();
    let mut keys = Vec::new();
    for (a, b) in find_pairs(&proxies) {
        let (a, b) = (a as usize, b as usize);
        let (body_a, body_b) = (&gathered[a], &gathered[b]);
        let (Some(collider_a), Some(collider_b)) = (body_a.collider, body_b.collider) else {
            unreachable!("only bodies with a collider have a proxy");
        };
        let key = (body_a.entity, body_b.entity);
        let manifold = collide(
            &collider_a.shape,
            body_a.transform(),
            &collider_b.shape,
            body_b.transform(),
            SPECULATIVE_DISTANCE,
        );
        if manifold.count == 0 {
            next.insert(
                key,
                CachedContact {
                    touching: false,
                    points: Vec::new(),
                },
            );
            continue;
        }
        let previous = cache.pairs.get(&key);
        let offset_a = body_a.pose.rotation.rotate(body_a.local_center);
        let offset_b = body_b.pose.rotation.rotate(body_b.local_center);
        let mut points = [ConstraintPoint::default(); 2];
        for (point, manifold_point) in points.iter_mut().zip(&manifold.points[..manifold.count]) {
            let warm = previous.and_then(|contact| contact.point(manifold_point.id));
            *point = ConstraintPoint {
                id: manifold_point.id,
                anchor_a: manifold_point.anchor_a - offset_a,
                anchor_b: manifold_point.anchor_b - offset_b,
                separation: manifold_point.separation,
                normal_impulse: warm.map_or(0.0, |w| w.normal_impulse),
                tangent_impulse: warm.map_or(0.0, |w| w.tangent_impulse),
                ..ConstraintPoint::default()
            };
        }
        constraints.push(ContactConstraint {
            body_a: a,
            body_b: b,
            normal: manifold.normal,
            friction: (collider_a.friction * collider_b.friction).sqrt(),
            restitution: max(collider_a.restitution, collider_b.restitution),
            softness: RIGID,
            points,
            count: manifold.count,
        });
        keys.push(key);
    }

    let context = StepContext::new(
        time.dt,
        settings.substeps,
        settings.gravity,
        settings.contact_hertz,
        settings.contact_damping_ratio,
        settings.contact_push_max_velocity,
        settings.restitution_threshold,
    );
    solve(&mut solver_bodies, &mut constraints, &context, &parallel);

    for (key, constraint) in keys.into_iter().zip(&constraints) {
        let points = constraint
            .points()
            .iter()
            .map(|point| CachedPoint {
                id: point.id,
                normal_impulse: point.normal_impulse,
                tangent_impulse: point.tangent_impulse,
            })
            .collect();
        next.insert(
            key,
            CachedContact {
                touching: true,
                points,
            },
        );
    }
    cache.pairs = next;

    for (((entity, pose, body), item), solved) in
        bodies.iter_mut().zip(&gathered).zip(&solver_bodies)
    {
        debug_assert_eq!(entity, item.entity);
        if body.kind == BodyKind::Static {
            continue;
        }
        let rotation = solved.delta_rotation.mul(item.pose.rotation).normalize();
        let center = item.center + solved.delta_position;
        pose.position = center - rotation.rotate(item.local_center);
        pose.rotation = rotation;
        body.linear_velocity = solved.linear_velocity;
        body.angular_velocity = solved.angular_velocity;
        let finite = [
            pose.position.x,
            pose.position.y,
            pose.rotation.cos,
            pose.rotation.sin,
            body.linear_velocity.x,
            body.linear_velocity.y,
            body.angular_velocity,
        ]
        .iter()
        .all(|value| value.is_finite());
        assert!(
            finite,
            "physics produced a non-finite state for entity {entity}"
        );
    }
}
