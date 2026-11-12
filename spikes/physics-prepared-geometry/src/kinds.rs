//! The shape kinds of the Rapier half: a triangle mesh with its acceleration
//! structure (with no flag, and with the flag the library asks for so that
//! bodies do not catch on the inner edges of a ground) and a convex hull. For
//! each: the library's call that builds the shape with no world, the prepared
//! form (the `bincode` bytes of the shape, which derives serde's traits under
//! the library's `serde-serialize` feature), and the world the behavior
//! measurement puts the shape in. Throwaway, never product code.

use std::time::Instant;

use rapier3d::math::Vector;
use rapier3d::parry::shape::{ConvexPolyhedron, TriMesh, TriMeshFlags};
use rapier3d::prelude::PhysicsWorld;

use crate::inputs::{self, Terrain};
use crate::worlds;

pub fn milliseconds_since(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

/// What the measurement needs from a shape kind.
pub trait PreparedKind {
    type Input;
    type Shape;

    fn kind(&self) -> &'static str;
    fn variant(&self) -> &'static str;
    /// The kind and its variant, for a progress line.
    fn label(&self) -> String {
        [self.kind(), self.variant()].join(" ").trim_end().to_string()
    }
    /// The name of the file that holds the form of the smallest ladder size.
    fn file(&self) -> &'static str;
    /// One or two sentences of fact on how the form is built, written and
    /// read, with what a built shape shows.
    fn how(&self, shape: &Self::Shape) -> String;

    fn input(&self, elements: usize) -> Self::Input;
    fn input_hash(&self, input: &Self::Input) -> u64;
    /// Builds the shape outside any world; the milliseconds are those of the
    /// library's call alone.
    fn build(&self, input: &Self::Input) -> Result<(f64, Self::Shape), String>;
    /// The count of what was built: triangles, or hull vertices.
    fn built(&self, shape: &Self::Shape) -> usize;

    /// The prepared form of a shape.
    fn form(&self, shape: &Self::Shape) -> Result<Vec<u8>, String>;
    /// The shape a prepared form holds.
    fn read(&self, form: &[u8]) -> Result<Self::Shape, String>;

    /// The world of the behavior measurement around a shape built from
    /// `elements` input elements.
    fn world(&self, shape: Self::Shape, elements: usize, bodies: usize) -> Result<PhysicsWorld, String>;
}

/// A triangle mesh with its bounding volume hierarchy, built from a terrain.
pub struct TerrainMesh {
    pub fixed_internal_edges: bool,
}

impl TerrainMesh {
    fn flags(&self) -> TriMeshFlags {
        if self.fixed_internal_edges { TriMeshFlags::FIX_INTERNAL_EDGES } else { TriMeshFlags::empty() }
    }
}

impl PreparedKind for TerrainMesh {
    type Input = Terrain;
    type Shape = TriMesh;

    fn kind(&self) -> &'static str {
        "triangle_mesh"
    }

    fn variant(&self) -> &'static str {
        if self.fixed_internal_edges { "fixed_internal_edges" } else { "no_flags" }
    }

    fn file(&self) -> &'static str {
        if self.fixed_internal_edges { "rapier-triangle-mesh-fixed-internal-edges.bin" } else { "rapier-triangle-mesh.bin" }
    }

    fn how(&self, shape: &TriMesh) -> String {
        format!(
            "parry3d's TriMesh::with_flags(vertices, indices, {}) builds the mesh and its bounding volume hierarchy with no world; TriMesh derives serde's Serialize and Deserialize under the serde-serialize feature, and the form is its bincode bytes, read back with bincode::deserialize. The built mesh holds pseudo-normals: {}; a half-edge topology: {}.",
            if self.fixed_internal_edges { "TriMeshFlags::FIX_INTERNAL_EDGES" } else { "TriMeshFlags::empty()" },
            shape.pseudo_normals().is_some(),
            shape.topology().is_some(),
        )
    }

    fn input(&self, elements: usize) -> Terrain {
        inputs::terrain(elements)
    }

    fn input_hash(&self, input: &Terrain) -> u64 {
        inputs::terrain_hash(input)
    }

    fn build(&self, input: &Terrain) -> Result<(f64, TriMesh), String> {
        // The library takes the buffers by value: they are copied outside the
        // timed region.
        let (vertices, indices) = (input.vertices.clone(), input.indices.clone());
        let start = Instant::now();
        let mesh = TriMesh::with_flags(vertices, indices, self.flags());
        let milliseconds = milliseconds_since(start);
        mesh.map(|mesh| (milliseconds, mesh)).map_err(|error| error.to_string())
    }

    fn built(&self, shape: &TriMesh) -> usize {
        shape.num_triangles()
    }

    fn form(&self, shape: &TriMesh) -> Result<Vec<u8>, String> {
        bincode::serialize(shape).map_err(|error| error.to_string())
    }

    fn read(&self, form: &[u8]) -> Result<TriMesh, String> {
        bincode::deserialize(form).map_err(|error| error.to_string())
    }

    fn world(&self, shape: TriMesh, elements: usize, bodies: usize) -> Result<PhysicsWorld, String> {
        Ok(worlds::mesh_world(shape, inputs::terrain_quads(elements), bodies))
    }
}

/// A convex polyhedron, the hull of a point cloud.
pub struct ConvexHull;

impl PreparedKind for ConvexHull {
    type Input = Vec<Vector>;
    type Shape = ConvexPolyhedron;

    fn kind(&self) -> &'static str {
        "convex_hull"
    }

    fn variant(&self) -> &'static str {
        ""
    }

    fn file(&self) -> &'static str {
        "rapier-convex-hull.bin"
    }

    fn how(&self, shape: &ConvexPolyhedron) -> String {
        format!(
            "parry3d's ConvexPolyhedron::from_convex_hull(points) computes the hull and its faces, edges and adjacency with no world; ConvexPolyhedron derives serde's Serialize and Deserialize under the serde-serialize feature, and the form is its bincode bytes, read back with bincode::deserialize. The hull of the smallest input has {} faces and {} edges.",
            shape.faces().len(),
            shape.edges().len(),
        )
    }

    fn input(&self, elements: usize) -> Vec<Vector> {
        inputs::cloud(elements)
    }

    fn input_hash(&self, input: &Vec<Vector>) -> u64 {
        inputs::cloud_hash(input)
    }

    fn build(&self, input: &Vec<Vector>) -> Result<(f64, ConvexPolyhedron), String> {
        let start = Instant::now();
        let hull = ConvexPolyhedron::from_convex_hull(input);
        let milliseconds = milliseconds_since(start);
        hull.map(|hull| (milliseconds, hull)).ok_or_else(|| String::from("the library returned no hull for the cloud"))
    }

    fn built(&self, shape: &ConvexPolyhedron) -> usize {
        shape.points().len()
    }

    fn form(&self, shape: &ConvexPolyhedron) -> Result<Vec<u8>, String> {
        bincode::serialize(shape).map_err(|error| error.to_string())
    }

    fn read(&self, form: &[u8]) -> Result<ConvexPolyhedron, String> {
        bincode::deserialize(form).map_err(|error| error.to_string())
    }

    fn world(&self, shape: ConvexPolyhedron, _elements: usize, bodies: usize) -> Result<PhysicsWorld, String> {
        worlds::hull_world(shape, bodies)
    }
}
