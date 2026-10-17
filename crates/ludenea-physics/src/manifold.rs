//! Narrow phase: contact manifolds between pairs of shapes, following the
//! structure of Box2D v3 `manifold.c`.

use std::borrow::Cow;

use crate::shape::{Capsule, Circle, LINEAR_SLOP, MAX_POLYGON_VERTICES, Polygon, Shape};
use ludenea_math::{Rot, Transform, Vec2, clamp, max};

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ManifoldPoint {
    /// World space contact point.
    pub point: Vec2,
    /// `point` relative to the origin of body A.
    pub anchor_a: Vec2,
    /// `point` relative to the origin of body B.
    pub anchor_b: Vec2,
    /// Negative when the shapes overlap.
    pub separation: f32,
    /// Feature id, stable from frame to frame for warm starting.
    pub id: u16,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Manifold {
    /// Normal pointing from A to B: unit length, except zero when two circle
    /// centers coincide, so never divide by its length.
    pub normal: Vec2,
    pub points: [ManifoldPoint; 2],
    pub count: usize,
}

/// Set in a feature id when shape B owns the reference edge.
const FLIP_BIT: u16 = 0x8000;
/// SAT hysteresis and speculative fallback threshold.
const TOLERANCE: f32 = 0.1 * LINEAR_SLOP;
/// Sine of the largest angle (about 3 degrees) at which two capsule segments
/// count as parallel and get a two-point manifold.
const PARALLEL_SINE: f32 = 0.05;

/// Computes the contact manifold of `a` against `b`. Points whose separation
/// exceeds `speculative_distance` are dropped.
pub fn collide(
    a: &Shape,
    xf_a: Transform,
    b: &Shape,
    xf_b: Transform,
    speculative_distance: f32,
) -> Manifold {
    match (a, b) {
        (Shape::Circle(ca), Shape::Circle(cb)) => {
            collide_circles(ca, xf_a, cb, xf_b, speculative_distance)
        }
        (Shape::Capsule(ca), Shape::Circle(cb)) => collide_capsule_circle(
            ca.center1,
            ca.center2,
            ca.radius,
            xf_a,
            cb,
            xf_b,
            speculative_distance,
        ),
        (Shape::Circle(ca), Shape::Capsule(cb)) => swapped(collide_capsule_circle(
            cb.center1,
            cb.center2,
            cb.radius,
            xf_b,
            ca,
            xf_a,
            speculative_distance,
        )),
        (Shape::Polygon(pa), Shape::Circle(cb)) => {
            collide_polygon_circle(pa, xf_a, cb, xf_b, speculative_distance)
        }
        (Shape::Circle(ca), Shape::Polygon(pb)) => swapped(collide_polygon_circle(
            pb,
            xf_b,
            ca,
            xf_a,
            speculative_distance,
        )),
        (Shape::Capsule(ca), Shape::Capsule(cb)) => {
            collide_capsules(ca, xf_a, cb, xf_b, speculative_distance)
        }
        _ => collide_polygons(
            &as_polygon(a),
            xf_a,
            &as_polygon(b),
            xf_b,
            speculative_distance,
        ),
    }
}

/// Capsules collide with polygons as two-vertex rounded polygons.
fn as_polygon(shape: &Shape) -> Cow<'_, Polygon> {
    match shape {
        Shape::Polygon(p) => Cow::Borrowed(p),
        Shape::Capsule(c) => Cow::Owned(Polygon::from_segment(c.center1, c.center2, c.radius)),
        Shape::Circle(_) => unreachable!("circle pairs are dispatched before polygon pairs"),
    }
}

/// Turns a B-versus-A manifold into the A-versus-B one.
fn swapped(mut m: Manifold) -> Manifold {
    m.normal = -m.normal;
    for p in &mut m.points[..m.count] {
        std::mem::swap(&mut p.anchor_a, &mut p.anchor_b);
    }
    m
}

/// Converts a manifold computed in `frame` (attached to body A; only the
/// `normal`, `point`, `separation`, and `id` fields are read) to world space,
/// dropping points beyond the speculative distance and sorting by id.
fn to_world(
    frame: Transform,
    xf_a: Transform,
    xf_b: Transform,
    local: Manifold,
    speculative_distance: f32,
) -> Manifold {
    let mut m = Manifold {
        normal: frame.rotation.rotate(local.normal),
        ..Manifold::default()
    };
    for p in &local.points[..local.count] {
        if p.separation > speculative_distance {
            continue;
        }
        let anchor_a = frame.rotation.rotate(p.point) + (frame.position - xf_a.position);
        m.points[m.count] = ManifoldPoint {
            point: xf_a.position + anchor_a,
            anchor_a,
            anchor_b: anchor_a + (xf_a.position - xf_b.position),
            separation: p.separation,
            id: p.id,
        };
        m.count += 1;
    }
    if m.count == 2 && m.points[1].id < m.points[0].id {
        m.points.swap(0, 1);
    }
    m
}

fn single_point(normal: Vec2, point: Vec2, separation: f32, id: u16) -> Manifold {
    Manifold {
        normal,
        points: [
            ManifoldPoint {
                point,
                separation,
                id,
                ..ManifoldPoint::default()
            },
            ManifoldPoint::default(),
        ],
        count: 1,
    }
}

/// Contact between two rounded points given in A's frame.
fn round_contact(
    xf_a: Transform,
    xf_b: Transform,
    center_a: Vec2,
    radius_a: f32,
    center_b: Vec2,
    radius_b: f32,
    speculative_distance: f32,
) -> Manifold {
    let offset = center_b - center_a;
    let normal = offset.normalize();
    let separation = offset.length() - radius_a - radius_b;
    let contact = Vec2::lerp(
        center_a + normal * radius_a,
        center_b - normal * radius_b,
        0.5,
    );
    let local = single_point(normal, contact, separation, 0);
    to_world(xf_a, xf_a, xf_b, local, speculative_distance)
}

fn collide_circles(
    a: &Circle,
    xf_a: Transform,
    b: &Circle,
    xf_b: Transform,
    speculative_distance: f32,
) -> Manifold {
    let center_b = xf_a.inv_mul(xf_b).apply(b.center);
    round_contact(
        xf_a,
        xf_b,
        a.center,
        a.radius,
        center_b,
        b.radius,
        speculative_distance,
    )
}

fn collide_capsule_circle(
    p1: Vec2,
    p2: Vec2,
    radius_a: f32,
    xf_a: Transform,
    b: &Circle,
    xf_b: Transform,
    speculative_distance: f32,
) -> Manifold {
    let center_b = xf_a.inv_mul(xf_b).apply(b.center);
    let e = p2 - p1;
    let closest = if (center_b - p1).dot(e) < 0.0 {
        p1
    } else if (p2 - center_b).dot(e) < 0.0 {
        p2
    } else {
        p1 + e * ((center_b - p1).dot(e) / e.dot(e))
    };
    round_contact(
        xf_a,
        xf_b,
        closest,
        radius_a,
        center_b,
        b.radius,
        speculative_distance,
    )
}

fn collide_polygon_circle(
    a: &Polygon,
    xf_a: Transform,
    b: &Circle,
    xf_b: Transform,
    speculative_distance: f32,
) -> Manifold {
    let c = xf_a.inv_mul(xf_b).apply(b.center);
    let (vertices, normals) = (a.vertices(), a.normals());
    let (radius_a, radius_b) = (a.radius(), b.radius);

    let mut edge = 0;
    let mut separation = -f32::MAX;
    for i in 0..vertices.len() {
        let s = normals[i].dot(c - vertices[i]);
        if s > separation {
            separation = s;
            edge = i;
        }
    }
    if separation - radius_a - radius_b > speculative_distance {
        return Manifold::default();
    }

    let v1 = vertices[edge];
    let v2 = vertices[(edge + 1) % vertices.len()];
    if separation > f32::EPSILON && (c - v1).dot(v2 - v1) < 0.0 {
        return round_contact(xf_a, xf_b, v1, radius_a, c, radius_b, speculative_distance);
    }
    if separation > f32::EPSILON && (c - v2).dot(v1 - v2) < 0.0 {
        return round_contact(xf_a, xf_b, v2, radius_a, c, radius_b, speculative_distance);
    }
    let normal = normals[edge];
    let on_a = c - normal * (normal.dot(c - v1) - radius_a);
    let on_b = c - normal * radius_b;
    let local = single_point(
        normal,
        Vec2::lerp(on_a, on_b, 0.5),
        (on_b - on_a).dot(normal),
        0,
    );
    to_world(xf_a, xf_a, xf_b, local, speculative_distance)
}

/// Capsule pair by segment-segment closest points (SAT on two segments has
/// no axis along them, so collinear capsules cannot go through polygons).
/// Near-parallel segments that overlap along A's axis clip B onto A for two
/// points; every other configuration gives the single closest pair.
fn collide_capsules(
    a: &Capsule,
    xf_a: Transform,
    b: &Capsule,
    xf_b: Transform,
    speculative_distance: f32,
) -> Manifold {
    let frame = Transform {
        position: xf_a.apply(a.center1),
        rotation: xf_a.rotation,
    };
    let xf = frame.inv_mul(xf_b);
    let (p1, q1) = (Vec2::ZERO, a.center2 - a.center1);
    let (p2, q2) = (xf.apply(b.center1), xf.apply(b.center2));
    let (radius_a, radius_b) = (a.radius, b.radius);

    let (f1, f2) = segment_fractions(p1, q1, p2, q2);
    let closest1 = Vec2::lerp(p1, q1, f1);
    let closest2 = Vec2::lerp(p2, q2, f2);
    let offset = closest2 - closest1;
    let distance = offset.length();
    if distance - radius_a - radius_b > speculative_distance {
        return Manifold::default();
    }

    let (d1, d2) = (q1 - p1, q2 - p2);
    let (length1, length2) = (d1.length(), d2.length());
    let (u1, u2) = (d1.normalize(), d2.normalize());
    let outside =
        |fp: f32, fq: f32, length: f32| (fp <= 0.0 && fq <= 0.0) || (fp >= length && fq >= length);
    let (fp2, fq2) = ((p2 - p1).dot(u1), (q2 - p1).dot(u1));
    let outside_a = outside(fp2, fq2, length1);
    let outside_b = outside((p1 - p2).dot(u2), (q1 - p2).dot(u2), length2);
    let parallel = u1.cross(u2).abs() <= PARALLEL_SINE;

    // A positive distance means the segments do not cross, so the clipped
    // part of B lies on one side of A.
    let local = if parallel && !outside_a && !outside_b && distance > f32::EPSILON {
        clip_capsule_segments(
            p1,
            u1,
            length1,
            [p2, q2],
            [fp2, fq2],
            offset,
            radius_a,
            radius_b,
        )
    } else {
        let normal = if distance > f32::EPSILON {
            offset * (1.0 / distance)
        } else {
            u1.left_perp()
        };
        let contact = Vec2::lerp(
            closest1 + normal * radius_a,
            closest2 - normal * radius_b,
            0.5,
        );
        let nearest_end = |f: f32| usize::from(f >= 0.5);
        single_point(
            normal,
            contact,
            distance - radius_a - radius_b,
            make_id(nearest_end(f1), nearest_end(f2)),
        )
    };
    to_world(frame, xf_a, xf_b, local, speculative_distance)
}

/// Clips segment B (`ends`, projected at `projections` along `u1`) to the
/// span `[0, length1]` of segment A, which starts at `p1`. Ids follow the
/// polygon convention: A's edge facing B (as `Polygon::from_segment` numbers
/// it) and B's segment end each point comes from.
#[allow(clippy::too_many_arguments)]
fn clip_capsule_segments(
    p1: Vec2,
    u1: Vec2,
    length1: f32,
    ends: [Vec2; 2],
    projections: [f32; 2],
    offset: Vec2,
    radius_a: f32,
    radius_b: f32,
) -> Manifold {
    let (lower_end, upper_end) = if projections[0] <= projections[1] {
        (0, 1)
    } else {
        (1, 0)
    };
    let (lower, upper) = (ends[lower_end], ends[upper_end]);
    let (t_lower, t_upper) = (projections[lower_end], projections[upper_end]);
    let span = t_upper - t_lower;
    let v_lower = if t_lower < 0.0 && span > f32::EPSILON {
        Vec2::lerp(lower, upper, -t_lower / span)
    } else {
        lower
    };
    let v_upper = if t_upper > length1 && span > f32::EPSILON {
        Vec2::lerp(lower, upper, (length1 - t_lower) / span)
    } else {
        upper
    };

    // Edge 0 of a segment polygon faces right of `u1`, edge 1 faces left.
    let (normal, edge) = if u1.left_perp().dot(offset) >= 0.0 {
        (u1.left_perp(), 1)
    } else {
        (u1.right_perp(), 0)
    };
    let point = |v: Vec2, end: usize| {
        let separation = (v - p1).dot(normal);
        ManifoldPoint {
            point: v + normal * (0.5 * (radius_a - radius_b - separation)),
            separation: separation - radius_a - radius_b,
            id: make_id(edge, end),
            ..ManifoldPoint::default()
        }
    };
    Manifold {
        normal,
        points: [point(v_lower, lower_end), point(v_upper, upper_end)],
        count: 2,
    }
}

/// Polygon expressed in the shared collision frame.
struct LocalPolygon {
    vertices: [Vec2; MAX_POLYGON_VERTICES],
    normals: [Vec2; MAX_POLYGON_VERTICES],
    count: usize,
    radius: f32,
}

impl LocalPolygon {
    fn new(polygon: &Polygon, xf: Transform) -> LocalPolygon {
        let mut local = LocalPolygon {
            vertices: [Vec2::ZERO; MAX_POLYGON_VERTICES],
            normals: [Vec2::ZERO; MAX_POLYGON_VERTICES],
            count: polygon.vertices().len(),
            radius: polygon.radius(),
        };
        for (i, (v, n)) in polygon.vertices().iter().zip(polygon.normals()).enumerate() {
            local.vertices[i] = xf.apply(*v);
            local.normals[i] = xf.rotation.rotate(*n);
        }
        local
    }

    fn next(&self, i: usize) -> usize {
        if i + 1 < self.count { i + 1 } else { 0 }
    }
}

/// Both polygons in a frame anchored at A's first vertex, which keeps the
/// coordinates small and precise. Returns that frame in world space.
fn local_pair(
    a: &Polygon,
    xf_a: Transform,
    b: &Polygon,
    xf_b: Transform,
) -> (Transform, LocalPolygon, LocalPolygon) {
    let origin = a.vertices()[0];
    let frame = Transform {
        position: xf_a.apply(origin),
        rotation: xf_a.rotation,
    };
    let shift = Transform {
        position: -origin,
        rotation: Rot::IDENTITY,
    };
    (
        frame,
        LocalPolygon::new(a, shift),
        LocalPolygon::new(b, frame.inv_mul(xf_b)),
    )
}

/// Edge of `p1` with the largest separation from `p2`, and that separation.
fn find_max_separation(p1: &LocalPolygon, p2: &LocalPolygon) -> (usize, f32) {
    let mut best = (0, -f32::MAX);
    for i in 0..p1.count {
        let (n, v) = (p1.normals[i], p1.vertices[i]);
        let mut separation = f32::MAX;
        for w in &p2.vertices[..p2.count] {
            let s = n.dot(*w - v);
            if s < separation {
                separation = s;
            }
        }
        if separation > best.1 {
            best = (i, separation);
        }
    }
    best
}

/// Edge of `incident` most anti-parallel to `direction`.
fn find_incident_edge(direction: Vec2, incident: &LocalPolygon) -> usize {
    let mut best = (0, f32::MAX);
    for (i, n) in incident.normals[..incident.count].iter().enumerate() {
        let d = direction.dot(*n);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

fn make_id(reference: usize, incident: usize) -> u16 {
    ((reference as u16) << 8) | incident as u16
}

fn collide_polygons(
    a: &Polygon,
    xf_a: Transform,
    b: &Polygon,
    xf_b: Transform,
    speculative_distance: f32,
) -> Manifold {
    let (frame, local_a, local_b) = local_pair(a, xf_a, b, xf_b);
    let (edge_a, separation_a) = find_max_separation(&local_a, &local_b);
    let (edge_b, separation_b) = find_max_separation(&local_b, &local_a);
    let radius = local_a.radius + local_b.radius;
    if separation_a > speculative_distance + radius || separation_b > speculative_distance + radius
    {
        return Manifold::default();
    }

    // Hysteresis: B takes the reference face only when clearly better, so
    // resting stacks keep their reference faces and their warm starting.
    let flip = separation_b > separation_a + TOLERANCE;
    let (reference, incident, reference_edge, flag) = if flip {
        (&local_b, &local_a, edge_b, FLIP_BIT)
    } else {
        (&local_a, &local_b, edge_a, 0)
    };
    let incident_edge = find_incident_edge(reference.normals[reference_edge], incident);

    // Above the threshold the cores are disjoint, so a vertex pair has a
    // well defined normal.
    let corner = if max(separation_a, separation_b) > TOLERANCE {
        corner_contact(reference, reference_edge, incident, incident_edge, flag)
    } else {
        None
    };
    let mut local = corner
        .unwrap_or_else(|| clip_polygons(reference, reference_edge, incident, incident_edge, flag));
    if flip {
        local.normal = -local.normal;
    }
    to_world(frame, xf_a, xf_b, local, speculative_distance)
}

/// Fractions along `p1 -> q1` and `p2 -> q2` of their closest points. Both
/// segments have a non-zero length (polygon edges).
fn segment_fractions(p1: Vec2, q1: Vec2, p2: Vec2, q2: Vec2) -> (f32, f32) {
    let (d1, d2, r) = (q1 - p1, q2 - p2, p1 - p2);
    let (dd1, dd2) = (d1.dot(d1), d2.dot(d2));
    let (rd1, rd2, d12) = (r.dot(d1), r.dot(d2), d1.dot(d2));
    let denominator = dd1 * dd2 - d12 * d12;
    let f1 = if denominator != 0.0 {
        clamp((d12 * rd2 - rd1 * dd2) / denominator, 0.0, 1.0)
    } else {
        0.0
    };
    let f2 = (d12 * f1 + rd2) / dd2;
    if f2 < 0.0 {
        (clamp(-rd1 / dd1, 0.0, 1.0), 0.0)
    } else if f2 > 1.0 {
        (clamp((d12 - rd1) / dd1, 0.0, 1.0), 1.0)
    } else {
        (f1, f2)
    }
}

/// Single point contact when the closest features of the disjoint cores are
/// two vertices and the incident one lies outside the reference edge region.
fn corner_contact(
    reference: &LocalPolygon,
    i11: usize,
    incident: &LocalPolygon,
    i21: usize,
    flag: u16,
) -> Option<Manifold> {
    let (i12, i22) = (reference.next(i11), incident.next(i21));
    let (v11, v12) = (reference.vertices[i11], reference.vertices[i12]);
    let (v21, v22) = (incident.vertices[i21], incident.vertices[i22]);
    let (reference_vertex, incident_vertex) = match segment_fractions(v11, v12, v21, v22) {
        (0.0, 0.0) => (i11, i21),
        (0.0, 1.0) => (i11, i22),
        (1.0, 0.0) => (i12, i21),
        (1.0, 1.0) => (i12, i22),
        _ => return None,
    };
    let (p1, p2) = (
        reference.vertices[reference_vertex],
        incident.vertices[incident_vertex],
    );

    // Parallel overlapping edges also report endpoint pairs; those are face
    // contacts and go through clipping.
    let tangent = reference.normals[i11].left_perp();
    let t = (p2 - v11).dot(tangent);
    if t >= -TOLERANCE && t <= (v12 - v11).dot(tangent) + TOLERANCE {
        return None;
    }

    let offset = p2 - p1;
    let normal = offset.normalize();
    let on_reference = p1 + normal * reference.radius;
    let on_incident = p2 - normal * incident.radius;
    Some(single_point(
        normal,
        Vec2::lerp(on_reference, on_incident, 0.5),
        offset.length() - reference.radius - incident.radius,
        make_id(reference_vertex, incident_vertex) | flag,
    ))
}

/// Clips the incident edge against the side planes of the reference edge.
/// The normal is the reference edge normal.
fn clip_polygons(
    reference: &LocalPolygon,
    i11: usize,
    incident: &LocalPolygon,
    i21: usize,
    flag: u16,
) -> Manifold {
    let (i12, i22) = (reference.next(i11), incident.next(i21));
    let (v11, v12) = (reference.vertices[i11], reference.vertices[i12]);
    let (v21, v22) = (incident.vertices[i21], incident.vertices[i22]);
    let normal = reference.normals[i11];
    let tangent = normal.left_perp();

    // The reference edge spans [0, upper1] along the tangent; the incident
    // edge runs the opposite way because of the counter-clockwise winding.
    let upper1 = (v12 - v11).dot(tangent);
    let upper2 = (v21 - v11).dot(tangent);
    let lower2 = (v22 - v11).dot(tangent);
    let span = upper2 - lower2;
    let v_lower = if lower2 < 0.0 && span > f32::EPSILON {
        Vec2::lerp(v22, v21, -lower2 / span)
    } else {
        v22
    };
    let v_upper = if upper2 > upper1 && span > f32::EPSILON {
        Vec2::lerp(v22, v21, (upper1 - lower2) / span)
    } else {
        v21
    };

    let (r1, r2) = (reference.radius, incident.radius);
    let point = |v: Vec2, incident_vertex: usize| {
        let separation = (v - v11).dot(normal);
        ManifoldPoint {
            // Midway between the two rounded surfaces.
            point: v + normal * (0.5 * (r1 - r2 - separation)),
            separation: separation - r1 - r2,
            id: make_id(i11, incident_vertex) | flag,
            ..ManifoldPoint::default()
        }
    };
    Manifold {
        normal,
        points: [point(v_lower, i22), point(v_upper, i21)],
        count: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::Capsule;
    use ludenea_math::{PI, Rot};

    const SPECULATIVE: f32 = 4.0 * LINEAR_SLOP;

    fn at(x: f32, y: f32) -> Transform {
        Transform {
            position: Vec2::new(x, y),
            rotation: Rot::IDENTITY,
        }
    }

    fn circle(radius: f32) -> Shape {
        Shape::Circle(Circle {
            center: Vec2::ZERO,
            radius,
        })
    }

    fn unit_box() -> Shape {
        Shape::Polygon(Polygon::make_box(0.5, 0.5))
    }

    fn assert_near(actual: Vec2, expected: Vec2, tolerance: f32) {
        assert!(
            (actual.x - expected.x).abs() <= tolerance
                && (actual.y - expected.y).abs() <= tolerance,
            "{actual:?} is not within {tolerance} of {expected:?}"
        );
    }

    #[test]
    fn overlapping_circles() {
        let m = collide(
            &circle(1.0),
            at(0.0, 0.0),
            &circle(1.0),
            at(1.5, 0.0),
            SPECULATIVE,
        );
        assert_eq!(m.count, 1);
        assert_near(m.normal, Vec2::new(1.0, 0.0), 1e-6);
        assert!((m.points[0].separation + 0.5).abs() <= 1e-6);
        assert_near(m.points[0].point, Vec2::new(0.75, 0.0), 1e-6);
        assert_near(m.points[0].anchor_a, Vec2::new(0.75, 0.0), 1e-6);
        assert_near(m.points[0].anchor_b, Vec2::new(-0.75, 0.0), 1e-6);
    }

    #[test]
    fn distant_circles_have_no_points() {
        let m = collide(&circle(1.0), at(0.0, 0.0), &circle(1.0), at(3.0, 0.0), 0.02);
        assert_eq!(m.count, 0);
    }

    #[test]
    fn box_resting_on_box_gives_two_points() {
        let m = collide(
            &unit_box(),
            at(0.0, 0.0),
            &unit_box(),
            at(0.0, 0.9),
            SPECULATIVE,
        );
        assert_eq!(m.count, 2);
        assert_near(m.normal, Vec2::new(0.0, 1.0), 1e-6);
        for p in &m.points {
            assert!((p.separation + 0.1).abs() <= 1e-5, "{p:?}");
            assert!((p.point.y - 0.45).abs() <= 1e-5, "{p:?}");
            assert!((p.point.x.abs() - 0.5).abs() <= 1e-5, "{p:?}");
        }
        assert!(m.points[0].id < m.points[1].id);
    }

    #[test]
    fn circle_on_box_face() {
        let ball = circle(0.5);
        let m = collide(&unit_box(), at(0.0, 0.0), &ball, at(0.2, 0.95), SPECULATIVE);
        assert_eq!(m.count, 1);
        assert_near(m.normal, Vec2::new(0.0, 1.0), 1e-6);
        assert!((m.points[0].separation + 0.05).abs() <= 1e-5);
        assert_near(m.points[0].point, Vec2::new(0.2, 0.475), 1e-5);

        let swapped = collide(&ball, at(0.2, 0.95), &unit_box(), at(0.0, 0.0), SPECULATIVE);
        assert_eq!(swapped.count, 1);
        assert_near(swapped.normal, Vec2::new(0.0, -1.0), 1e-6);
        assert_near(swapped.points[0].anchor_a, m.points[0].anchor_b, 1e-6);
        assert_near(swapped.points[0].anchor_b, m.points[0].anchor_a, 1e-6);
    }

    #[test]
    fn capsule_lying_on_box_gives_two_points() {
        let capsule = Shape::Capsule(Capsule {
            center1: Vec2::new(-0.3, 0.0),
            center2: Vec2::new(0.3, 0.0),
            radius: 0.25,
        });
        let m = collide(
            &unit_box(),
            at(0.0, 0.0),
            &capsule,
            at(0.0, 0.74),
            SPECULATIVE,
        );
        assert_eq!(m.count, 2);
        assert_near(m.normal, Vec2::new(0.0, 1.0), 1e-6);
        for p in &m.points {
            assert!((p.separation + 0.01).abs() <= 1e-5, "{p:?}");
            assert!((p.point.y - 0.495).abs() <= 1e-5, "{p:?}");
            assert!((p.point.x.abs() - 0.3).abs() <= 1e-5, "{p:?}");
        }

        // Exactly as wide as the box: the closest features are an endpoint
        // pair of parallel edges, which is still a face contact.
        let flush = Shape::Capsule(Capsule {
            center1: Vec2::new(-0.5, 0.0),
            center2: Vec2::new(0.5, 0.0),
            radius: 0.25,
        });
        let m = collide(
            &unit_box(),
            at(0.0, 0.0),
            &flush,
            at(0.0, 0.74),
            SPECULATIVE,
        );
        assert_eq!(m.count, 2);
    }

    fn segment_capsule(radius: f32) -> Shape {
        Shape::Capsule(Capsule {
            center1: Vec2::new(-0.5, 0.0),
            center2: Vec2::new(0.5, 0.0),
            radius,
        })
    }

    #[test]
    fn collinear_capsules_end_to_end_give_one_point() {
        let capsule = segment_capsule(0.2);
        let m = collide(&capsule, at(0.0, 0.0), &capsule, at(1.41, 0.0), SPECULATIVE);
        assert_eq!(m.count, 1);
        assert_near(m.normal, Vec2::new(1.0, 0.0), 1e-5);
        assert!((m.points[0].separation - 0.01).abs() <= 1e-5, "{m:?}");
        assert_near(m.points[0].point, Vec2::new(0.705, 0.0), 1e-5);
    }

    #[test]
    fn parallel_stacked_capsules_give_two_points() {
        let capsule = segment_capsule(0.2);
        let m = collide(&capsule, at(0.0, 0.0), &capsule, at(0.2, 0.39), SPECULATIVE);
        assert_eq!(m.count, 2);
        assert_near(m.normal, Vec2::new(0.0, 1.0), 1e-6);
        assert!(m.points[0].id < m.points[1].id);
        let mut xs = [m.points[0].point.x, m.points[1].point.x];
        xs.sort_by(f32::total_cmp);
        assert_near(Vec2::new(xs[0], xs[1]), Vec2::new(-0.3, 0.5), 1e-5);
        for p in &m.points {
            assert!((p.separation + 0.01).abs() <= 1e-5, "{p:?}");
            assert!((p.point.y - 0.195).abs() <= 1e-5, "{p:?}");
        }
    }

    #[test]
    fn crossing_capsules_give_one_point() {
        let capsule = segment_capsule(0.2);
        let upright = |y| Transform {
            position: Vec2::new(0.1, y),
            rotation: Rot::from_angle(PI / 2.0),
        };
        let crossing = collide(&capsule, at(0.0, 0.0), &capsule, upright(0.1), SPECULATIVE);
        assert_eq!(crossing.count, 1);
        assert!((crossing.points[0].separation + 0.4).abs() <= 1e-5);

        // Upright capsule standing on the middle of a lying one.
        let standing = collide(&capsule, at(0.0, 0.0), &capsule, upright(0.85), SPECULATIVE);
        assert_eq!(standing.count, 1);
        assert_near(standing.normal, Vec2::new(0.0, 1.0), 1e-5);
        assert!((standing.points[0].separation + 0.05).abs() <= 1e-5);
    }

    #[test]
    fn rotated_box_corner_contact_gives_one_point() {
        let half_diagonal = 0.5 * 2.0f32.sqrt();
        let xf_b = Transform {
            position: Vec2::new(0.1, 0.5 + half_diagonal - 0.05),
            rotation: Rot::from_angle(PI / 4.0),
        };
        let m = collide(&unit_box(), at(0.0, 0.0), &unit_box(), xf_b, SPECULATIVE);
        assert_eq!(m.count, 1);
        assert_near(m.normal, Vec2::new(0.0, 1.0), 1e-5);
        assert!((m.points[0].separation + 0.05).abs() <= 1e-4);
        assert_near(m.points[0].point, Vec2::new(0.1, 0.475), 1e-4);
    }

    struct XorShift(u32);

    impl XorShift {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            (self.0 >> 8) as f32 / (1u32 << 24) as f32
        }

        fn range(&mut self, low: f32, high: f32) -> f32 {
            low + (high - low) * self.next()
        }
    }

    /// True when the result legitimately depends on argument order: SAT
    /// separations within the hysteresis band, or crossing capsule cores,
    /// whose fallback normal is perpendicular to A.
    fn order_dependent(a: &Shape, xf_a: Transform, b: &Shape, xf_b: Transform) -> bool {
        match (a, b) {
            (Shape::Circle(_), _) | (_, Shape::Circle(_)) => false,
            (Shape::Capsule(ca), Shape::Capsule(cb)) => {
                let (p1, q1) = (xf_a.apply(ca.center1), xf_a.apply(ca.center2));
                let (p2, q2) = (xf_b.apply(cb.center1), xf_b.apply(cb.center2));
                let (f1, f2) = segment_fractions(p1, q1, p2, q2);
                Vec2::lerp(p1, q1, f1).distance(Vec2::lerp(p2, q2, f2)) <= TOLERANCE
            }
            _ => {
                let tie = |a: &Shape, xf_a, b: &Shape, xf_b| {
                    let (_, local_a, local_b) =
                        local_pair(&as_polygon(a), xf_a, &as_polygon(b), xf_b);
                    let (_, separation_a) = find_max_separation(&local_a, &local_b);
                    let (_, separation_b) = find_max_separation(&local_b, &local_a);
                    (separation_a - separation_b).abs() <= TOLERANCE
                };
                tie(a, xf_a, b, xf_b) || tie(b, xf_b, a, xf_a)
            }
        }
    }

    #[test]
    fn swapping_arguments_negates_normal() {
        let shapes = [
            Shape::Circle(Circle {
                center: Vec2::new(0.1, 0.0),
                radius: 0.4,
            }),
            Shape::Capsule(Capsule {
                center1: Vec2::new(-0.4, 0.0),
                center2: Vec2::new(0.4, 0.1),
                radius: 0.2,
            }),
            Shape::Polygon(Polygon::make_box(0.5, 0.3)),
            Shape::Polygon(
                Polygon::new(
                    &[
                        Vec2::new(0.0, 0.0),
                        Vec2::new(0.8, 0.1),
                        Vec2::new(0.2, 0.7),
                    ],
                    0.05,
                )
                .unwrap(),
            ),
        ];
        let mut rng = XorShift(0x9E37_79B9);
        let mut contacts = 0;
        // Even iterations draw any pair; odd ones draw capsule pairs, every
        // other one near parallel so the two-point path is covered too.
        let (mut capsule_pairs, mut two_point_capsule_pairs) = (0, 0);
        for i in 0..200 {
            let (a, b) = if i % 2 == 0 {
                (
                    &shapes[(rng.next() * 4.0) as usize],
                    &shapes[(rng.next() * 4.0) as usize],
                )
            } else {
                (&shapes[1], &shapes[1])
            };
            let xf_a = Transform {
                position: Vec2::new(rng.range(-0.5, 0.5), rng.range(-0.5, 0.5)),
                rotation: Rot::from_angle(rng.range(-PI, PI)),
            };
            let rotation_b = if i % 4 == 1 {
                xf_a.rotation.mul(Rot::from_angle(rng.range(-0.04, 0.04)))
            } else {
                Rot::from_angle(rng.range(-PI, PI))
            };
            let xf_b = Transform {
                position: xf_a.position + Vec2::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)),
                rotation: rotation_b,
            };
            if order_dependent(a, xf_a, b, xf_b) {
                continue;
            }
            let ab = collide(a, xf_a, b, xf_b, SPECULATIVE);
            let ba = collide(b, xf_b, a, xf_a, SPECULATIVE);
            assert_eq!(ab.count, ba.count, "{a:?} {xf_a:?} {b:?} {xf_b:?}");
            if ab.count == 0 {
                continue;
            }
            contacts += 1;
            // Two-point capsule manifolds take the normal from the first
            // segment, so the orders differ by the angle between segments.
            let mut tolerance = 1e-5;
            if matches!((a, b), (Shape::Capsule(_), Shape::Capsule(_))) {
                capsule_pairs += 1;
                if ab.count == 2 {
                    two_point_capsule_pairs += 1;
                    tolerance = PARALLEL_SINE;
                }
            }
            assert_near(ab.normal, -ba.normal, tolerance);
        }

        assert!(contacts >= 40, "only {contacts} configurations touched");
        assert!(
            capsule_pairs >= 10,
            "only {capsule_pairs} capsule pairs touched"
        );
        assert!(
            two_point_capsule_pairs >= 3,
            "only {two_point_capsule_pairs} two-point capsule pairs"
        );
    }

    #[test]
    fn ids_are_stable_under_small_motion() {
        let before = collide(
            &unit_box(),
            at(0.0, 0.0),
            &unit_box(),
            at(0.0, 0.9),
            SPECULATIVE,
        );
        let after = collide(
            &unit_box(),
            at(0.0, 0.0),
            &unit_box(),
            at(0.001, 0.9),
            SPECULATIVE,
        );
        assert_eq!(before.count, 2);
        assert_eq!(after.count, 2);
        assert_eq!(before.points.map(|p| p.id), after.points.map(|p| p.id));
    }
}
