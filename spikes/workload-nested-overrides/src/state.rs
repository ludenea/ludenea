//! The domain state the world and the sequential oracle share: the entity
//! columns and their two per-tick hashes, an instance's identity, the
//! resolution of a full reference through an instance's table, the five
//! kinds of override as operations on a world, the records of the input log,
//! and the kernels of the passes. The stepping, the records and the
//! instantiation are implemented twice (`world.rs`, `oracle.rs`). Throwaway,
//! never product code.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use reference_workload::{CHUNK, Fnv1a, INTERACTION_RADIUS, REPULSION_STRENGTH};

use crate::scene::{ApplyError, Change, INSTANCING, LABEL, MASS, Override, POSITION, Proto, REFERENCE, VELOCITY, bit, bits_float, field_count, float_bits};

/// No record: the membership of a staging slot, the target of a reference
/// that leaves its family.
pub const NONE: u32 = u32::MAX;
/// What the reference system writes for an entity without a reference.
pub const NO_REFERENCE: u64 = u64::MAX;
/// The deterministic value of a reference whose target is not present.
pub const NOT_PRESENT: u64 = u64::MAX - 1;
/// An entity the passes move: it has a position, a velocity and a mass.
pub const DYNAMIC: u8 = bit(MASS) | bit(POSITION) | bit(VELOCITY);
pub const BIN_SIDE: f32 = 16.0;

/// An instance identity: the root key the load command chose, then the local
/// identifier of the instancing entity at each level. Compared element by
/// element, the root key first: the canonical order of instances.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct InstancePath {
    pub root: u64,
    pub locals: Vec<u64>,
}

impl InstancePath {
    pub fn root(root: u64) -> InstancePath {
        InstancePath { root, locals: Vec::new() }
    }

    pub fn child(&self, entity: u64) -> InstancePath {
        let mut locals = self.locals.clone();
        locals.push(entity);
        InstancePath { root: self.root, locals }
    }

    pub fn starts_with(&self, other: &InstancePath) -> bool {
        self.root == other.root && self.locals.starts_with(&other.locals)
    }

    pub fn digest(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        hasher.write(&self.root.to_le_bytes());
        hasher.write(&(self.locals.len() as u64).to_le_bytes());
        for local in &self.locals {
            hasher.write(&local.to_le_bytes());
        }
        hasher.finish()
    }

    /// The instance a relative reference names from this one: `ups` levels
    /// up, then down through the instancing entity `down` when it is not
    /// zero. None when the reference leaves the family.
    pub fn relative(&self, ups: u64, down: u64) -> Option<InstancePath> {
        let keep = self.locals.len().checked_sub(usize::try_from(ups).ok()?)?;
        let mut locals = self.locals[..keep].to_vec();
        if down != 0 {
            locals.push(down);
        }
        Some(InstancePath { root: self.root, locals })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailReason {
    /// The child's payload is at another content hash than the one its
    /// parent's payload names.
    AnotherContentHash,
    /// A list could not be applied to the payload.
    OverrideRefused,
}

/// The actual residency of an instance record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actual {
    Absent,
    Requested,
    Loaded,
    Failed(FailReason),
}

impl Actual {
    fn code(self) -> u8 {
        match self {
            Actual::Absent => 0,
            Actual::Requested => 1,
            Actual::Loaded => 2,
            Actual::Failed(FailReason::AnotherContentHash) => 3,
            Actual::Failed(FailReason::OverrideRefused) => 4,
        }
    }
}

/// What the instantiation did at a structural fixed point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// An instance spawned, with a digest of its members by local identifier.
    Spawned { tick: u32, path: InstancePath, members: u32, digest: u64 },
    Despawned { tick: u32, path: InstancePath, members: u32 },
    Failed { tick: u32, path: InstancePath, reason: FailReason },
}

/// A record of the input log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Load { root: u64, asset: u64, required: bool },
    Unload { root: u64 },
    Arrival { asset: u64, content_hash: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Logged {
    pub tick: u32,
    pub record: Record,
    pub accepted: bool,
}

/// The entity columns of a world. Slots are allocated lowest free first and
/// grown at the end; a despawn frees its slot and bumps its generation.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub alive: Vec<bool>,
    pub generation: Vec<u32>,
    /// The membership: the instance record of the entity.
    pub record: Vec<u32>,
    pub local: Vec<u64>,
    pub parent: Vec<u64>,
    pub mask: Vec<u8>,
    /// Alive with a position: what the spatial hashing lists.
    pub spatial: Vec<bool>,
    pub asset: Vec<u64>,
    pub anchor_x: Vec<f32>,
    pub anchor_y: Vec<f32>,
    pub label: Vec<u64>,
    pub mass: Vec<f32>,
    pub reference_ups: Vec<u64>,
    pub reference_down: Vec<u64>,
    pub reference_local: Vec<u64>,
    /// The record of the instance the reference names, resolved at the spawn.
    pub reference_record: Vec<u32>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    /// What the reference system read last.
    pub observed: Vec<u64>,
    pub energy: f32,
}

impl State {
    pub fn len(&self) -> usize {
        self.alive.len()
    }

    pub fn chunk_count(&self) -> usize {
        self.len().div_ceil(CHUNK)
    }

    pub fn alive_count(&self) -> usize {
        self.alive.iter().filter(|alive| **alive).count()
    }

    pub fn has(&self, slot: usize, component: u8) -> bool {
        self.mask[slot] & bit(component) != 0
    }

    /// Appends one dead slot of generation 0.
    pub fn push_slot(&mut self) -> usize {
        self.alive.push(false);
        self.generation.push(0);
        self.record.push(NONE);
        self.local.push(0);
        self.parent.push(0);
        self.mask.push(0);
        self.spatial.push(false);
        self.asset.push(0);
        self.anchor_x.push(0.0);
        self.anchor_y.push(0.0);
        self.label.push(0);
        self.mass.push(0.0);
        self.reference_ups.push(0);
        self.reference_down.push(0);
        self.reference_local.push(0);
        self.reference_record.push(NONE);
        self.x.push(0.0);
        self.y.push(0.0);
        self.vx.push(0.0);
        self.vy.push(0.0);
        self.observed.push(NO_REFERENCE);
        self.len() - 1
    }

    fn clear_components(&mut self, slot: usize) {
        self.mask[slot] = 0;
        self.spatial[slot] = false;
        self.asset[slot] = 0;
        self.anchor_x[slot] = 0.0;
        self.anchor_y[slot] = 0.0;
        self.label[slot] = 0;
        self.mass[slot] = 0.0;
        self.reference_ups[slot] = 0;
        self.reference_down[slot] = 0;
        self.reference_local[slot] = 0;
        self.reference_record[slot] = NONE;
        self.x[slot] = 0.0;
        self.y[slot] = 0.0;
        self.vx[slot] = 0.0;
        self.vy[slot] = 0.0;
        self.observed[slot] = NO_REFERENCE;
    }

    /// Writes one field; a position is authored relative to the origin of
    /// its instance.
    pub fn set_field(&mut self, slot: usize, component: u8, field: u8, value: u64, origin: (f32, f32)) {
        match (component, field) {
            (INSTANCING, 0) => self.asset[slot] = value,
            (INSTANCING, 1) => self.anchor_x[slot] = bits_float(value),
            (INSTANCING, _) => self.anchor_y[slot] = bits_float(value),
            (LABEL, _) => self.label[slot] = value,
            (MASS, _) => self.mass[slot] = bits_float(value),
            (POSITION, 0) => self.x[slot] = bits_float(value) + origin.0,
            (POSITION, _) => self.y[slot] = bits_float(value) + origin.1,
            (REFERENCE, 0) => self.reference_ups[slot] = value,
            (REFERENCE, 1) => self.reference_down[slot] = value,
            (REFERENCE, _) => self.reference_local[slot] = value,
            (VELOCITY, 0) => self.vx[slot] = bits_float(value),
            (VELOCITY, _) => self.vy[slot] = bits_float(value),
            _ => unreachable!("six components"),
        }
    }

    pub fn add_component(&mut self, slot: usize, component: u8, values: &[u64], origin: (f32, f32)) {
        self.mask[slot] |= bit(component);
        for (field, value) in values.iter().take(field_count(component)).enumerate() {
            self.set_field(slot, component, field as u8, *value, origin);
        }
        match component {
            POSITION => self.spatial[slot] = true,
            REFERENCE => self.observed[slot] = NOT_PRESENT,
            _ => {}
        }
    }

    pub fn remove_component(&mut self, slot: usize, component: u8) {
        self.mask[slot] &= !bit(component);
        for field in 0..field_count(component) {
            self.set_field(slot, component, field as u8, 0, (0.0, 0.0));
        }
        match component {
            POSITION => self.spatial[slot] = false,
            REFERENCE => {
                self.reference_record[slot] = NONE;
                self.observed[slot] = NO_REFERENCE;
            }
            _ => {}
        }
    }

    /// Writes a prototype into a free slot as a member of `record`: every
    /// column but the generation, which the slot keeps.
    pub fn write_proto(&mut self, slot: usize, record: u32, proto: &Proto, origin: (f32, f32), reference_record: u32) {
        debug_assert!(!self.alive[slot], "a spawn takes a free slot");
        self.clear_components(slot);
        self.alive[slot] = true;
        self.record[slot] = record;
        self.local[slot] = proto.local;
        self.parent[slot] = proto.parent;
        for component in 0..crate::scene::COMPONENT_COUNT {
            if proto.has(component) {
                self.add_component(slot, component, &proto.values(component), origin);
            }
        }
        if proto.has(REFERENCE) {
            self.reference_record[slot] = reference_record;
        }
    }

    /// Reads a live slot back as a prototype. Exact for a slot written at
    /// the origin (0, 0), which is where the staging area holds its members.
    pub fn read_proto(&self, slot: usize) -> Proto {
        let mut proto = Proto::new(self.local[slot], self.parent[slot]);
        let values: [(u8, [u64; 3]); 6] = [
            (INSTANCING, [self.asset[slot], float_bits(self.anchor_x[slot]), float_bits(self.anchor_y[slot])]),
            (LABEL, [self.label[slot], 0, 0]),
            (MASS, [float_bits(self.mass[slot]), 0, 0]),
            (POSITION, [float_bits(self.x[slot]), float_bits(self.y[slot]), 0]),
            (REFERENCE, [self.reference_ups[slot], self.reference_down[slot], self.reference_local[slot]]),
            (VELOCITY, [float_bits(self.vx[slot]), float_bits(self.vy[slot]), 0]),
        ];
        for (component, values) in values {
            if self.has(slot, component) {
                proto.add(component, &values);
            }
        }
        proto
    }

    /// Frees a slot: dead, and its generation bumped so that a stale runtime
    /// identifier never matches it again.
    pub fn free_slot(&mut self, slot: usize) {
        debug_assert!(self.alive[slot], "a despawn takes a live slot");
        self.alive[slot] = false;
        self.spatial[slot] = false;
        self.generation[slot] += 1;
    }

    /// The columns the structural fixed point writes and the passes never
    /// do: what the lagging buffer of the double-buffered graph must learn.
    pub fn copy_static_from(&mut self, other: &State, slot: usize) {
        self.alive[slot] = other.alive[slot];
        self.generation[slot] = other.generation[slot];
        self.record[slot] = other.record[slot];
        self.local[slot] = other.local[slot];
        self.parent[slot] = other.parent[slot];
        self.mask[slot] = other.mask[slot];
        self.spatial[slot] = other.spatial[slot];
        self.asset[slot] = other.asset[slot];
        self.anchor_x[slot] = other.anchor_x[slot];
        self.anchor_y[slot] = other.anchor_y[slot];
        self.label[slot] = other.label[slot];
        self.mass[slot] = other.mass[slot];
        self.reference_ups[slot] = other.reference_ups[slot];
        self.reference_down[slot] = other.reference_down[slot];
        self.reference_local[slot] = other.reference_local[slot];
        self.reference_record[slot] = other.reference_record[slot];
    }

    /// The canonical bytes of a member apart from its identity: its parent,
    /// its components in the order of their stable names with their fields
    /// in declaration order, a reference by the identity it names, and the
    /// observed word.
    fn write_member(&self, hasher: &mut Fnv1a, slot: usize, digests: &[u64]) {
        hasher.write(&self.parent[slot].to_le_bytes());
        hasher.write(&[self.mask[slot]]);
        if self.has(slot, INSTANCING) {
            hasher.write(&self.asset[slot].to_le_bytes());
            hasher.write(&self.anchor_x[slot].to_le_bytes());
            hasher.write(&self.anchor_y[slot].to_le_bytes());
        }
        if self.has(slot, LABEL) {
            hasher.write(&self.label[slot].to_le_bytes());
        }
        if self.has(slot, MASS) {
            hasher.write(&self.mass[slot].to_le_bytes());
        }
        if self.has(slot, POSITION) {
            hasher.write(&self.x[slot].to_le_bytes());
            hasher.write(&self.y[slot].to_le_bytes());
        }
        if self.has(slot, REFERENCE) {
            let target = self.reference_record[slot];
            hasher.write(&(if target == NONE { 0 } else { digests[target as usize] }).to_le_bytes());
            hasher.write(&self.reference_ups[slot].to_le_bytes());
            hasher.write(&self.reference_down[slot].to_le_bytes());
            hasher.write(&self.reference_local[slot].to_le_bytes());
        }
        if self.has(slot, VELOCITY) {
            hasher.write(&self.vx[slot].to_le_bytes());
            hasher.write(&self.vy[slot].to_le_bytes());
        }
        hasher.write(&self.observed[slot].to_le_bytes());
    }

    /// The state hash: FNV-1a 64 slot by slot, in the order of the runtime
    /// identifiers (the generation and the liveness; for a live slot its
    /// instance, its local identifier and its member bytes), then the
    /// energy. It depends on which slot every entity was allocated.
    pub fn hash(&self, digests: &[u64]) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.len() {
            hasher.write(&self.generation[slot].to_le_bytes());
            hasher.write(&[u8::from(self.alive[slot])]);
            if self.alive[slot] {
                hasher.write(&digests[self.record[slot] as usize].to_le_bytes());
                hasher.write(&self.local[slot].to_le_bytes());
                self.write_member(&mut hasher, slot, digests);
            }
        }
        hasher.write(&self.energy.to_le_bytes());
        hasher.finish()
    }

    /// A digest of the members of one instance, in the order of their local
    /// identifiers: what a spawn produced, whatever the slots it took.
    pub fn members_digest(&self, members: &[(u64, u64)], digests: &[u64]) -> u64 {
        let mut hasher = Fnv1a::new();
        for (local, id) in members {
            hasher.write(&local.to_le_bytes());
            self.write_member(&mut hasher, slot_of(*id), digests);
        }
        hasher.finish()
    }
}

/// The content hash: the instances that are not absent in the canonical
/// order of their identities, each with its residency and, loaded, its
/// members in the order of their local identifiers. Every entity is named by
/// its full reference and no runtime identifier enters.
pub fn content_hash<'a>(state: &State, digests: &[u64], instances: impl Iterator<Item = (&'a InstancePath, Actual, &'a [(u64, u64)])>) -> u64 {
    let mut hasher = Fnv1a::new();
    for (path, actual, members) in instances {
        if actual == Actual::Absent {
            continue;
        }
        hasher.write(&path.digest().to_le_bytes());
        hasher.write(&[actual.code()]);
        hasher.write(&(members.len() as u64).to_le_bytes());
        for (local, id) in members {
            hasher.write(&local.to_le_bytes());
            state.write_member(&mut hasher, slot_of(*id), digests);
        }
    }
    hasher.write(&state.energy.to_le_bytes());
    hasher.finish()
}

/// A runtime identifier: the slot and the generation it was allocated at.
pub fn runtime_id(slot: usize, generation: u32) -> u64 {
    (slot as u64) | (u64::from(generation) << 32)
}

pub fn slot_of(id: u64) -> usize {
    (id & 0xffff_ffff) as usize
}

pub fn generation_of(id: u64) -> u32 {
    (id >> 32) as u32
}

/// The outcome of one reference read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resolution {
    /// The target instance is not loaded or does not hold the entity: the
    /// deterministic absent value.
    Absent,
    /// The target entity's position.
    Present(f32, f32),
    /// The table named a slot that is not the target: a bug the hypotheses
    /// refute on.
    Stale,
}

/// Resolves a full reference through the table of its target instance
/// (the members sorted by local identifier), verifying that the slot the
/// table names is the target.
pub fn resolve(state: &State, loaded: bool, members: &[(u64, u64)], record: u32, local: u64) -> Resolution {
    if !loaded {
        return Resolution::Absent;
    }
    let Ok(at) = members.binary_search_by_key(&local, |member| member.0) else { return Resolution::Absent };
    let id = members[at].1;
    let slot = slot_of(id);
    if slot < state.len() && state.alive[slot] && state.generation[slot] == generation_of(id) && state.record[slot] == record && state.local[slot] == local {
        Resolution::Present(state.x[slot], state.y[slot])
    } else {
        Resolution::Stale
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadCounts {
    pub absent: u64,
    pub present: u64,
    pub stale: u64,
}

impl ReadCounts {
    /// Counts a resolution and gives the word the entity observes.
    pub fn record(&mut self, resolution: Resolution) -> u64 {
        match resolution {
            Resolution::Absent => {
                self.absent += 1;
                NOT_PRESENT
            }
            Resolution::Present(x, y) => {
                self.present += 1;
                (u64::from(x.to_bits()) << 32) | u64::from(y.to_bits())
            }
            Resolution::Stale => {
                self.stale += 1;
                NOT_PRESENT
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The five kinds of override as operations on a world
// ---------------------------------------------------------------------------

/// Where world operations allocate: one buffer (the staging area, the
/// oracle) or the leading buffer of a double-buffered world with the lagging
/// one kept at the same length and told which slots changed.
pub struct Substrate<'a> {
    pub state: &'a mut State,
    pub lagging: Option<&'a mut State>,
    pub free: &'a mut BinaryHeap<Reverse<usize>>,
    pub changed: Option<&'a mut Vec<usize>>,
}

impl Substrate<'_> {
    /// The lowest free slot, or a new one at the end.
    pub fn allocate(&mut self) -> usize {
        let slot = match self.free.pop() {
            Some(Reverse(slot)) => slot,
            None => {
                if let Some(lagging) = self.lagging.as_mut() {
                    lagging.push_slot();
                }
                self.state.push_slot()
            }
        };
        self.touch(slot);
        slot
    }

    pub fn release(&mut self, slot: usize) {
        self.state.free_slot(slot);
        self.free.push(Reverse(slot));
        self.touch(slot);
    }

    pub fn touch(&mut self, slot: usize) {
        if let Some(changed) = self.changed.as_mut() {
            changed.push(slot);
        }
    }
}

/// Applies the items of one list in its canonical order (entities removed,
/// entities added, components removed, components added, field values) to
/// the spawned members of one instance, as structural and field operations
/// on the world: the ordered application after a spawn. `members` is the
/// instance's table, kept sorted; `target_record` resolves the instance a
/// reference names.
pub fn apply_in_world(
    substrate: &mut Substrate<'_>,
    members: &mut Vec<(u64, u64)>,
    record: u32,
    origin: (f32, f32),
    list: &[Override],
    target_record: &mut dyn FnMut(u64, u64) -> u32,
) -> Result<(), ApplyError> {
    for item in list.iter().filter(|item| item.path.is_empty()) {
        let refused = ApplyError { target: item.target, kind: item.kind() };
        let found = members.binary_search_by_key(&item.target, |member| member.0);
        match &item.change {
            Change::RemoveEntity => {
                let at = found.map_err(|_| refused)?;
                substrate.release(slot_of(members.remove(at).1));
            }
            Change::AddEntity(proto) => {
                let at = found.err().ok_or(refused)?;
                let slot = substrate.allocate();
                let reference = if proto.has(REFERENCE) { target_record(proto.get(REFERENCE, 0), proto.get(REFERENCE, 1)) } else { NONE };
                substrate.state.write_proto(slot, record, proto, origin, reference);
                members.insert(at, (proto.local, runtime_id(slot, substrate.state.generation[slot])));
            }
            Change::RemoveComponent(component) => {
                let slot = slot_of(members[found.map_err(|_| refused)?].1);
                if !substrate.state.has(slot, *component) {
                    return Err(refused);
                }
                substrate.state.remove_component(slot, *component);
                substrate.touch(slot);
            }
            Change::AddComponent(component, values) => {
                let slot = slot_of(members[found.map_err(|_| refused)?].1);
                if substrate.state.has(slot, *component) {
                    return Err(refused);
                }
                substrate.state.add_component(slot, *component, values, origin);
                if *component == REFERENCE {
                    substrate.state.reference_record[slot] = target_record(values[0], values[1]);
                }
                substrate.touch(slot);
            }
            Change::SetField(component, field, value) => {
                let slot = slot_of(members[found.map_err(|_| refused)?].1);
                if !substrate.state.has(slot, *component) {
                    return Err(refused);
                }
                substrate.state.set_field(slot, *component, *field, *value, origin);
                if *component == REFERENCE {
                    let (ups, down) = (substrate.state.reference_ups[slot], substrate.state.reference_down[slot]);
                    substrate.state.reference_record[slot] = target_record(ups, down);
                }
                substrate.touch(slot);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The kernels the passes and the oracle share
// ---------------------------------------------------------------------------

/// The workload's cell lists as a counting sort over bins of `BIN_SIDE`
/// covering the world; a bin lists the slots with a position that fall in
/// it, in increasing slot order.
#[derive(Clone, Debug)]
pub struct SpatialHash {
    bins_per_side: usize,
    bin_start: Vec<u32>,
    slots: Vec<u32>,
    cursor: Vec<u32>,
}

impl SpatialHash {
    pub fn new(world_side: f32) -> SpatialHash {
        let bins_per_side = (world_side / BIN_SIDE) as usize;
        let bins = bins_per_side * bins_per_side;
        SpatialHash { bins_per_side, bin_start: vec![0; bins + 1], slots: Vec::new(), cursor: vec![0; bins + 1] }
    }

    fn bin_of(&self, x: f32, y: f32) -> (usize, usize) {
        let last = self.bins_per_side - 1;
        (((x / BIN_SIDE).floor() as usize).min(last), ((y / BIN_SIDE).floor() as usize).min(last))
    }

    pub fn build(&mut self, spatial: &[bool], x: &[f32], y: &[f32]) {
        let bins = self.bins_per_side * self.bins_per_side;
        self.bin_start.clear();
        self.bin_start.resize(bins + 1, 0);
        for slot in 0..spatial.len() {
            if spatial[slot] {
                let (bx, by) = self.bin_of(x[slot], y[slot]);
                self.bin_start[by * self.bins_per_side + bx + 1] += 1;
            }
        }
        for bin in 0..bins {
            self.bin_start[bin + 1] += self.bin_start[bin];
        }
        self.cursor.clear();
        self.cursor.extend_from_slice(&self.bin_start);
        self.slots.clear();
        self.slots.resize(self.bin_start[bins] as usize, 0);
        for slot in 0..spatial.len() {
            if spatial[slot] {
                let (bx, by) = self.bin_of(x[slot], y[slot]);
                let bin = by * self.bins_per_side + bx;
                self.slots[self.cursor[bin] as usize] = slot as u32;
                self.cursor[bin] += 1;
            }
        }
    }

    fn slots_in(&self, bx: usize, by: usize) -> &[u32] {
        let bin = by * self.bins_per_side + bx;
        &self.slots[self.bin_start[bin] as usize..self.bin_start[bin + 1] as usize]
    }
}

/// System 2 of the workload for one slot, over this world's bins: the same
/// operations in the same order as the reference kernel, the neighbours of a
/// bin summed in increasing slot order.
pub fn repulsion_force(x: &[f32], y: &[f32], hash: &SpatialHash, i: usize) -> (f32, f32) {
    let (xi, yi) = (x[i], y[i]);
    let (bx, by) = hash.bin_of(xi, yi);
    let side = hash.bins_per_side as isize;
    let mut fx = 0.0_f32;
    let mut fy = 0.0_f32;
    for b in -1_isize..=1 {
        for a in -1_isize..=1 {
            let (gx, gy) = (bx as isize + a, by as isize + b);
            if gx < 0 || gy < 0 || gx >= side || gy >= side {
                continue;
            }
            for &j in hash.slots_in(gx as usize, gy as usize) {
                let j = j as usize;
                if j == i {
                    continue;
                }
                let dx = x[j] - xi;
                let dy = y[j] - yi;
                let d2 = (dx * dx) + (dy * dy);
                if d2 < INTERACTION_RADIUS * INTERACTION_RADIUS && d2 > 0.0 {
                    let d = d2.sqrt();
                    let s = (REPULSION_STRENGTH * (INTERACTION_RADIUS - d)) / d;
                    fx -= dx * s;
                    fy -= dy * s;
                }
            }
        }
    }
    (fx, fy)
}

/// System 5 of the workload for one axis, inside the world's box `[0, side)`.
pub fn bounced_in(mut p: f32, mut v: f32, side: f32) -> (f32, f32) {
    if p < 0.0 {
        p = -p;
        v = -v;
    }
    if p >= side {
        p = (2.0 * side) - p;
        v = -v;
    }
    if p >= side {
        p = side - 0.0625;
    }
    (p, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_orders_instances_and_names_relatives() {
        let level = InstancePath::root(2);
        let building = level.child(9);
        let room = building.child(4);
        assert!(level < building && building < room && room < level.child(10) && room < InstancePath::root(3));
        assert!(room.starts_with(&level) && !level.starts_with(&room));
        assert_eq!(room.relative(0, 0), Some(room.clone()));
        assert_eq!(room.relative(1, 0), Some(building.clone()));
        assert_eq!(room.relative(1, 5), Some(building.child(5)));
        assert_eq!(level.relative(1, 0), None, "a reference that leaves its family names nothing");
        assert_ne!(room.digest(), building.child(5).digest());
    }

    #[test]
    fn a_slot_reads_back_what_was_written_and_a_freed_slot_is_stale() {
        let mut state = State::default();
        let slot = state.push_slot();
        let proto = Proto::new(7, 3).with(POSITION, &[float_bits(1.5), float_bits(2.5)]).with(REFERENCE, &[1, 0, 9]).with(LABEL, &[4]);
        state.write_proto(slot, 0, &proto, (0.0, 0.0), NONE);
        assert_eq!(state.read_proto(slot), proto);
        assert_eq!(state.observed[slot], NOT_PRESENT);
        let members = vec![(7, runtime_id(slot, 0))];
        assert_eq!(resolve(&state, true, &members, 0, 7), Resolution::Present(1.5, 2.5));
        assert_eq!(resolve(&state, false, &members, 0, 7), Resolution::Absent);
        assert_eq!(resolve(&state, true, &members, 0, 8), Resolution::Absent, "an entity the instance does not hold is not present");
        state.free_slot(slot);
        assert_eq!(resolve(&state, true, &members, 0, 7), Resolution::Stale);
    }

    #[test]
    fn the_world_operations_follow_the_list_and_keep_the_table_sorted() {
        let mut state = State::default();
        let mut free = BinaryHeap::new();
        let mut members = Vec::new();
        let mut substrate = Substrate { state: &mut state, lagging: None, free: &mut free, changed: None };
        for local in [10, 11, 12] {
            let slot = substrate.allocate();
            substrate.state.write_proto(slot, 0, &Proto::new(local, 0).with(LABEL, &[local]), (0.0, 0.0), NONE);
            members.push((local, runtime_id(slot, 0)));
        }
        let list = crate::scene::sorted(vec![
            Override::new(&[], 11, Change::RemoveEntity),
            Override::new(&[], 5, Change::AddEntity(Proto::new(5, 0).with(MASS, &[float_bits(2.0)]))),
            Override::new(&[], 10, Change::RemoveComponent(LABEL)),
            Override::new(&[], 12, Change::AddComponent(POSITION, [float_bits(1.0), float_bits(2.0), 0])),
            Override::new(&[], 12, Change::SetField(LABEL, 0, 99)),
            Override::new(&[7], 12, Change::RemoveEntity),
        ]);
        apply_in_world(&mut substrate, &mut members, 0, (8.0, 0.0), &list, &mut |_, _| NONE).unwrap();
        assert_eq!(members.iter().map(|member| member.0).collect::<Vec<_>>(), vec![5, 10, 12]);
        assert_eq!(members[0].1, runtime_id(1, 1), "the added entity takes the slot the removed one freed, at its next generation");
        assert_eq!(state.read_proto(0), Proto::new(10, 0));
        assert_eq!((state.x[2], state.label[2], state.spatial[2]), (9.0, 99, true));
        assert_eq!(state.alive_count(), 3);
    }
}
