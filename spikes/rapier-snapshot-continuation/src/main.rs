//! Rapier snapshot continuation: a throwaway spike of the physics research,
//! never product code.
//!
//! Runs three scenes per dimension built from Rapier's own examples, in 2D and
//! in 3D, serializes the world through serde into bytes after a number of
//! steps, and checks that a world restored from those bytes continues exactly
//! like the uninterrupted run: the same world restored in place (its state
//! replaced, its pipelines kept), a new world from the bytes on a thread pool
//! of the same size, and a new world on a pool of another size. Every body's
//! position and rotation is hashed after every step; the first divergent step
//! against the uninterrupted run is the measurement, with the image size per
//! body and the serialization times. A second mode steps every scene on the
//! global pool restricted to one thread and on handed-in pools of several
//! sizes, so that the cost of a pool against the sequential step is a figure.
//!
//! Prints one JSON document on standard output. See README.md.

use std::sync::Arc;
use std::time::Instant;

use reference_workload::{Json, SplitMix64, write_platform_and_build};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "rapier-snapshot-continuation";
const RAPIER_VERSION: &str = "0.36.1";
const STEPS_BEFORE_SNAPSHOT: usize = 100;
const STEPS_AFTER_SNAPSHOT: usize = 200;
const SNAPSHOT_POOL_SIZES: [usize; 2] = [1, 4];
const SHARING_STEPS: usize = 120;
const SHARING_POOL_SIZES: [usize; 4] = [1, 2, 4, 8];
const SCENE_SEED: u64 = 0x5261_7069_6572_3231;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

/// A thread pool handed to the library; `None` when the build has no pool.
#[cfg(feature = "parallel")]
type Pool = Option<Arc<rayon::ThreadPool>>;
#[cfg(not(feature = "parallel"))]
type Pool = Option<Arc<()>>;

fn make_pool(threads: usize) -> Pool {
    #[cfg(feature = "parallel")]
    {
        Some(Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("a thread pool"),
        ))
    }
    #[cfg(not(feature = "parallel"))]
    {
        let _ = threads;
        None
    }
}

/// What the experiment needs from a world of either dimension.
trait World: Sized {
    const DIMENSION: &'static str;
    fn step(&mut self);
    /// The hash of every body's position and rotation, sorted by handle, and
    /// the number of bodies hashed.
    fn hash(&self) -> (u64, usize);
    fn body_count(&self) -> usize;
    fn to_bytes(&self) -> Vec<u8>;
    fn from_bytes(bytes: &[u8]) -> Result<Self, String>;
    /// Replaces the world's state from the bytes while keeping its pipelines
    /// (the solver and collision workspaces), the in-place restore.
    fn restore_in_place(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn set_pool(&mut self, pool: Pool);
    fn scenes() -> Vec<Scene<Self>>;
}

struct Scene<W> {
    name: &'static str,
    build: fn() -> W,
}

/// The sort key and the transform of one body, so that the hash depends on
/// neither the set's iteration order nor struct padding.
fn hash_transforms(mut rows: Vec<((u32, u32), Vec<f32>)>) -> (u64, usize) {
    rows.sort_by_key(|(key, _)| *key);
    let mut hash = FNV_OFFSET_BASIS;
    for (_, components) in &rows {
        for component in components {
            hash = fnv1a(hash, &component.to_bits().to_le_bytes());
        }
    }
    (hash, rows.len())
}

/// A small deterministic jitter for the rain scenes, so that the columns do
/// not stack perfectly.
fn jitter(random: &mut SplitMix64, amplitude: f32) -> f32 {
    (random.u01() - 0.5) * 2.0 * amplitude
}

mod two {
    use super::{Pool, Scene, World, hash_transforms, jitter};
    use rapier2d::prelude::*;
    use reference_workload::SplitMix64;

    impl World for PhysicsWorld {
        const DIMENSION: &'static str = "2d";

        fn step(&mut self) {
            PhysicsWorld::step(self);
        }

        fn hash(&self) -> (u64, usize) {
            let rows = self
                .bodies
                .iter()
                .map(|(handle, body)| {
                    let pose = body.position();
                    (
                        handle.into_raw_parts(),
                        vec![pose.translation.x, pose.translation.y, pose.rotation.re, pose.rotation.im],
                    )
                })
                .collect();
            hash_transforms(rows)
        }

        fn body_count(&self) -> usize {
            self.bodies.len()
        }

        fn to_bytes(&self) -> Vec<u8> {
            bincode::serialize(self).expect("the world serializes")
        }

        fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
            bincode::deserialize(bytes).map_err(|error| error.to_string())
        }

        fn restore_in_place(&mut self, bytes: &[u8]) -> Result<(), String> {
            let state: PhysicsWorld = bincode::deserialize(bytes).map_err(|error| error.to_string())?;
            self.gravity = state.gravity;
            self.integration_parameters = state.integration_parameters;
            self.islands = state.islands;
            self.broad_phase = state.broad_phase;
            self.narrow_phase = state.narrow_phase;
            self.bodies = state.bodies;
            self.colliders = state.colliders;
            self.impulse_joints = state.impulse_joints;
            self.multibody_joints = state.multibody_joints;
            self.soft_bodies = state.soft_bodies;
            Ok(())
        }

        fn set_pool(&mut self, pool: Pool) {
            #[cfg(feature = "parallel")]
            self.set_thread_pool(pool);
            #[cfg(not(feature = "parallel"))]
            let _ = pool;
        }

        fn scenes() -> Vec<Scene<Self>> {
            vec![
                Scene { name: "large_pyramid", build: large_pyramid },
                Scene { name: "joint_grid", build: joint_grid },
                Scene { name: "mixed_shape_rain", build: mixed_shape_rain },
            ]
        }
    }

    fn new_world() -> PhysicsWorld {
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(0.0, -10.0);
        world
    }

    /// Rapier's port of Box2D's `large_pyramid` benchmark (`examples2d/b2d_large_pyramid.rs`),
    /// at Box2D's release base count of 100 rather than the example's 200, so that
    /// the run stays bounded on the research CI runners.
    fn large_pyramid() -> PhysicsWorld {
        let mut world = new_world();
        let base_count = 100i32;
        world.insert(
            RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0)),
            ColliderBuilder::cuboid(120.0, 1.0),
        );
        let a = 0.5f32;
        let shift = a;
        for i in 0..base_count {
            let y = (2.0 * i as f32 + 1.0) * shift;
            for j in i..base_count {
                let x = (i as f32 + 1.0) * shift + 2.0 * (j - i) as f32 * shift - a * base_count as f32;
                let body = RigidBodyBuilder::dynamic().translation(Vector::new(x, y)).can_sleep(false);
                world.insert(body, ColliderBuilder::cuboid(a, a).density(1.0));
            }
        }
        world
    }

    /// Rapier's port of Box2D's `joint_grid` benchmark (`examples2d/b2d_joint_grid.rs`),
    /// a 50 by 50 grid rather than 100 by 100 for the same reason.
    fn joint_grid() -> PhysicsWorld {
        let mut world = new_world();
        let n = 50usize;
        let groups = InteractionGroups::new(Group::GROUP_2, Group::ALL ^ Group::GROUP_2, InteractionTestMode::And);
        let mut bodies = vec![RigidBodyHandle::invalid(); n * n];
        let mut index = 0usize;
        for k in 0..n {
            for i in 0..n {
                let is_static = k >= n / 2 - 3 && k <= n / 2 + 3 && i == 0;
                let body = if is_static {
                    RigidBodyBuilder::fixed()
                } else {
                    RigidBodyBuilder::dynamic().can_sleep(false)
                }
                .translation(Vector::new(k as f32, -(i as f32)));
                let handle = world.insert_body(body);
                world.insert_collider(
                    ColliderBuilder::ball(0.4).density(1.0).collision_groups(groups),
                    Some(handle),
                );
                if i > 0 {
                    let joint = RevoluteJointBuilder::new()
                        .local_anchor1(Vector::new(0.0, -0.5))
                        .local_anchor2(Vector::new(0.0, 0.5))
                        .contacts_enabled(false);
                    world.insert_impulse_joint(bodies[index - 1], handle, joint);
                }
                if k > 0 {
                    let joint = RevoluteJointBuilder::new()
                        .local_anchor1(Vector::new(0.5, 0.0))
                        .local_anchor2(Vector::new(-0.5, 0.0))
                        .contacts_enabled(false);
                    world.insert_impulse_joint(bodies[index - n], handle, joint);
                }
                bodies[index] = handle;
                index += 1;
            }
        }
        world
    }

    /// A rain of mixed shapes (balls, boxes, capsules) placed at staggered
    /// heights so that they land over the run, on a ground box; sleeping is
    /// allowed. Derived from the shapes of Rapier's examples; the bodies exist
    /// from the start so that a world rebuilt from the bytes needs no per-step
    /// driver (Rapier's own `rain` spawns ragdolls over time).
    fn mixed_shape_rain() -> PhysicsWorld {
        let mut world = new_world();
        let mut random = SplitMix64::new(super::SCENE_SEED);
        world.insert(
            RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0)),
            ColliderBuilder::cuboid(400.0, 1.0),
        );
        let columns = 40usize;
        let rows = 30usize;
        for row in 0..rows {
            for column in 0..columns {
                let x = -100.0 + 5.0 * column as f32 + jitter(&mut random, 0.5);
                let y = 5.0 + 3.0 * row as f32 + jitter(&mut random, 0.5);
                let body = RigidBodyBuilder::dynamic().translation(Vector::new(x, y));
                let collider = match (row + column) % 3 {
                    0 => ColliderBuilder::ball(0.5),
                    1 => ColliderBuilder::cuboid(0.5, 0.5),
                    _ => ColliderBuilder::capsule_y(0.5, 0.3),
                };
                world.insert(body, collider.density(1.0));
            }
        }
        world
    }
}

mod three {
    use super::{Pool, Scene, World, hash_transforms, jitter};
    use rapier3d::prelude::*;
    use reference_workload::SplitMix64;

    impl World for PhysicsWorld {
        const DIMENSION: &'static str = "3d";

        fn step(&mut self) {
            PhysicsWorld::step(self);
        }

        fn hash(&self) -> (u64, usize) {
            let rows = self
                .bodies
                .iter()
                .map(|(handle, body)| {
                    let pose = body.position();
                    (
                        handle.into_raw_parts(),
                        vec![
                            pose.translation.x,
                            pose.translation.y,
                            pose.translation.z,
                            pose.rotation.x,
                            pose.rotation.y,
                            pose.rotation.z,
                            pose.rotation.w,
                        ],
                    )
                })
                .collect();
            hash_transforms(rows)
        }

        fn body_count(&self) -> usize {
            self.bodies.len()
        }

        fn to_bytes(&self) -> Vec<u8> {
            bincode::serialize(self).expect("the world serializes")
        }

        fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
            bincode::deserialize(bytes).map_err(|error| error.to_string())
        }

        fn restore_in_place(&mut self, bytes: &[u8]) -> Result<(), String> {
            let state: PhysicsWorld = bincode::deserialize(bytes).map_err(|error| error.to_string())?;
            self.gravity = state.gravity;
            self.integration_parameters = state.integration_parameters;
            self.islands = state.islands;
            self.broad_phase = state.broad_phase;
            self.narrow_phase = state.narrow_phase;
            self.bodies = state.bodies;
            self.colliders = state.colliders;
            self.impulse_joints = state.impulse_joints;
            self.multibody_joints = state.multibody_joints;
            self.soft_bodies = state.soft_bodies;
            Ok(())
        }

        fn set_pool(&mut self, pool: Pool) {
            #[cfg(feature = "parallel")]
            self.set_thread_pool(pool);
            #[cfg(not(feature = "parallel"))]
            let _ = pool;
        }

        fn scenes() -> Vec<Scene<Self>> {
            vec![
                Scene { name: "large_pyramid", build: large_pyramid },
                Scene { name: "joint_grid", build: joint_grid },
                Scene { name: "mixed_shape_rain", build: mixed_shape_rain },
            ]
        }
    }

    fn new_world() -> PhysicsWorld {
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(0.0, -10.0, 0.0);
        world
    }

    /// Rapier's port of Box3D's `large_pyramid` benchmark (`examples3d/b3d_large_pyramid.rs`),
    /// with a base of 60 cubes rather than 200, so that the run stays bounded on
    /// the research CI runners.
    fn large_pyramid() -> PhysicsWorld {
        let mut world = new_world();
        let base_count = 60i32;
        world.insert(
            RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)),
            ColliderBuilder::cuboid(400.0, 1.0, 400.0),
        );
        let h = 0.5f32;
        let shift = 1.0 * h;
        for i in 0..base_count {
            let y = (2.0 * i as f32 + 1.0) * shift;
            for j in i..base_count {
                let x = (i as f32 + 1.0) * shift + 2.0 * (j - i) as f32 * shift - h * base_count as f32;
                let body = RigidBodyBuilder::dynamic().translation(Vector::new(x, y, 0.0)).can_sleep(false);
                world.insert(body, ColliderBuilder::cuboid(h, h, h).density(100.0));
            }
        }
        world
    }

    /// Rapier's port of Box3D's `joint_grid` benchmark (`examples3d/b3d_joint_grid.rs`),
    /// a 50 by 50 grid rather than 100 by 100 for the same reason.
    fn joint_grid() -> PhysicsWorld {
        let mut world = new_world();
        let n = 50usize;
        let mut bodies = vec![RigidBodyHandle::invalid(); n * n];
        let mut index = 0usize;
        for k in 0..n {
            for i in 0..n {
                let body = if i == 0 {
                    RigidBodyBuilder::fixed()
                } else {
                    RigidBodyBuilder::dynamic().can_sleep(false)
                }
                .translation(Vector::new(k as f32, -(i as f32), 0.0));
                let handle = world.insert_body(body);
                world.insert_collider(ColliderBuilder::ball(0.4), Some(handle));
                if i > 0 {
                    let joint = SphericalJointBuilder::new()
                        .local_anchor1(Vector::new(0.0, -0.5, 0.0))
                        .local_anchor2(Vector::new(0.0, 0.5, 0.0));
                    world.insert_impulse_joint(bodies[index - 1], handle, joint);
                }
                if k > 0 {
                    let joint = SphericalJointBuilder::new()
                        .local_anchor1(Vector::new(0.5, 0.0, 0.0))
                        .local_anchor2(Vector::new(-0.5, 0.0, 0.0));
                    world.insert_impulse_joint(bodies[index - n], handle, joint);
                }
                bodies[index] = handle;
                index += 1;
            }
        }
        world
    }

    /// The 3D counterpart of the 2D mixed shape rain: a grid of columns of
    /// balls, boxes and capsules at staggered heights over a ground box.
    fn mixed_shape_rain() -> PhysicsWorld {
        let mut world = new_world();
        let mut random = SplitMix64::new(super::SCENE_SEED);
        world.insert(
            RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)),
            ColliderBuilder::cuboid(200.0, 1.0, 200.0),
        );
        let side = 12usize;
        let rows = 10usize;
        for row in 0..rows {
            for ix in 0..side {
                for iz in 0..side {
                    let x = -33.0 + 6.0 * ix as f32 + jitter(&mut random, 0.5);
                    let z = -33.0 + 6.0 * iz as f32 + jitter(&mut random, 0.5);
                    let y = 5.0 + 3.0 * row as f32 + jitter(&mut random, 0.5);
                    let body = RigidBodyBuilder::dynamic().translation(Vector::new(x, y, z));
                    let collider = match (row + ix + iz) % 3 {
                        0 => ColliderBuilder::ball(0.5),
                        1 => ColliderBuilder::cuboid(0.5, 0.5, 0.5),
                        _ => ColliderBuilder::capsule_y(0.5, 0.3),
                    };
                    world.insert(body, collider.density(1.0));
                }
            }
        }
        world
    }
}

// --- The snapshot continuation experiment ---------------------------------------

struct PathReport {
    path: &'static str,
    pool_threads: usize,
    restore_succeeded: bool,
    hash_at_restore_matches: bool,
    first_divergent_step: i64,
    final_hash: u64,
    hashed_body_count: usize,
    restore_ms: f64,
}

struct SceneReport {
    dimension: &'static str,
    scene: &'static str,
    pool_threads: usize,
    body_count: usize,
    hashed_body_count: usize,
    steps_before_snapshot: usize,
    steps_after_snapshot: usize,
    runs_identical_before_snapshot: bool,
    hash_at_snapshot: u64,
    uninterrupted_final_hash: u64,
    uninterrupted_step_ms_total: f64,
    snapshot_bytes: usize,
    bytes_per_body: f64,
    serialize_ms: f64,
    second_world_bytes_identical: bool,
    paths: Vec<PathReport>,
}

/// Steps the world `steps` times, hashing after every step; returns the hashed
/// body count of the last hash and the wall time of the steps alone.
fn step_and_hash<W: World>(world: &mut W, steps: usize, hashes: &mut Vec<u64>) -> (usize, f64) {
    let mut hashed = 0;
    let mut total_ms = 0.0;
    for _ in 0..steps {
        let start = Instant::now();
        world.step();
        total_ms += start.elapsed().as_secs_f64() * 1_000.0;
        let (hash, count) = world.hash();
        hashes.push(hash);
        hashed = count;
    }
    (hashed, total_ms)
}

fn first_divergent_step(reference: &[u64], run: &[u64]) -> i64 {
    reference
        .iter()
        .zip(run)
        .position(|(a, b)| a != b)
        .map_or(-1, |step| step as i64)
}

fn continue_and_compare<W: World>(
    world: Option<&mut W>,
    path: &'static str,
    pool_threads: usize,
    restore_error: Option<String>,
    restore_ms: f64,
    hash_at_snapshot: u64,
    reference: &[u64],
    steps_after: usize,
) -> PathReport {
    let mut report = PathReport {
        path,
        pool_threads,
        restore_succeeded: restore_error.is_none(),
        hash_at_restore_matches: false,
        first_divergent_step: -1,
        final_hash: 0,
        hashed_body_count: 0,
        restore_ms,
    };
    if let Some(error) = restore_error {
        eprintln!("{path}: restore failed: {error}");
        return report;
    }
    let world = world.expect("a world when the restore succeeded");
    report.hash_at_restore_matches = world.hash().0 == hash_at_snapshot;
    let mut hashes = Vec::with_capacity(steps_after);
    let (hashed, _) = step_and_hash(world, steps_after, &mut hashes);
    report.first_divergent_step = first_divergent_step(reference, &hashes);
    report.final_hash = *hashes.last().unwrap_or(&0);
    report.hashed_body_count = hashed;
    report
}

fn run_scene<W: World>(scene: &Scene<W>, pool_threads: usize, quick: bool) -> SceneReport {
    let steps_before = if quick { STEPS_BEFORE_SNAPSHOT / 10 } else { STEPS_BEFORE_SNAPSHOT };
    let steps_after = if quick { STEPS_AFTER_SNAPSHOT / 10 } else { STEPS_AFTER_SNAPSHOT };

    // The uninterrupted run: the reference for every path.
    let mut uninterrupted = (scene.build)();
    uninterrupted.set_pool(make_pool(pool_threads));
    let mut before = Vec::with_capacity(steps_before);
    let mut reference = Vec::with_capacity(steps_after);
    let (_, before_ms) = step_and_hash(&mut uninterrupted, steps_before, &mut before);
    let (hashed_body_count, after_ms) = step_and_hash(&mut uninterrupted, steps_after, &mut reference);
    let body_count = uninterrupted.body_count();
    drop(uninterrupted);

    // The snapshotted run: the same steps up to the snapshot, then the paths.
    let mut world = (scene.build)();
    world.set_pool(make_pool(pool_threads));
    let mut hashes = Vec::with_capacity(steps_before);
    step_and_hash(&mut world, steps_before, &mut hashes);
    let runs_identical_before_snapshot = first_divergent_step(&before, &hashes) == -1;
    let hash_at_snapshot = *hashes.last().expect("at least one step before the snapshot");
    let start = Instant::now();
    let bytes = world.to_bytes();
    let serialize_ms = start.elapsed().as_secs_f64() * 1_000.0;

    // A third world serialized after the same steps: the bytes must be the same.
    let mut other = (scene.build)();
    other.set_pool(make_pool(pool_threads));
    let mut other_hashes = Vec::with_capacity(steps_before);
    step_and_hash(&mut other, steps_before, &mut other_hashes);
    let second_world_bytes_identical = other.to_bytes() == bytes;
    drop(other);

    let mut paths = Vec::with_capacity(4);

    // Path 1: the world keeps stepping after the serialization.
    paths.push(continue_and_compare(
        Some(&mut world),
        "continued_after_snapshot",
        pool_threads,
        None,
        0.0,
        hash_at_snapshot,
        &reference,
        steps_after,
    ));

    // Path 2: the same world restored in place (state replaced, pipelines kept).
    let start = Instant::now();
    let restored = world.restore_in_place(&bytes);
    let restore_ms = start.elapsed().as_secs_f64() * 1_000.0;
    paths.push(continue_and_compare(
        Some(&mut world),
        "restored_in_place",
        pool_threads,
        restored.err(),
        restore_ms,
        hash_at_snapshot,
        &reference,
        steps_after,
    ));
    drop(world);

    // Path 3: a new world from the bytes on a pool of the same size.
    let start = Instant::now();
    let rebuilt = W::from_bytes(&bytes);
    let restore_ms = start.elapsed().as_secs_f64() * 1_000.0;
    let (mut rebuilt, error) = match rebuilt {
        Ok(mut rebuilt) => {
            rebuilt.set_pool(make_pool(pool_threads));
            (Some(rebuilt), None)
        }
        Err(error) => (None, Some(error)),
    };
    paths.push(continue_and_compare(
        rebuilt.as_mut(),
        "new_world_same_pool",
        pool_threads,
        error,
        restore_ms,
        hash_at_snapshot,
        &reference,
        steps_after,
    ));
    drop(rebuilt);

    // Path 4: a new world from the bytes on a pool of the other size.
    let other_threads = if pool_threads == 1 { 4 } else { 1 };
    let start = Instant::now();
    let rebuilt = W::from_bytes(&bytes);
    let restore_ms = start.elapsed().as_secs_f64() * 1_000.0;
    let (mut rebuilt, error) = match rebuilt {
        Ok(mut rebuilt) => {
            rebuilt.set_pool(make_pool(other_threads));
            (Some(rebuilt), None)
        }
        Err(error) => (None, Some(error)),
    };
    paths.push(continue_and_compare(
        rebuilt.as_mut(),
        "new_world_other_pool",
        other_threads,
        error,
        restore_ms,
        hash_at_snapshot,
        &reference,
        steps_after,
    ));

    SceneReport {
        dimension: W::DIMENSION,
        scene: scene.name,
        pool_threads,
        body_count,
        hashed_body_count,
        steps_before_snapshot: steps_before,
        steps_after_snapshot: steps_after,
        runs_identical_before_snapshot,
        hash_at_snapshot,
        uninterrupted_final_hash: *reference.last().expect("at least one step after the snapshot"),
        uninterrupted_step_ms_total: before_ms + after_ms,
        snapshot_bytes: bytes.len(),
        bytes_per_body: if body_count == 0 { 0.0 } else { bytes.len() as f64 / body_count as f64 },
        serialize_ms,
        second_world_bytes_identical,
        paths,
    }
}

// --- The pool sharing mode -------------------------------------------------------

struct SharingReport {
    dimension: &'static str,
    scene: &'static str,
    mode: &'static str,
    threads: usize,
    steps: usize,
    final_hash: u64,
    matches_baseline: bool,
    step_ms_total: f64,
    step_ms_median: f64,
    step_ms_max: f64,
}

fn step_timed<W: World>(world: &mut W, steps: usize) -> (u64, f64, f64, f64) {
    let mut times = Vec::with_capacity(steps);
    let mut hash = 0;
    for _ in 0..steps {
        let start = Instant::now();
        world.step();
        times.push(start.elapsed().as_secs_f64() * 1_000.0);
        hash = world.hash().0;
    }
    let total: f64 = times.iter().sum();
    let max = times.iter().copied().fold(0.0, f64::max);
    times.sort_by(|a, b| a.partial_cmp(b).expect("finite step times"));
    (hash, total, times[steps / 2], max)
}

fn baseline_mode() -> &'static str {
    if cfg!(feature = "parallel") { "global_pool_one_thread" } else { "sequential_build" }
}

fn sharing_threads() -> Vec<usize> {
    let mut threads = SHARING_POOL_SIZES.to_vec();
    let available = std::thread::available_parallelism().map_or(1, |n| n.get());
    if available > *threads.last().expect("a pool size") {
        threads.push(available);
    }
    threads
}

fn run_sharing<W: World>(scene: &Scene<W>, quick: bool) -> Vec<SharingReport> {
    let steps = if quick { SHARING_STEPS / 10 } else { SHARING_STEPS };
    let mut reports = Vec::new();

    let mut baseline = (scene.build)();
    let (baseline_hash, total, median, max) = step_timed(&mut baseline, steps);
    drop(baseline);
    reports.push(SharingReport {
        dimension: W::DIMENSION,
        scene: scene.name,
        mode: baseline_mode(),
        threads: 1,
        steps,
        final_hash: baseline_hash,
        matches_baseline: true,
        step_ms_total: total,
        step_ms_median: median,
        step_ms_max: max,
    });

    if cfg!(feature = "parallel") {
        for threads in sharing_threads() {
            let mut world = (scene.build)();
            world.set_pool(make_pool(threads));
            let (hash, total, median, max) = step_timed(&mut world, steps);
            reports.push(SharingReport {
                dimension: W::DIMENSION,
                scene: scene.name,
                mode: "handed_in_pool",
                threads,
                steps,
                final_hash: hash,
                matches_baseline: hash == baseline_hash,
                step_ms_total: total,
                step_ms_median: median,
                step_ms_max: max,
            });
        }
    }
    reports
}

// --- Report ----------------------------------------------------------------------

fn measure<W: World>(quick: bool, scenes: &mut Vec<SceneReport>, sharing: &mut Vec<SharingReport>) {
    for scene in W::scenes() {
        for pool_threads in SNAPSHOT_POOL_SIZES {
            eprint!("{} {} at {} threads ...", W::DIMENSION, scene.name, pool_threads);
            let report = run_scene(&scene, pool_threads, quick);
            eprint!(" {} bytes", report.snapshot_bytes);
            for path in &report.paths {
                eprint!(", {} diverges at {}", path.path, path.first_divergent_step);
            }
            eprintln!();
            scenes.push(report);
        }
        eprintln!("{} {} pool sharing ...", W::DIMENSION, scene.name);
        sharing.extend(run_sharing(&scene, quick));
    }
}

fn write_report(cell: &str, quick: bool, scenes: &[SceneReport], sharing: &[SharingReport]) -> String {
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    out.bool("quick", quick);
    out.key("rapier");
    out.object_start();
    out.string("version", RAPIER_VERSION);
    out.string("license", "Apache-2.0");
    out.string("crates", "rapier2d, rapier3d");
    out.string(
        "features",
        if cfg!(feature = "parallel") {
            "enhanced-determinism, parallel, serde-serialize"
        } else {
            "enhanced-determinism, serde-serialize"
        },
    );
    out.bool("parallel_feature", cfg!(feature = "parallel"));
    out.object_end();
    out.key("serializer");
    out.object_start();
    out.string("name", "bincode");
    out.string("version", "1.3.3");
    out.string("license", "MIT");
    out.object_end();
    out.key("settings");
    out.object_start();
    out.decimal("time_step", 1.0 / 60.0);
    out.u64("solver_iterations", 4);
    out.u64("steps_before_snapshot", STEPS_BEFORE_SNAPSHOT as u64);
    out.u64("steps_after_snapshot", STEPS_AFTER_SNAPSHOT as u64);
    out.key("snapshot_pool_threads");
    out.array_start();
    for threads in SNAPSHOT_POOL_SIZES {
        out.object_start();
        out.u64("threads", threads as u64);
        out.object_end();
    }
    out.array_end();
    out.u64("sharing_steps", SHARING_STEPS as u64);
    out.key("sharing_pool_threads");
    out.array_start();
    for threads in sharing_threads() {
        out.object_start();
        out.u64("threads", threads as u64);
        out.object_end();
    }
    out.array_end();
    out.string("baseline_mode", baseline_mode());
    out.object_end();
    write_platform_and_build(&mut out);
    out.key("runs");
    out.array_start();
    for scene in scenes {
        out.object_start();
        out.string("dimension", scene.dimension);
        out.string("scene", scene.scene);
        out.u64("pool_threads", scene.pool_threads as u64);
        out.u64("body_count", scene.body_count as u64);
        out.u64("hashed_body_count", scene.hashed_body_count as u64);
        out.u64("steps_before_snapshot", scene.steps_before_snapshot as u64);
        out.u64("steps_after_snapshot", scene.steps_after_snapshot as u64);
        out.bool("runs_identical_before_snapshot", scene.runs_identical_before_snapshot);
        out.hex("hash_at_snapshot", scene.hash_at_snapshot);
        out.hex("uninterrupted_final_hash", scene.uninterrupted_final_hash);
        out.decimal("uninterrupted_step_ms_total", scene.uninterrupted_step_ms_total);
        out.u64("snapshot_bytes", scene.snapshot_bytes as u64);
        out.decimal("bytes_per_body", scene.bytes_per_body);
        out.decimal("serialize_ms", scene.serialize_ms);
        out.bool("second_world_bytes_identical", scene.second_world_bytes_identical);
        out.key("paths");
        out.array_start();
        for path in &scene.paths {
            out.object_start();
            out.string("path", path.path);
            out.u64("pool_threads", path.pool_threads as u64);
            out.bool("restore_succeeded", path.restore_succeeded);
            out.bool("hash_at_restore_matches", path.hash_at_restore_matches);
            out.i64("first_divergent_step", path.first_divergent_step);
            out.hex("final_hash", path.final_hash);
            out.u64("hashed_body_count", path.hashed_body_count as u64);
            out.decimal("restore_ms", path.restore_ms);
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();
    out.key("pool_sharing");
    out.array_start();
    for run in sharing {
        out.object_start();
        out.string("dimension", run.dimension);
        out.string("scene", run.scene);
        out.string("mode", run.mode);
        out.u64("threads", run.threads as u64);
        out.u64("steps", run.steps as u64);
        out.hex("final_hash", run.final_hash);
        out.bool("matches_baseline", run.matches_baseline);
        out.decimal("step_ms_total", run.step_ms_total);
        out.decimal("step_ms_median", run.step_ms_median);
        out.decimal("step_ms_max", run.step_ms_max);
        out.object_end();
    }
    out.array_end();
    out.object_end();
    out.finish()
}

/// The global pool is restricted to one thread, so that a world without a
/// handed-in pool measures the library on the calling thread alone.
fn restrict_global_pool() {
    #[cfg(feature = "parallel")]
    {
        let _ = rayon::ThreadPoolBuilder::new().num_threads(1).build_global();
    }
}

fn main() {
    let mut cell = String::from("unspecified");
    let mut quick = false;
    for argument in std::env::args().skip(1) {
        if argument == "--quick" {
            quick = true;
        } else {
            cell = argument;
        }
    }
    restrict_global_pool();

    let mut scenes = Vec::new();
    let mut sharing = Vec::new();
    measure::<rapier2d::prelude::PhysicsWorld>(quick, &mut scenes, &mut sharing);
    measure::<rapier3d::prelude::PhysicsWorld>(quick, &mut scenes, &mut sharing);
    println!("{}", write_report(&cell, quick, &scenes, &sharing));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_reference_vectors() {
        assert_eq!(fnv1a(FNV_OFFSET_BASIS, b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(FNV_OFFSET_BASIS, b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn hash_is_independent_of_row_order() {
        let a = vec![((0, 0), vec![1.0]), ((1, 0), vec![2.0])];
        let b = vec![((1, 0), vec![2.0]), ((0, 0), vec![1.0])];
        assert_eq!(hash_transforms(a), hash_transforms(b));
        assert_ne!(hash_transforms(vec![((0, 0), vec![1.0])]), hash_transforms(vec![((0, 0), vec![1.5])]));
    }

    fn two_runs_agree_and_bytes_round_trip<W: World>() {
        restrict_global_pool();
        for scene in W::scenes() {
            let mut a = (scene.build)();
            let mut b = (scene.build)();
            let mut hashes_a = Vec::new();
            let mut hashes_b = Vec::new();
            let (hashed, _) = step_and_hash(&mut a, 5, &mut hashes_a);
            step_and_hash(&mut b, 5, &mut hashes_b);
            assert!(hashed > 0, "{} {}: no body hashed", W::DIMENSION, scene.name);
            assert_eq!(hashed, a.body_count(), "{} {}: a body was missed", W::DIMENSION, scene.name);
            assert_eq!(first_divergent_step(&hashes_a, &hashes_b), -1, "{} {}: two runs differ", W::DIMENSION, scene.name);

            // The bytes give back the state they were taken from, in place and as a new world.
            let bytes = a.to_bytes();
            let at_snapshot = a.hash().0;
            step_and_hash(&mut a, 2, &mut hashes_a);
            a.restore_in_place(&bytes).expect("the in-place restore succeeds");
            assert_eq!(a.hash().0, at_snapshot, "{} {}: the in-place restore changed the state", W::DIMENSION, scene.name);
            let rebuilt = W::from_bytes(&bytes).expect("the new world deserializes");
            assert_eq!(rebuilt.hash().0, at_snapshot, "{} {}: the rebuilt world differs", W::DIMENSION, scene.name);
        }
    }

    #[test]
    fn two_dimensional_runs_agree_and_round_trip() {
        two_runs_agree_and_bytes_round_trip::<rapier2d::prelude::PhysicsWorld>();
    }

    #[test]
    fn three_dimensional_runs_agree_and_round_trip() {
        two_runs_agree_and_bytes_round_trip::<rapier3d::prelude::PhysicsWorld>();
    }

    #[test]
    fn quick_report_is_json_with_every_path() {
        restrict_global_pool();
        let scene = &rapier2d::prelude::PhysicsWorld::scenes()[2];
        let report = run_scene(scene, 1, true);
        assert_eq!(report.paths.len(), 4);
        assert!(report.runs_identical_before_snapshot);
        let json = write_report("test", true, &[report], &[]);
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"new_world_other_pool\""));
    }
}
