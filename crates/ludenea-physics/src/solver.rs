//! Soft step contact solver (Box2D v3): substepped velocity solve with soft
//! constraints, relax iterations, and restitution, run as distinct stages over a body
//! array and a constraint array.

use crate::coloring::{GRAPH_COLOR_COUNT, SOLVER_CHUNK, color_constraints};
use crate::components::BodyKind;
use ludenea_app::Parallel;
use ludenea_math::{PI, Rot, Vec2, clamp, max, min};

/// Linear speed clamp, a guard against tunnelling and overflow.
const MAX_LINEAR_SPEED: f32 = 400.0;

/// Body state the solver reads and writes. Positions are deltas from the start of the
/// step, about the center of mass.
#[derive(Copy, Clone, Debug)]
pub(crate) struct SolverBody {
    pub kind: BodyKind,
    pub linear_velocity: Vec2,
    pub angular_velocity: f32,
    pub delta_position: Vec2,
    pub delta_rotation: Rot,
    pub inv_mass: f32,
    pub inv_inertia: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub gravity_scale: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Softness {
    pub bias_rate: f32,
    pub mass_scale: f32,
    pub impulse_scale: f32,
}

pub(crate) const RIGID: Softness = Softness {
    bias_rate: 0.0,
    mass_scale: 1.0,
    impulse_scale: 0.0,
};

pub(crate) fn softness(hertz: f32, zeta: f32, h: f32) -> Softness {
    if hertz == 0.0 {
        return RIGID;
    }
    let omega = 2.0 * PI * hertz;
    let a1 = 2.0 * zeta + h * omega;
    let a2 = h * omega * a1;
    let a3 = 1.0 / (1.0 + a2);
    Softness {
        bias_rate: omega / a1,
        mass_scale: a2 * a3,
        impulse_scale: a3,
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct ConstraintPoint {
    pub id: u16,
    /// Contact point relative to each body's center of mass, fixed for the step.
    pub anchor_a: Vec2,
    pub anchor_b: Vec2,
    /// Manifold separation; `prepare` turns it into the adjusted separation.
    pub separation: f32,
    pub normal_impulse: f32,
    pub tangent_impulse: f32,
    pub max_normal_impulse: f32,
    pub normal_mass: f32,
    pub tangent_mass: f32,
    /// Normal velocity before the step, for restitution.
    pub relative_velocity: f32,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ContactConstraint {
    /// Body indices, `body_a < body_b`.
    pub body_a: usize,
    pub body_b: usize,
    pub normal: Vec2,
    pub friction: f32,
    pub restitution: f32,
    pub softness: Softness,
    pub points: [ConstraintPoint; 2],
    pub count: usize,
}

impl ContactConstraint {
    pub fn points(&self) -> &[ConstraintPoint] {
        &self.points[..self.count]
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct StepContext {
    pub h: f32,
    pub inv_h: f32,
    pub substeps: u32,
    pub gravity: Vec2,
    pub contact_softness: Softness,
    pub static_softness: Softness,
    pub push_max_velocity: f32,
    pub restitution_threshold: f32,
}

impl StepContext {
    pub fn new(
        dt: f32,
        substeps: u32,
        gravity: Vec2,
        contact_hertz: f32,
        damping_ratio: f32,
        push_max_velocity: f32,
        restitution_threshold: f32,
    ) -> Self {
        let substeps = substeps.max(1);
        let h = dt / substeps as f32;
        let contact_hertz = min(contact_hertz, 0.25 / h);
        StepContext {
            h,
            inv_h: 1.0 / h,
            substeps,
            gravity,
            contact_softness: softness(contact_hertz, damping_ratio, h),
            static_softness: softness(2.0 * contact_hertz, damping_ratio, h),
            push_max_velocity,
            restitution_threshold,
        }
    }
}

/// Runs the whole soft step on `bodies` and `constraints`. Constraint stages run color by
/// color, so the result never depends on the thread count of `parallel`.
pub(crate) fn solve(
    bodies: &mut [SolverBody],
    constraints: &mut [ContactConstraint],
    context: &StepContext,
    parallel: &Parallel,
) {
    let pairs: Vec<(u32, u32)> = constraints
        .iter()
        .map(|c| (c.body_a as u32, c.body_b as u32))
        .collect();
    let is_dynamic: Vec<bool> = bodies.iter().map(|b| b.kind == BodyKind::Dynamic).collect();
    let colors = color_constraints(&pairs, &is_dynamic);
    let read: &[SolverBody] = bodies;
    parallel.for_each_mut(constraints, SOLVER_CHUNK, |c| {
        prepare(c, &read[c.body_a], &read[c.body_b], context)
    });
    let mut stage = |bodies: &mut [SolverBody], stage: &Stage| {
        solve_colored(bodies, constraints, &colors, parallel, stage)
    };
    for _ in 0..context.substeps {
        parallel.for_each_mut(bodies, SOLVER_CHUNK, |body| {
            integrate_velocity(body, context)
        });
        stage(bodies, &warm_start);
        stage(bodies, &|c, a, b| solve_contact(c, a, b, context, true));
        parallel.for_each_mut(bodies, SOLVER_CHUNK, |body| {
            integrate_position(body, context.h)
        });
        stage(bodies, &|c, a, b| solve_contact(c, a, b, context, false));
    }
    stage(bodies, &|c, a, b| {
        apply_restitution(c, a, b, context.restitution_threshold)
    });
}

/// A constraint stage, applied to a constraint and copies of its two bodies.
type Stage<'a> = dyn Fn(&mut ContactConstraint, &mut SolverBody, &mut SolverBody) + Sync + 'a;

/// The constraint and body velocities a stage produced for one constraint.
type Solved = (ContactConstraint, [(Vec2, f32); 2]);

fn solve_one(constraint: &ContactConstraint, bodies: &[SolverBody], stage: &Stage) -> Solved {
    let mut c = *constraint;
    let (mut a, mut b) = (bodies[c.body_a], bodies[c.body_b]);
    stage(&mut c, &mut a, &mut b);
    (
        c,
        [
            (a.linear_velocity, a.angular_velocity),
            (b.linear_velocity, b.angular_velocity),
        ],
    )
}

/// Writes a solved constraint back. Only dynamic bodies are written: static and kinematic
/// bodies are read only, which keeps writes within a color disjoint.
fn scatter(
    bodies: &mut [SolverBody],
    constraints: &mut [ContactConstraint],
    index: u32,
    (constraint, velocities): Solved,
) {
    for (body, (linear, angular)) in [constraint.body_a, constraint.body_b]
        .into_iter()
        .zip(velocities)
    {
        let body = &mut bodies[body];
        if body.kind == BodyKind::Dynamic {
            body.linear_velocity = linear;
            body.angular_velocity = angular;
        }
    }
    constraints[index as usize] = constraint;
}

/// Runs `stage` on every color in parallel then on the overflow group sequentially.
fn solve_colored(
    bodies: &mut [SolverBody],
    constraints: &mut [ContactConstraint],
    colors: &[Vec<u32>],
    parallel: &Parallel,
    stage: &Stage,
) {
    // ponytail: gather then scatter costs one copy per color; switch to disjoint unsafe writes if profiling shows it
    for color in &colors[..GRAPH_COLOR_COUNT] {
        let (read_bodies, read_constraints): (&[SolverBody], &[ContactConstraint]) =
            (bodies, constraints);
        let solved = parallel.map(color, SOLVER_CHUNK, |&index| {
            solve_one(&read_constraints[index as usize], read_bodies, stage)
        });
        for (&index, solved) in color.iter().zip(solved) {
            scatter(bodies, constraints, index, solved);
        }
    }
    for &index in &colors[GRAPH_COLOR_COUNT] {
        let solved = solve_one(&constraints[index as usize], bodies, stage);
        scatter(bodies, constraints, index, solved);
    }
}

fn prepare(c: &mut ContactConstraint, a: &SolverBody, b: &SolverBody, context: &StepContext) {
    c.softness = if a.inv_mass == 0.0 || b.inv_mass == 0.0 {
        context.static_softness
    } else {
        context.contact_softness
    };
    let normal = c.normal;
    let tangent = normal.right_perp();
    let (m_a, i_a, m_b, i_b) = (a.inv_mass, a.inv_inertia, b.inv_mass, b.inv_inertia);
    for point in &mut c.points[..c.count] {
        let (r_a, r_b) = (point.anchor_a, point.anchor_b);
        let rn_a = r_a.cross(normal);
        let rn_b = r_b.cross(normal);
        let k_normal = m_a + m_b + i_a * rn_a * rn_a + i_b * rn_b * rn_b;
        point.normal_mass = if k_normal > 0.0 { 1.0 / k_normal } else { 0.0 };
        let rt_a = r_a.cross(tangent);
        let rt_b = r_b.cross(tangent);
        let k_tangent = m_a + m_b + i_a * rt_a * rt_a + i_b * rt_b * rt_b;
        point.tangent_mass = if k_tangent > 0.0 {
            1.0 / k_tangent
        } else {
            0.0
        };
        point.separation -= (r_b - r_a).dot(normal);
        point.relative_velocity = normal.dot(relative_velocity(a, b, r_a, r_b));
        point.max_normal_impulse = 0.0;
    }
}

/// Applies gravity, damping, and the linear speed limit to a dynamic body, and the angular
/// speed limit to every moving body: a kinematic body spins as fast as a dynamic one at most.
fn integrate_velocity(body: &mut SolverBody, context: &StepContext) {
    if body.kind == BodyKind::Static {
        return;
    }
    let h = context.h;
    let mut w = body.angular_velocity;
    if body.kind == BodyKind::Dynamic {
        let mut v = context.gravity * (h * body.gravity_scale)
            + body.linear_velocity * (1.0 / (1.0 + h * body.linear_damping));
        w *= 1.0 / (1.0 + h * body.angular_damping);
        let speed_squared = v.dot(v);
        if speed_squared > MAX_LINEAR_SPEED * MAX_LINEAR_SPEED {
            v *= MAX_LINEAR_SPEED / speed_squared.sqrt();
        }
        body.linear_velocity = v;
    }
    // At most an eighth of a turn per substep keeps the small-angle rotation integration
    // accurate.
    let max_angular_speed = 0.25 * PI * context.inv_h;
    if w * w > max_angular_speed * max_angular_speed {
        w *= max_angular_speed / w.abs();
    }
    body.angular_velocity = w;
}

fn integrate_position(body: &mut SolverBody, h: f32) {
    if body.kind == BodyKind::Static {
        return;
    }
    body.delta_position += body.linear_velocity * h;
    body.delta_rotation = body.delta_rotation.integrate(h * body.angular_velocity);
}

fn warm_start(c: &mut ContactConstraint, a: &mut SolverBody, b: &mut SolverBody) {
    let tangent = c.normal.right_perp();
    for point in c.points() {
        let impulse = c.normal * point.normal_impulse + tangent * point.tangent_impulse;
        apply(a, b, point, impulse);
    }
}

fn solve_contact(
    c: &mut ContactConstraint,
    a: &mut SolverBody,
    b: &mut SolverBody,
    context: &StepContext,
    use_bias: bool,
) {
    let normal = c.normal;
    let delta = b.delta_position - a.delta_position;
    for point in &mut c.points[..c.count] {
        let d = delta + b.delta_rotation.rotate(point.anchor_b)
            - a.delta_rotation.rotate(point.anchor_a);
        let s = d.dot(normal) + point.separation;
        let (bias, mass_scale, impulse_scale) = if s > 0.0 {
            // Speculative: allow the gap to close this substep, no more.
            (s * context.inv_h, 1.0, 0.0)
        } else if use_bias {
            (
                max(c.softness.bias_rate * s, -context.push_max_velocity),
                c.softness.mass_scale,
                c.softness.impulse_scale,
            )
        } else {
            (0.0, 1.0, 0.0)
        };
        let vn = normal.dot(relative_velocity(a, b, point.anchor_a, point.anchor_b));
        let impulse =
            -point.normal_mass * mass_scale * (vn + bias) - impulse_scale * point.normal_impulse;
        let accumulated = max(point.normal_impulse + impulse, 0.0);
        let impulse = accumulated - point.normal_impulse;
        point.normal_impulse = accumulated;
        point.max_normal_impulse = max(point.max_normal_impulse, impulse);
        apply(a, b, point, normal * impulse);
    }

    let tangent = normal.right_perp();
    for point in &mut c.points[..c.count] {
        let vt = tangent.dot(relative_velocity(a, b, point.anchor_a, point.anchor_b));
        let impulse = -point.tangent_mass * vt;
        let max_friction = c.friction * point.normal_impulse;
        let accumulated = clamp(point.tangent_impulse + impulse, -max_friction, max_friction);
        let impulse = accumulated - point.tangent_impulse;
        point.tangent_impulse = accumulated;
        apply(a, b, point, tangent * impulse);
    }
}

fn apply_restitution(
    c: &mut ContactConstraint,
    a: &mut SolverBody,
    b: &mut SolverBody,
    threshold: f32,
) {
    if c.restitution == 0.0 {
        return;
    }
    let normal = c.normal;
    for point in &mut c.points[..c.count] {
        if point.relative_velocity > -threshold || point.max_normal_impulse == 0.0 {
            continue;
        }
        let vn = normal.dot(relative_velocity(a, b, point.anchor_a, point.anchor_b));
        let impulse = -point.normal_mass * (vn + c.restitution * point.relative_velocity);
        let accumulated = max(point.normal_impulse + impulse, 0.0);
        let impulse = accumulated - point.normal_impulse;
        point.normal_impulse = accumulated;
        point.max_normal_impulse = max(point.max_normal_impulse, impulse);
        apply(a, b, point, normal * impulse);
    }
}

/// Velocity of B's contact point relative to A's.
fn relative_velocity(a: &SolverBody, b: &SolverBody, r_a: Vec2, r_b: Vec2) -> Vec2 {
    let v_a = a.linear_velocity + Vec2::cross_sv(a.angular_velocity, r_a);
    let v_b = b.linear_velocity + Vec2::cross_sv(b.angular_velocity, r_b);
    v_b - v_a
}

/// Applies `impulse` to B and its opposite to A at the point's anchors.
fn apply(a: &mut SolverBody, b: &mut SolverBody, point: &ConstraintPoint, impulse: Vec2) {
    a.linear_velocity -= impulse * a.inv_mass;
    a.angular_velocity -= a.inv_inertia * point.anchor_a.cross(impulse);
    b.linear_velocity += impulse * b.inv_mass;
    b.angular_velocity += b.inv_inertia * point.anchor_b.cross(impulse);
}
