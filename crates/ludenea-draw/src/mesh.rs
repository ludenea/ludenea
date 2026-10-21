//! Triangle lists for the renderer's debug draw, built from a frame without a GPU.

use std::collections::BTreeSet;

use ludenea_ecs::Entity;

use ludenea_math::{PI, Rot, Vec2};

use crate::{Color, DebugDrawFrame, Primitive};

pub const CIRCLE_SEGMENTS: usize = 32;

/// Color of the primitives of selected entities.
pub const HIGHLIGHT: Color = Color(0xFFD400FF);

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 2],
    pub color: u32,
}

pub fn triangulate(
    frame: &DebugDrawFrame,
    layers: &BTreeSet<String>,
    selected: &BTreeSet<Entity>,
    line_width: f32,
) -> Vec<Vertex> {
    let mut vertices = Vec::new();
    for layer in layers {
        for item in frame.layers.get(layer).into_iter().flatten() {
            let highlighted = item.entity.is_some_and(|entity| selected.contains(&entity));
            let mut out = Painter {
                vertices: &mut vertices,
                width: line_width,
            };
            let paint = |color: Color| if highlighted { HIGHLIGHT.0 } else { color.0 };
            match &item.primitive {
                Primitive::Line { a, b, color } => out.segment(*a, *b, paint(*color)),
                Primitive::Circle {
                    center,
                    radius,
                    color,
                    fill,
                } => {
                    let color = paint(*color);
                    let ring = arc(*center, *radius, Vec2::new(1.0, 0.0), CIRCLE_SEGMENTS, true);
                    if *fill {
                        for (i, &from) in ring.iter().enumerate() {
                            out.triangle([*center, from, ring[(i + 1) % ring.len()]], color);
                        }
                    } else {
                        out.shape(&ring, false, color);
                    }
                }
                Primitive::Polygon {
                    points,
                    color,
                    fill,
                } => out.shape(points, *fill, paint(*color)),
                Primitive::Capsule {
                    a,
                    b,
                    radius,
                    color,
                } => out.shape(&capsule_outline(*a, *b, *radius), false, paint(*color)),
                Primitive::Label { .. } => {}
            }
        }
    }
    vertices
}

struct Painter<'a> {
    vertices: &'a mut Vec<Vertex>,
    width: f32,
}

impl Painter<'_> {
    fn triangle(&mut self, corners: [Vec2; 3], color: u32) {
        for corner in corners {
            self.vertices.push(Vertex {
                position: [corner.x, corner.y],
                color,
            });
        }
    }

    /// A quad of `width` centered on the segment from `a` to `b`, as two triangles with
    /// `h` the half-width offset to the left: `[a - h, a + h, b + h]`, `[a - h, b + h, b - h]`.
    fn segment(&mut self, a: Vec2, b: Vec2, color: u32) {
        let half = (b - a).normalize().left_perp() * (self.width * 0.5);
        self.triangle([a - half, a + half, b + half], color);
        self.triangle([a - half, b + half, b - half], color);
    }

    /// A closed outline, or a triangle fan from the first point (convex shapes only).
    fn shape(&mut self, points: &[Vec2], fill: bool, color: u32) {
        if fill {
            for i in 1..points.len().saturating_sub(1) {
                self.triangle([points[0], points[i], points[i + 1]], color);
            }
        } else {
            for (i, &from) in points.iter().enumerate() {
                self.segment(from, points[(i + 1) % points.len()], color);
            }
        }
    }
}

/// `segments` points on the circle around `center`, counter-clockwise from `start_direction`
/// (unit length); one more point to close a half circle when `full` is false.
fn arc(center: Vec2, radius: f32, start_direction: Vec2, segments: usize, full: bool) -> Vec<Vec2> {
    let total = if full { segments } else { segments + 1 };
    let sweep = if full { 2.0 * PI } else { PI };
    (0..total)
        .map(|k| {
            let angle = sweep * k as f32 / segments as f32;
            center + Rot::from_angle(angle).rotate(start_direction) * radius
        })
        .collect()
}

/// The closed outline of a capsule: two sides and a half circle around each end.
fn capsule_outline(a: Vec2, b: Vec2, radius: f32) -> Vec<Vec2> {
    let axis = (b - a).normalize();
    let axis = if axis == Vec2::ZERO {
        Vec2::new(1.0, 0.0)
    } else {
        axis
    };
    let side = axis.left_perp();
    let half = CIRCLE_SEGMENTS / 2;
    // Counter-clockwise: around b from the right side to the left side, then around a.
    let mut points = arc(b, radius, -side, half, false);
    points.extend(arc(a, radius, side, half, false));
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(layer: &str) -> BTreeSet<String> {
        BTreeSet::from([layer.to_string()])
    }

    #[test]
    fn filled_circle_has_32_triangles() {
        let mut frame = DebugDrawFrame::default();
        frame.push(
            "l",
            None,
            Primitive::Circle {
                center: Vec2::ZERO,
                radius: 1.0,
                color: Color(0x112233FF),
                fill: true,
            },
        );
        let vertices = triangulate(&frame, &all("l"), &BTreeSet::new(), 0.1);
        assert_eq!(vertices.len(), CIRCLE_SEGMENTS * 3);
        assert!(vertices.iter().all(|v| v.color == 0x112233FF));
    }

    #[test]
    fn capsule_outline_is_closed() {
        let mut frame = DebugDrawFrame::default();
        frame.push(
            "l",
            None,
            Primitive::Capsule {
                a: Vec2::new(-1.0, 0.0),
                b: Vec2::new(1.0, 0.0),
                radius: 0.5,
                color: Color(1),
            },
        );
        // Zero width collapses each quad onto its segment: vertices 0 and 2 of a quad are
        // the segment start and end, so consecutive segments must chain and wrap around.
        let vertices = triangulate(&frame, &all("l"), &BTreeSet::new(), 0.0);
        assert_eq!(vertices.len() % 6, 0);
        let quads: Vec<&[Vertex]> = vertices.chunks(6).collect();
        assert!(quads.len() > CIRCLE_SEGMENTS);
        for (i, quad) in quads.iter().enumerate() {
            let next = quads[(i + 1) % quads.len()];
            let (end, start) = (quad[2].position, next[0].position);
            assert!(
                (end[0] - start[0]).abs() < 1e-5 && (end[1] - start[1]).abs() < 1e-5,
                "segment {i} does not chain"
            );
        }
    }

    #[test]
    fn selected_entity_uses_highlight_color() {
        let mut world = ludenea_ecs::World::new();
        let picked = world.spawn();
        let other = world.spawn();
        let mut frame = DebugDrawFrame::default();
        for e in [picked, other] {
            frame.push(
                "l",
                Some(e),
                Primitive::Line {
                    a: Vec2::ZERO,
                    b: Vec2::new(1.0, 0.0),
                    color: Color(0x0000FFFF),
                },
            );
        }
        let vertices = triangulate(&frame, &all("l"), &BTreeSet::from([picked]), 0.1);
        assert_eq!(vertices.len(), 12);
        assert_eq!(vertices[0].color, HIGHLIGHT.0);
        assert_eq!(vertices[6].color, 0x0000FFFF);
    }

    #[test]
    fn labels_produce_no_vertices() {
        let mut frame = DebugDrawFrame::default();
        frame.push(
            "l",
            None,
            Primitive::Label {
                at: Vec2::ZERO,
                text: "x".into(),
                color: Color(1),
            },
        );
        assert!(triangulate(&frame, &all("l"), &BTreeSet::new(), 0.1).is_empty());
    }
}
