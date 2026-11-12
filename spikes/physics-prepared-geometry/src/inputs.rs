//! The large inputs the Rapier shapes are built from, each a function of its
//! size and of a seed. Every coordinate is computed with sums and single
//! products of values drawn from the reference workload's generator, with no
//! transcendental function, so that every platform builds from the same input
//! bytes; the hash of the bytes is recorded so that the cells can be compared
//! on it. Throwaway, never product code.

use rapier3d::math::Vector;
use reference_workload::{Fnv1a, SplitMix64};

const TERRAIN_SEED: u64 = 0x5465_7272_6169_6e33;
const CLOUD_SEED: u64 = 0x436c_6f75_6473_3344;

/// A value in [-0.5, 0.5), exact in `f32`.
pub fn centered(random: &mut SplitMix64) -> f32 {
    random.u01() - 0.5
}

/// A terrain grid of quads of one unit, each split into two triangles.
pub struct Terrain {
    pub vertices: Vec<Vector>,
    pub indices: Vec<[u32; 3]>,
}

/// The grid of a terrain of `triangles` triangles, as the count of quads
/// along x and along z: the grids the measurement names for its ladder, and
/// for any other count the most square grid whose quads give exactly that
/// count.
pub fn terrain_quads(triangles: usize) -> (usize, usize) {
    match triangles {
        100 => (5, 10),
        1_000 => (20, 25),
        10_000 => (50, 100),
        100_000 => (200, 250),
        other => {
            let quads = (other / 2).max(1);
            let mut along_x = 1;
            while (along_x + 1) * (along_x + 1) <= quads {
                along_x += 1;
            }
            while quads % along_x != 0 {
                along_x -= 1;
            }
            (along_x, quads / along_x)
        }
    }
}

/// The terrain of `triangles` triangles: heights within plus or minus 0.3,
/// triangles wound so that their normals point upwards.
pub fn terrain(triangles: usize) -> Terrain {
    let (along_x, along_z) = terrain_quads(triangles);
    let mut random = SplitMix64::new(TERRAIN_SEED.wrapping_add(triangles as u64));
    let mut vertices = Vec::with_capacity((along_x + 1) * (along_z + 1));
    for z in 0..=along_z {
        for x in 0..=along_x {
            let height = centered(&mut random) * 0.6;
            vertices.push(Vector::new(x as f32, height, z as f32));
        }
    }
    let vertex = |x: usize, z: usize| (z * (along_x + 1) + x) as u32;
    let mut indices = Vec::with_capacity(2 * along_x * along_z);
    for z in 0..along_z {
        for x in 0..along_x {
            indices.push([vertex(x, z), vertex(x, z + 1), vertex(x + 1, z + 1)]);
            indices.push([vertex(x, z), vertex(x + 1, z + 1), vertex(x + 1, z)]);
        }
    }
    Terrain { vertices, indices }
}

fn write_vector(hasher: &mut Fnv1a, vector: Vector) {
    for component in [vector.x, vector.y, vector.z] {
        hasher.write(&component.to_bits().to_le_bytes());
    }
}

/// The hash of a terrain's bytes: its vertices then its indices, in
/// little-endian order.
pub fn terrain_hash(terrain: &Terrain) -> u64 {
    let mut hasher = Fnv1a::new();
    for vertex in &terrain.vertices {
        write_vector(&mut hasher, *vertex);
    }
    for triangle in &terrain.indices {
        for index in triangle {
            hasher.write(&index.to_le_bytes());
        }
    }
    hasher.finish()
}

/// A cloud of `count` points: drawn in the cube that holds the unit ball,
/// kept when inside the ball, then scaled by (1.0, 0.6, 0.8).
pub fn cloud(count: usize) -> Vec<Vector> {
    let mut random = SplitMix64::new(CLOUD_SEED.wrapping_add(count as u64));
    let mut points = Vec::with_capacity(count);
    while points.len() < count {
        let x = centered(&mut random) * 2.0;
        let y = centered(&mut random) * 2.0;
        let z = centered(&mut random) * 2.0;
        if x * x + y * y + z * z <= 1.0 {
            points.push(Vector::new(x, y * 0.6, z * 0.8));
        }
    }
    points
}

/// The hash of a cloud's bytes, in little-endian order.
pub fn cloud_hash(points: &[Vector]) -> u64 {
    let mut hasher = Fnv1a::new();
    for point in points {
        write_vector(&mut hasher, *point);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_gives_the_published_vectors() {
        let mut random = SplitMix64::new(0);
        assert_eq!(random.next(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(random.next(), 0x6e78_9e6a_a1b9_65f4);
        assert_eq!(random.next(), 0x06c4_5d18_8009_454f);
    }

    #[test]
    fn the_ladder_grids_give_their_triangle_counts() {
        for triangles in [24, 100, 1_000, 10_000, 100_000] {
            let (along_x, along_z) = terrain_quads(triangles);
            assert_eq!(2 * along_x * along_z, triangles);
        }
        assert_eq!(terrain_quads(1_000), (20, 25));
        assert_eq!(terrain_quads(24), (3, 4));
    }

    #[test]
    fn a_terrain_is_a_function_of_its_size() {
        let terrain = terrain(24);
        assert_eq!(terrain.vertices.len(), 4 * 5);
        assert_eq!(terrain.indices.len(), 24);
        assert!(terrain.vertices.iter().all(|vertex| vertex.y.abs() <= 0.3));
        assert!(terrain.indices.iter().flatten().all(|index| (*index as usize) < terrain.vertices.len()));
        assert_eq!(terrain_hash(&terrain), terrain_hash(&super::terrain(24)));
        assert_ne!(terrain_hash(&terrain), terrain_hash(&super::terrain(48)));
    }

    #[test]
    fn a_terrain_faces_upwards() {
        let terrain = terrain(24);
        for [a, b, c] in &terrain.indices {
            let (a, b, c) = (terrain.vertices[*a as usize], terrain.vertices[*b as usize], terrain.vertices[*c as usize]);
            assert!((b - a).cross(c - a).y > 0.0);
        }
    }

    #[test]
    fn a_cloud_lies_inside_the_scaled_ball() {
        let points = cloud(200);
        assert_eq!(points.len(), 200);
        for point in &points {
            let (x, y, z) = (point.x, point.y / 0.6, point.z / 0.8);
            assert!(x * x + y * y + z * z <= 1.0 + 1e-5);
        }
        assert_eq!(cloud_hash(&points), cloud_hash(&cloud(200)));
        assert_ne!(cloud_hash(&points), cloud_hash(&cloud(201)));
    }
}
