use crate::shape::{Capsule, Circle, Polygon, Shape};
use ludenea_math::{PI, Vec2};

/// Mass properties; `rotational_inertia` is about `center`, not the body origin.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MassData {
    pub mass: f32,
    pub center: Vec2,
    pub rotational_inertia: f32,
}

pub fn mass_data(shape: &Shape, density: f32) -> MassData {
    match shape {
        Shape::Circle(c) => circle_mass(c, density),
        Shape::Capsule(c) => capsule_mass(c, density),
        Shape::Polygon(p) => polygon_mass(p, density),
    }
}

fn circle_mass(c: &Circle, density: f32) -> MassData {
    let rr = c.radius * c.radius;
    let mass = density * PI * rr;
    MassData {
        mass,
        center: c.center,
        rotational_inertia: mass * 0.5 * rr,
    }
}

fn capsule_mass(c: &Capsule, density: f32) -> MassData {
    let radius = c.radius;
    let rr = radius * radius;
    let length = (c.center2 - c.center1).length();
    let circle_mass = density * PI * rr;
    let box_mass = density * (2.0 * radius * length);
    // Distance from the semicircle's flat edge to its center of mass.
    let lc = 4.0 * radius / (3.0 * PI);
    let h = 0.5 * length;
    let circle_inertia = circle_mass * (0.5 * rr + h * h + 2.0 * h * lc);
    let box_inertia = box_mass * (4.0 * rr + length * length) / 12.0;
    MassData {
        mass: circle_mass + box_mass,
        center: (c.center1 + c.center2) * 0.5,
        rotational_inertia: circle_inertia + box_inertia,
    }
}

fn polygon_mass(p: &Polygon, density: f32) -> MassData {
    let count = p.vertices().len();
    let radius = p.radius();
    // Rounded polygons are approximated by pushing vertices out along the
    // averaged normals (Box2D v3).
    let mut points = [Vec2::ZERO; crate::shape::MAX_POLYGON_VERTICES];
    for (i, point) in points.iter_mut().enumerate().take(count) {
        *point = p.vertices()[i];
        if radius > 0.0 {
            let previous = if i == 0 { count - 1 } else { i - 1 };
            let mid = (p.normals()[previous] + p.normals()[i]).normalize();
            *point += mid * (1.412 * radius);
        }
    }
    let points = &points[..count];

    // Triangle fan around the centroid keeps the integrals well conditioned.
    const INV3: f32 = 1.0 / 3.0;
    let origin = p.centroid();
    let mut center = Vec2::ZERO;
    let mut area = 0.0;
    let mut inertia = 0.0;
    for i in 0..count {
        let e1 = points[i] - origin;
        let e2 = points[(i + 1) % count] - origin;
        let d = e1.cross(e2);
        let triangle_area = 0.5 * d;
        area += triangle_area;
        center += (e1 + e2) * (triangle_area * INV3);
        let intx2 = e1.x * e1.x + e2.x * e1.x + e2.x * e2.x;
        let inty2 = e1.y * e1.y + e2.y * e1.y + e2.y * e2.y;
        inertia += (0.25 * INV3 * d) * (intx2 + inty2);
    }
    let mass = density * area;
    let center = center * (1.0 / area);
    // Inertia so far is about `origin`; shift it to the center of mass.
    MassData {
        mass,
        center: origin + center,
        rotational_inertia: density * inertia - mass * center.dot(center),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tolerance: f32) {
        assert!((a - b).abs() <= tolerance, "{a} vs {b}");
    }

    #[test]
    fn circle_mass() {
        let m = mass_data(
            &Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 1.0,
            }),
            1.0,
        );
        close(m.mass, PI, 1e-6);
        close(m.rotational_inertia, 0.5 * PI, 1e-5);
    }

    #[test]
    fn offset_circle_inertia_is_about_its_center() {
        let m = mass_data(
            &Shape::Circle(Circle {
                center: Vec2::new(1.0, 2.0),
                radius: 1.0,
            }),
            1.0,
        );
        close(m.mass, PI, 1e-6);
        assert_eq!(m.center, Vec2::new(1.0, 2.0));
        close(m.rotational_inertia, 0.5 * PI, 1e-5);
    }

    #[test]
    fn box_mass() {
        let m = mass_data(&Shape::Polygon(Polygon::make_box(1.0, 1.0)), 1.0);
        close(m.mass, 4.0, 1e-5);
        close(m.rotational_inertia, 8.0 / 3.0, 1e-5);
        close(m.center.x, 0.0, 1e-6);
        close(m.center.y, 0.0, 1e-6);
    }

    #[test]
    fn offset_box_inertia_is_about_its_center() {
        let points = [
            Vec2::new(2.0, 3.0),
            Vec2::new(4.0, 3.0),
            Vec2::new(4.0, 5.0),
            Vec2::new(2.0, 5.0),
        ];
        let m = mass_data(&Shape::Polygon(Polygon::new(&points, 0.0).unwrap()), 1.0);
        close(m.center.x, 3.0, 1e-5);
        close(m.center.y, 4.0, 1e-5);
        close(m.rotational_inertia, 8.0 / 3.0, 1e-4);
    }

    #[test]
    fn capsule_mass() {
        let m = mass_data(
            &Shape::Capsule(Capsule {
                center1: Vec2::new(-1.0, 0.0),
                center2: Vec2::new(1.0, 0.0),
                radius: 0.5,
            }),
            1.0,
        );
        close(m.mass, PI * 0.25 + 2.0, 1e-5);
        assert_eq!(m.center, Vec2::ZERO);
        close(m.rotational_inertia, 2.05024, 1e-4);
    }
}
