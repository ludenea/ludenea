//! The worlds of the behavior measurement and their hashes: a terrain mesh
//! with bodies dropped onto it, and a ground with bodies that each carry the
//! hull. A world is a function of its shape, so that a world whose shape was
//! read from a form compares, step by step, with the world that built it.
//! Throwaway, never product code.

use rapier3d::math::Vector;
use rapier3d::parry::shape::{ConvexPolyhedron, SharedShape, TriMesh};
use rapier3d::prelude::{ColliderBuilder, PhysicsWorld, RigidBodyBuilder};
use reference_workload::{Fnv1a, SplitMix64};

use crate::inputs::centered;

const PLACEMENT_SEED: u64 = 0x506c_6163_656d_3344;
const HULL_COLUMNS: usize = 10;

/// Gravity 10 downwards, the library's default integration parameters with a
/// step of one sixtieth of a second, sleeping left on. Without the library's
/// `parallel` feature the step runs on the calling thread.
fn new_world() -> PhysicsWorld {
    let mut world = PhysicsWorld::new();
    world.gravity = Vector::new(0.0, -10.0, 0.0);
    world.integration_parameters.dt = 1.0 / 60.0;
    world
}

/// A height among five, so that the bodies land over the run and not at once.
fn staggered_height(column: usize, row: usize, random: &mut SplitMix64) -> f32 {
    2.0 + 1.5 * ((column + 2 * row) % 5) as f32 + centered(random) * 0.4
}

/// The columns and rows of a grid of at least `bodies` places whose cells are
/// about square over a terrain of the given quads.
fn grid_over(bodies: usize, quads: (usize, usize)) -> (usize, usize) {
    let (along_x, along_z) = quads;
    let mut columns = 1;
    while columns * columns * along_z < bodies * along_x {
        columns += 1;
    }
    (columns, bodies.div_ceil(columns).max(1))
}

/// A fixed body that carries the terrain mesh, and `bodies` dynamic bodies
/// (balls, cuboids and capsules in turn) placed over it on a grid, each moved
/// by the seeded generator, at staggered heights.
pub fn mesh_world(mesh: TriMesh, quads: (usize, usize), bodies: usize) -> PhysicsWorld {
    let mut world = new_world();
    world.insert(RigidBodyBuilder::fixed(), ColliderBuilder::new(SharedShape::new(mesh)));

    let mut random = SplitMix64::new(PLACEMENT_SEED);
    let (columns, rows) = grid_over(bodies, quads);
    let (width, depth) = (quads.0 as f32, quads.1 as f32);
    // The bodies stay a unit away from the edges of a terrain large enough.
    let (margin_x, margin_z) = ((width * 0.25).min(1.0), (depth * 0.25).min(1.0));
    let (step_x, step_z) = ((width - 2.0 * margin_x) / columns as f32, (depth - 2.0 * margin_z) / rows as f32);
    for index in 0..bodies {
        let (column, row) = (index % columns, index / columns);
        let x = margin_x + (column as f32 + 0.5) * step_x + centered(&mut random) * 0.4;
        let z = margin_z + (row as f32 + 0.5) * step_z + centered(&mut random) * 0.4;
        let y = staggered_height(column, row, &mut random);
        let collider = match index % 3 {
            0 => ColliderBuilder::ball(0.4),
            1 => ColliderBuilder::cuboid(0.35, 0.35, 0.35),
            _ => ColliderBuilder::capsule_y(0.25, 0.25),
        };
        world.insert(RigidBodyBuilder::dynamic().translation(Vector::new(x, y, z)), collider.density(1.0));
    }
    world
}

/// A fixed ground cuboid and `bodies` dynamic bodies that each carry the
/// hull, scaled by one half so that a body is about one unit across, two
/// units apart on a grid, at staggered heights.
pub fn hull_world(hull: ConvexPolyhedron, bodies: usize) -> Result<PhysicsWorld, String> {
    let hull = hull.scaled(Vector::splat(0.5)).ok_or("the library could not scale the hull")?;
    let shape = SharedShape::new(hull);

    let mut world = new_world();
    world.insert(RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)), ColliderBuilder::cuboid(40.0, 1.0, 40.0));

    let mut random = SplitMix64::new(PLACEMENT_SEED);
    let rows = bodies.div_ceil(HULL_COLUMNS).max(1);
    for index in 0..bodies {
        let (column, row) = (index % HULL_COLUMNS, index / HULL_COLUMNS);
        let x = 2.0 * column as f32 - (HULL_COLUMNS - 1) as f32 + centered(&mut random) * 0.4;
        let z = 2.0 * row as f32 - (rows - 1) as f32 + centered(&mut random) * 0.4;
        let y = staggered_height(column, row, &mut random);
        world.insert(
            RigidBodyBuilder::dynamic().translation(Vector::new(x, y, z)),
            ColliderBuilder::new(shape.clone()).density(1.0),
        );
    }
    Ok(world)
}

/// The hash of every body's position and rotation, the bodies sorted by
/// handle and the components hashed field by field as little-endian bytes,
/// as the Rapier snapshot continuation spike hashes them: the hash depends on
/// neither the set's iteration order nor struct padding.
pub fn hash_bodies(world: &PhysicsWorld) -> u64 {
    let mut rows: Vec<((u32, u32), [f32; 7])> = world
        .bodies
        .iter()
        .map(|(handle, body)| {
            let pose = body.position();
            let (translation, rotation) = (pose.translation, pose.rotation);
            (
                handle.into_raw_parts(),
                [translation.x, translation.y, translation.z, rotation.x, rotation.y, rotation.z, rotation.w],
            )
        })
        .collect();
    rows.sort_by_key(|(handle, _)| *handle);
    let mut hasher = Fnv1a::new();
    for (_, components) in &rows {
        for component in components {
            hasher.write(&component.to_bits().to_le_bytes());
        }
    }
    hasher.finish()
}

/// Steps the world and gives the hash of its bodies after every step.
pub fn step_hashes(world: &mut PhysicsWorld, steps: usize) -> Vec<u64> {
    (0..steps)
        .map(|_| {
            world.step();
            hash_bodies(world)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inputs;
    use rapier3d::parry::shape::TriMeshFlags;

    fn small_mesh() -> (TriMesh, (usize, usize)) {
        let terrain = inputs::terrain(48);
        let mesh = TriMesh::with_flags(terrain.vertices, terrain.indices, TriMeshFlags::empty()).expect("a mesh");
        (mesh, inputs::terrain_quads(48))
    }

    #[test]
    fn a_grid_holds_every_body() {
        for (bodies, quads) in [(150, (20, 25)), (15, (5, 10)), (6, (4, 6)), (1, (1, 1))] {
            let (columns, rows) = grid_over(bodies, quads);
            assert!(columns * rows >= bodies, "{bodies} bodies over {quads:?}");
        }
        assert_eq!(grid_over(150, (20, 25)), (11, 14));
    }

    #[test]
    fn two_worlds_built_the_same_way_step_alike() {
        let (mesh, quads) = small_mesh();
        let mut first = mesh_world(mesh.clone(), quads, 6);
        let mut second = mesh_world(mesh, quads, 6);
        assert_eq!(first.bodies.len(), 7);
        assert_eq!(step_hashes(&mut first, 30), step_hashes(&mut second, 30));
    }

    #[test]
    fn the_hash_sees_the_bodies_move_and_the_shape_change() {
        let (mesh, quads) = small_mesh();
        let mut world = mesh_world(mesh, quads, 6);
        let hashes = step_hashes(&mut world, 90);
        assert_ne!(hashes[0], hashes[1]);

        // A flat terrain of the same grid gives other hashes once the bodies land.
        let mut flat = inputs::terrain(48);
        flat.vertices.iter_mut().for_each(|vertex| vertex.y = 0.0);
        let flat = TriMesh::with_flags(flat.vertices, flat.indices, TriMeshFlags::empty()).expect("a mesh");
        assert_ne!(step_hashes(&mut mesh_world(flat, quads, 6), 90), hashes);
    }

    /// The share of the dynamic bodies that are above `floor`.
    fn share_above(world: &PhysicsWorld, floor: f32) -> f32 {
        let dynamic: Vec<f32> = world.bodies.iter().filter(|(_, body)| body.is_dynamic()).map(|(_, body)| body.position().translation.y).collect();
        dynamic.iter().filter(|height| **height > floor).count() as f32 / dynamic.len() as f32
    }

    #[test]
    fn the_terrain_of_the_measurement_catches_the_bodies() {
        // The world of the smallest ladder size: were the triangles wound
        // downwards, or the bodies placed beside the terrain, they would be
        // far below it after four seconds.
        for flags in [TriMeshFlags::empty(), TriMeshFlags::FIX_INTERNAL_EDGES] {
            let terrain = inputs::terrain(1_000);
            let mesh = TriMesh::with_flags(terrain.vertices, terrain.indices, flags).expect("a mesh");
            let mut world = mesh_world(mesh, inputs::terrain_quads(1_000), 150);
            assert_eq!(world.bodies.len(), 151);
            step_hashes(&mut world, 240);
            assert!(share_above(&world, -1.0) > 0.9, "{flags:?}: {}", share_above(&world, -1.0));
        }
    }

    #[test]
    fn the_ground_of_the_hull_world_catches_the_bodies() {
        let hull = ConvexPolyhedron::from_convex_hull(&inputs::cloud(1_000)).expect("a hull");
        let mut world = hull_world(hull, 150).expect("a world");
        step_hashes(&mut world, 240);
        assert_eq!(share_above(&world, 0.0), 1.0);
    }

    #[test]
    fn a_hull_world_holds_the_ground_and_every_body() {
        let hull = ConvexPolyhedron::from_convex_hull(&inputs::cloud(64)).expect("a hull");
        let mut world = hull_world(hull.clone(), 6).expect("a world");
        assert_eq!(world.bodies.len(), 7);
        let mut again = hull_world(hull, 6).expect("a world");
        assert_eq!(step_hashes(&mut world, 30), step_hashes(&mut again, 30));
    }
}
