//! The world the Rapier half steps: entities with a definition, the
//! components read back from the library after a step, the three parts a take
//! writes (the library's image, the tables that map an entity to its library
//! handle, the canonical bytes of the components), the ring that keeps them,
//! the restore in place and the write-back.

use std::sync::Arc;
use std::time::Instant;

use rapier3d::prelude::*;

pub const FIXED_STEP_MS: f64 = 1_000.0 / 60.0;
pub const IMPULSES_PER_TICK: usize = 4;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

/// The seeded stream (SplitMix64, as the reference workload's), with the
/// signed float the Box2D half draws from it.
pub struct Random {
    state: u64,
}

impl Random {
    pub fn new(seed: u64) -> Random {
        Random { state: seed }
    }

    pub fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A float in [-1, 1) from 24 bits of the stream: exact on every platform.
    pub fn signed(&mut self) -> f32 {
        let bits = (self.next() >> 40) as u32;
        bits as f32 * (1.0 / 8_388_608.0) - 1.0
    }
}

pub const KIND_BALL: u32 = 0;
pub const KIND_CUBOID: u32 = 1;
pub const KIND_CAPSULE: u32 = 2;
/// A body of a scene taken from the library's examples: the definition holds
/// what the library says of it (type, mass).
pub const KIND_FOREIGN: u32 = 3;

pub const BODY_DYNAMIC: u32 = 0;
pub const BODY_FIXED: u32 = 1;
pub const BODY_OTHER: u32 = 2;

/// What the engine would own as the definition of a body, written field by
/// field into the canonical bytes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Definition {
    pub kind: u32,
    pub body_type: u32,
    pub collides: u32,
    /// Ball radius, cuboid half extent along x, capsule half length, or mass.
    pub a: f32,
    /// Cuboid half extent along y and z, or capsule radius.
    pub b: f32,
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
}

pub const DEFINITION_BYTES: usize = 32;
pub const CANONICAL_ROW_BYTES: usize = 8 + DEFINITION_BYTES + 28 + 24 + 1;
pub const TABLE_ROW_BYTES: usize = 24;

impl Definition {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.extend_from_slice(&self.body_type.to_le_bytes());
        out.extend_from_slice(&self.collides.to_le_bytes());
        for field in [self.a, self.b, self.density, self.friction, self.restitution] {
            out.extend_from_slice(&field.to_bits().to_le_bytes());
        }
    }

    fn read(cursor: &mut Cursor) -> Definition {
        Definition {
            kind: cursor.u32(),
            body_type: cursor.u32(),
            collides: cursor.u32(),
            a: cursor.f32(),
            b: cursor.f32(),
            density: cursor.f32(),
            friction: cursor.f32(),
            restitution: cursor.f32(),
        }
    }

    pub fn digest(&self) -> u64 {
        let mut bytes = Vec::with_capacity(DEFINITION_BYTES);
        self.write(&mut bytes);
        fnv1a(FNV_OFFSET_BASIS, &bytes)
    }

    /// The largest extent of the body's bounding box at rest.
    pub fn size(&self) -> f32 {
        match self.kind {
            KIND_BALL => 2.0 * self.a,
            KIND_CUBOID => 2.0 * self.a.max(self.b),
            KIND_CAPSULE => 2.0 * (self.a + self.b),
            _ => 1.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Components {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub linear: [f32; 3],
    pub angular: [f32; 3],
    pub awake: bool,
}

impl Components {
    pub fn resting_at(x: f32, y: f32, z: f32) -> Components {
        Components {
            position: [x, y, z],
            rotation: [0.0, 0.0, 0.0, 1.0],
            linear: [0.0; 3],
            angular: [0.0; 3],
            awake: true,
        }
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> u32 {
        let value = u32::from_le_bytes(self.bytes[self.at..self.at + 4].try_into().expect("four bytes"));
        self.at += 4;
        value
    }

    fn u64(&mut self) -> u64 {
        let value = u64::from_le_bytes(self.bytes[self.at..self.at + 8].try_into().expect("eight bytes"));
        self.at += 8;
        value
    }

    fn f32(&mut self) -> f32 {
        f32::from_bits(self.u32())
    }

    fn u8(&mut self) -> u8 {
        let value = self.bytes[self.at];
        self.at += 1;
        value
    }
}

pub struct Entity {
    pub id: u64,
    pub handle: RigidBodyHandle,
    pub digest: u64,
    pub definition: Definition,
}

fn handle_bits(handle: RigidBodyHandle) -> u64 {
    let (index, generation) = handle.into_raw_parts();
    u64::from(index) | (u64::from(generation) << 32)
}

fn handle_from_bits(bits: u64) -> RigidBodyHandle {
    RigidBodyHandle::from_raw_parts(bits as u32, (bits >> 32) as u32)
}

pub type Pool = Arc<rayon::ThreadPool>;

pub fn make_pool(threads: usize) -> Pool {
    Arc::new(rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("a thread pool"))
}

pub struct Sim {
    pub world: PhysicsWorld,
    pub workers: usize,
    pub sleep: bool,
    pub entities: Vec<Entity>,
    pub components: Vec<Components>,
    next_id: u64,
    /// The indexes of the entities an input may push (the dynamic ones).
    pushable: Vec<usize>,
    input_seed: u64,
    pub impulse_speed: f32,
    /// Where the image is written before it is copied into a ring slot of
    /// exactly its size, as the other library does inside its own call.
    scratch: Vec<u8>,
}

impl Sim {
    pub fn new(gravity: Vector, pool: &Pool, sleep: bool, input_seed: u64, impulse_speed: f32) -> Sim {
        let mut world = PhysicsWorld::new();
        world.gravity = gravity;
        world.set_thread_pool(Some(pool.clone()));
        Sim {
            world,
            workers: pool.current_num_threads(),
            sleep,
            entities: Vec::new(),
            components: Vec::new(),
            next_id: 0,
            pushable: Vec::new(),
            input_seed,
            impulse_speed,
            scratch: Vec::new(),
        }
    }

    pub fn count(&self) -> usize {
        self.entities.len()
    }

    /// Creates the library's body for a definition and a state, as a world
    /// created from components does.
    fn create_library_body(&mut self, d: &Definition, c: &Components) -> RigidBodyHandle {
        let pose = Pose::from_parts(
            Vector::new(c.position[0], c.position[1], c.position[2]),
            Rotation::from_xyzw(c.rotation[0], c.rotation[1], c.rotation[2], c.rotation[3]),
        );
        let builder = if d.body_type == BODY_DYNAMIC { RigidBodyBuilder::dynamic() } else { RigidBodyBuilder::fixed() };
        let body = builder
            .pose(pose)
            .linvel(Vector::new(c.linear[0], c.linear[1], c.linear[2]))
            .angvel(Vector::new(c.angular[0], c.angular[1], c.angular[2]))
            .can_sleep(self.sleep)
            .sleeping(!c.awake);
        let shape = match d.kind {
            KIND_BALL => ColliderBuilder::ball(d.a),
            KIND_CUBOID => ColliderBuilder::cuboid(d.a, d.b, d.b),
            _ => ColliderBuilder::capsule_x(d.a, d.b),
        };
        let mut collider = shape.density(d.density).friction(d.friction).restitution(d.restitution);
        if d.collides == 0 {
            collider = collider.collision_groups(InteractionGroups::new(
                Group::GROUP_1,
                Group::NONE,
                InteractionTestMode::And,
            ));
        }
        self.world.insert(body, collider).0
    }

    /// Adds an entity that wraps an existing library body.
    pub fn adopt(&mut self, handle: RigidBodyHandle, definition: Definition) -> usize {
        self.entities.push(Entity { id: self.next_id, handle, digest: definition.digest(), definition });
        self.next_id += 1;
        self.components.push(Components::resting_at(0.0, 0.0, 0.0));
        self.entities.len() - 1
    }

    pub fn spawn(&mut self, definition: Definition, components: Components) -> usize {
        let handle = self.create_library_body(&definition, &components);
        let index = self.adopt(handle, definition);
        self.components[index] = components;
        index
    }

    /// Removes the entity at an index, keeping the entities in identifier order.
    pub fn despawn(&mut self, index: usize) {
        let entity = self.entities.remove(index);
        self.components.remove(index);
        self.world.remove_body(entity.handle);
    }

    pub fn refresh_pushable(&mut self) {
        self.pushable.clear();
        for (index, entity) in self.entities.iter().enumerate() {
            if entity.definition.body_type == BODY_DYNAMIC {
                self.pushable.push(index);
            }
        }
    }

    /// The inputs of one tick: impulses on a few bodies, a pure function of
    /// the seed and the tick. The mispredicted form pushes the first body the
    /// other way and three times as hard, which is what a correction amends.
    pub fn apply_inputs(&mut self, tick: usize, mispredicted: bool) {
        if self.pushable.is_empty() || self.impulse_speed == 0.0 {
            return;
        }
        let mut random = Random::new(self.input_seed ^ (tick as u64).wrapping_mul(0xd6e8_feb8_6659_fd93));
        for i in 0..IMPULSES_PER_TICK {
            let index = self.pushable[(random.next() % self.pushable.len() as u64) as usize];
            let mut push = Vector::new(random.signed(), random.signed(), random.signed()) * self.impulse_speed;
            if mispredicted && i == 0 {
                push *= -3.0;
            }
            let body = &mut self.world.bodies[self.entities[index].handle];
            let mass = body.mass();
            body.apply_impulse(push * mass, true);
        }
    }

    pub fn step(&mut self) {
        self.world.step();
    }

    /// Reads the components back from the library, as the engine's tick does
    /// after every step whether or not anything is taken.
    pub fn sync(&mut self) {
        for (entity, c) in self.entities.iter().zip(self.components.iter_mut()) {
            let body = &self.world.bodies[entity.handle];
            let pose = body.position();
            let linear = body.linvel();
            let angular = body.angvel();
            c.position = [pose.translation.x, pose.translation.y, pose.translation.z];
            c.rotation = [pose.rotation.x, pose.rotation.y, pose.rotation.z, pose.rotation.w];
            c.linear = [linear.x, linear.y, linear.z];
            c.angular = [angular.x, angular.y, angular.z];
            c.awake = !body.is_sleeping();
        }
    }

    pub fn tick(&mut self, tick: usize, mispredicted: bool) {
        self.apply_inputs(tick, mispredicted);
        self.step();
        self.sync();
    }

    /// The hash of the transforms and the motion, in entity order.
    pub fn hash(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        for c in &self.components {
            for field in c.position.iter().chain(&c.rotation).chain(&c.linear).chain(&c.angular) {
                hash = fnv1a(hash, &field.to_bits().to_le_bytes());
            }
        }
        hash
    }

    pub fn awake_dynamic_bodies(&self) -> usize {
        self.entities
            .iter()
            .filter(|entity| entity.definition.body_type == BODY_DYNAMIC && !self.world.bodies[entity.handle].is_sleeping())
            .count()
    }

    fn take_image(&mut self, image: &mut Vec<u8>) {
        self.scratch.clear();
        bincode::serialize_into(&mut self.scratch, &self.world).expect("the world serializes");
        fit(image, self.scratch.len());
        image.extend_from_slice(&self.scratch);
    }

    fn take_tables(&self, tables: &mut Vec<u8>) {
        fit(tables, 8 + self.count() * TABLE_ROW_BYTES);
        tables.extend_from_slice(&(self.count() as u64).to_le_bytes());
        for entity in &self.entities {
            tables.extend_from_slice(&entity.id.to_le_bytes());
            tables.extend_from_slice(&handle_bits(entity.handle).to_le_bytes());
            tables.extend_from_slice(&entity.digest.to_le_bytes());
        }
    }

    fn take_canonical(&self, canonical: &mut Vec<u8>) {
        fit(canonical, 8 + self.count() * CANONICAL_ROW_BYTES);
        canonical.extend_from_slice(&(self.count() as u64).to_le_bytes());
        for (entity, c) in self.entities.iter().zip(&self.components) {
            canonical.extend_from_slice(&entity.id.to_le_bytes());
            entity.definition.write(canonical);
            for field in c.position.iter().chain(&c.rotation).chain(&c.linear).chain(&c.angular) {
                canonical.extend_from_slice(&field.to_bits().to_le_bytes());
            }
            canonical.push(u8::from(c.awake));
        }
    }

    pub fn take(&mut self, take: &mut Take, tick: usize) -> TakeTimes {
        let start = Instant::now();
        self.take_image(&mut take.image);
        let image_ms = milliseconds(start);
        let start = Instant::now();
        self.take_tables(&mut take.tables);
        let tables_ms = milliseconds(start);
        let start = Instant::now();
        self.take_canonical(&mut take.canonical);
        let canonical_ms = milliseconds(start);
        take.tick = tick;
        take.held = true;
        TakeTimes { image_ms, tables_ms, canonical_ms }
    }

    /// Reads the tables and the canonical bytes back into the entities and
    /// the components. The entity set of a world under correction does not
    /// change, so the rows are checked against the entities in place.
    fn restore_tables_and_components(&mut self, take: &Take) {
        let mut cursor = Cursor { bytes: &take.tables, at: 0 };
        assert_eq!(cursor.u64(), self.count() as u64, "a restored table holds another entity count");
        for entity in &mut self.entities {
            entity.id = cursor.u64();
            entity.handle = handle_from_bits(cursor.u64());
            entity.digest = cursor.u64();
        }
        let mut cursor = Cursor { bytes: &take.canonical, at: 0 };
        assert_eq!(cursor.u64(), self.count() as u64, "restored components hold another entity count");
        for (entity, c) in self.entities.iter_mut().zip(self.components.iter_mut()) {
            assert_eq!(cursor.u64(), entity.id, "restored components are not in entity order");
            entity.definition = Definition::read(&mut cursor);
            for field in c
                .position
                .iter_mut()
                .chain(&mut c.rotation)
                .chain(&mut c.linear)
                .chain(&mut c.angular)
            {
                *field = cursor.f32();
            }
            c.awake = cursor.u8() != 0;
        }
    }

    /// The restore in place of the three parts: the world's state is replaced
    /// from the image and its pipelines are kept, as the Rapier snapshot
    /// continuation spike restored it.
    pub fn restore(&mut self, take: &Take) {
        assert!(take.held, "a correction asked for a tick the ring does not hold");
        let state: PhysicsWorld = bincode::deserialize(&take.image).expect("the library reads its own image");
        self.world.gravity = state.gravity;
        self.world.integration_parameters = state.integration_parameters;
        self.world.islands = state.islands;
        self.world.broad_phase = state.broad_phase;
        self.world.narrow_phase = state.narrow_phase;
        self.world.bodies = state.bodies;
        self.world.colliders = state.colliders;
        self.world.impulse_joints = state.impulse_joints;
        self.world.multibody_joints = state.multibody_joints;
        self.world.soft_bodies = state.soft_bodies;
        self.restore_tables_and_components(take);
    }

    /// The correction that needs no image: the components are restored, the
    /// library's world stays where it is, and the restored transforms and
    /// motion are written into its bodies.
    pub fn write_back(&mut self, take: &Take) {
        self.restore_tables_and_components(take);
        for (entity, c) in self.entities.iter().zip(&self.components) {
            if entity.definition.body_type != BODY_DYNAMIC {
                continue;
            }
            let body = &mut self.world.bodies[entity.handle];
            body.set_position(
                Pose::from_parts(
                    Vector::new(c.position[0], c.position[1], c.position[2]),
                    Rotation::from_xyzw(c.rotation[0], c.rotation[1], c.rotation[2], c.rotation[3]),
                ),
                true,
            );
            body.set_linvel(Vector::new(c.linear[0], c.linear[1], c.linear[2]), true);
            body.set_angvel(Vector::new(c.angular[0], c.angular[1], c.angular[2]), true);
        }
    }
}

pub fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

/// Empties a buffer and gives it room for the size asked and never beyond it,
/// so that a ring's memory is what its takes weigh.
fn fit(buffer: &mut Vec<u8>, size: usize) {
    buffer.clear();
    if buffer.capacity() < size {
        *buffer = Vec::with_capacity(size);
    }
}

#[derive(Default)]
pub struct Take {
    pub image: Vec<u8>,
    pub tables: Vec<u8>,
    pub canonical: Vec<u8>,
    pub tick: usize,
    pub held: bool,
}

impl Take {
    pub fn bytes(&self) -> usize {
        self.image.len() + self.tables.len() + self.canonical.len()
    }
}

pub struct TakeTimes {
    pub image_ms: f64,
    pub tables_ms: f64,
    pub canonical_ms: f64,
}

impl TakeTimes {
    pub fn total(&self) -> f64 {
        self.image_ms + self.tables_ms + self.canonical_ms
    }
}

pub struct Ring {
    slots: Vec<Take>,
    cadence: usize,
}

impl Ring {
    pub fn new(slot_count: usize, cadence: usize) -> Ring {
        Ring { slots: (0..slot_count).map(|_| Take::default()).collect(), cadence }
    }

    pub fn slot(&mut self, tick: usize) -> &mut Take {
        let index = (tick / self.cadence) % self.slots.len();
        &mut self.slots[index]
    }

    /// The take of a tick the ring holds, or a failure: a correction never
    /// starts from a tick that was overwritten.
    pub fn held(&self, tick: usize) -> &Take {
        let take = &self.slots[(tick / self.cadence) % self.slots.len()];
        assert!(take.held && take.tick == tick, "the ring no longer holds the tick a correction starts from");
        take
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The memory the ring holds: the capacity of every buffer of every slot.
    pub fn bytes(&self) -> usize {
        self.slots
            .iter()
            .map(|take| take.image.capacity() + take.tables.capacity() + take.canonical.capacity())
            .sum()
    }
}
