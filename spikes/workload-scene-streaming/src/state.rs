//! The domain state the streamed world and the sequential oracle share: the
//! entity columns and their per-tick hash, the instance records (residency,
//! the pending structural changes in record order, the remap table keyed by
//! instance, the input log) with the validation of the records, and the
//! resolution of a full reference through the remap table. The stepping, the
//! spawn and the despawn are implemented twice (the tick graph in `world.rs`,
//! the plain loop in `oracle.rs`). Throwaway, never product code.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use reference_workload::{CHUNK, Fnv1a};

use crate::grid::{AuthoredEntity, NO_CELL, Payload, PointOfInterest};
use crate::streaming::{InputLog, LoggedRecord, NO_REFERENCE, NOT_PRESENT, Record, Residency};

/// The entity columns of a world. Slots are allocated lowest free first and
/// grown at the end; a despawn frees its slots and bumps their generation.
#[derive(Clone, Debug)]
pub struct State {
    pub alive: Vec<bool>,
    pub generation: Vec<u32>,
    pub cell: Vec<u32>,
    pub local: Vec<u32>,
    pub reference_cell: Vec<u32>,
    pub reference_local: Vec<u32>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub mass: Vec<f32>,
    /// What the reference system read last: a packed position, the
    /// "not present" value, or the no-reference value.
    pub observed: Vec<u64>,
    pub energy: f32,
    pub point: PointOfInterest,
}

impl State {
    pub fn empty(point: PointOfInterest) -> State {
        State {
            alive: Vec::new(),
            generation: Vec::new(),
            cell: Vec::new(),
            local: Vec::new(),
            reference_cell: Vec::new(),
            reference_local: Vec::new(),
            x: Vec::new(),
            y: Vec::new(),
            vx: Vec::new(),
            vy: Vec::new(),
            mass: Vec::new(),
            observed: Vec::new(),
            energy: 0.0,
            point,
        }
    }

    pub fn len(&self) -> usize {
        self.alive.len()
    }

    pub fn chunk_count(&self) -> usize {
        self.len().div_ceil(CHUNK)
    }

    #[cfg(test)]
    pub fn alive_count(&self) -> usize {
        self.alive.iter().filter(|alive| **alive).count()
    }

    /// Appends one dead slot of generation 0.
    pub fn push_slot(&mut self) -> usize {
        self.alive.push(false);
        self.generation.push(0);
        self.cell.push(NO_CELL);
        self.local.push(0);
        self.reference_cell.push(NO_CELL);
        self.reference_local.push(0);
        self.x.push(0.0);
        self.y.push(0.0);
        self.vx.push(0.0);
        self.vy.push(0.0);
        self.mass.push(0.0);
        self.observed.push(NO_REFERENCE);
        self.len() - 1
    }

    /// Writes an authored entity of `cell` into a free slot: every column
    /// but the generation, which the slot keeps.
    pub fn write_authored(&mut self, slot: usize, cell: u32, entity: &AuthoredEntity) {
        debug_assert!(!self.alive[slot], "a spawn takes a free slot");
        self.alive[slot] = true;
        self.cell[slot] = cell;
        self.local[slot] = entity.local;
        let (reference_cell, reference_local) = entity.reference.map_or((NO_CELL, 0), |r| (r.cell, r.local));
        self.reference_cell[slot] = reference_cell;
        self.reference_local[slot] = reference_local;
        self.x[slot] = entity.x;
        self.y[slot] = entity.y;
        self.vx[slot] = entity.vx;
        self.vy[slot] = entity.vy;
        self.mass[slot] = entity.mass;
        self.observed[slot] = if entity.reference.is_some() { NOT_PRESENT } else { NO_REFERENCE };
    }

    /// Frees a slot: dead, and its generation bumped so that a stale runtime
    /// identifier never matches it again.
    pub fn free_slot(&mut self, slot: usize) {
        debug_assert!(self.alive[slot], "a despawn takes a live slot");
        self.alive[slot] = false;
        self.generation[slot] += 1;
    }

    /// The columns a spawn or a despawn writes and the passes never do: what
    /// the lagging buffer of the double-buffered graph must learn.
    pub fn copy_static_from(&mut self, other: &State, slot: usize) {
        self.alive[slot] = other.alive[slot];
        self.generation[slot] = other.generation[slot];
        self.cell[slot] = other.cell[slot];
        self.local[slot] = other.local[slot];
        self.reference_cell[slot] = other.reference_cell[slot];
        self.reference_local[slot] = other.reference_local[slot];
        self.mass[slot] = other.mass[slot];
    }

    /// The tick hash: FNV-1a 64 over the canonical bytes, slot by slot (the
    /// generation and the liveness; for a live slot the instance, the local
    /// identifier, the position, the velocity, the mass and the observed
    /// word), then the energy and the point of interest.
    pub fn hash(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.len() {
            hasher.write(&self.generation[slot].to_le_bytes());
            hasher.write(&[u8::from(self.alive[slot])]);
            if self.alive[slot] {
                hasher.write(&self.cell[slot].to_le_bytes());
                hasher.write(&self.local[slot].to_le_bytes());
                hasher.write(&self.x[slot].to_le_bytes());
                hasher.write(&self.y[slot].to_le_bytes());
                hasher.write(&self.vx[slot].to_le_bytes());
                hasher.write(&self.vy[slot].to_le_bytes());
                hasher.write(&self.mass[slot].to_le_bytes());
                hasher.write(&self.observed[slot].to_le_bytes());
            }
        }
        hasher.write(&self.energy.to_le_bytes());
        hasher.write(&self.point.x.to_le_bytes());
        hasher.write(&self.point.y.to_le_bytes());
        hasher.finish()
    }
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
    /// The target instance is not loaded: the deterministic absent value.
    Absent,
    /// The target entity's position.
    Present(f32, f32),
    /// The table named a slot that is not the target any more: a bug the
    /// hypotheses refute on.
    Stale,
}

/// Resolves a full reference through the remap table against the state the
/// passes read, verifying the slot named by the table is the target.
pub fn resolve(state: &State, remap: &BTreeMap<u32, Vec<u64>>, cell: u32, local: u32) -> Resolution {
    let Some(partition) = remap.get(&cell) else { return Resolution::Absent };
    let Some(&id) = partition.get(local as usize) else { return Resolution::Stale };
    let slot = slot_of(id);
    if slot < state.len()
        && state.alive[slot]
        && state.generation[slot] == generation_of(id)
        && state.cell[slot] == cell
        && state.local[slot] == local
    {
        Resolution::Present(state.x[slot], state.y[slot])
    } else {
        Resolution::Stale
    }
}

/// The per-tick counters of the reference system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadCounts {
    pub absent: u64,
    pub present: u64,
    pub stale: u64,
}

impl ReadCounts {
    pub fn record(&mut self, resolution: Resolution) -> u64 {
        match resolution {
            Resolution::Absent => {
                self.absent += 1;
                NOT_PRESENT
            }
            Resolution::Present(x, y) => {
                self.present += 1;
                crate::streaming::packed_position(x, y)
            }
            Resolution::Stale => {
                self.stale += 1;
                NOT_PRESENT
            }
        }
    }

    pub fn add(&mut self, other: ReadCounts) {
        self.absent += other.absent;
        self.present += other.present;
        self.stale += other.stale;
    }
}

/// A structural change awaiting the fixed point, in record order.
#[derive(Debug)]
pub enum Pending {
    Spawn { cell: u32, request: u64, next_local: u32, partition: Vec<u64>, elapsed: Duration },
    Despawn { cell: u32 },
}

/// The instance records of a world: residency per cell, the pending
/// structural changes, the arrived payloads, the remap table and the log.
pub struct Instances {
    pub residency: Vec<Residency>,
    pub pending: VecDeque<Pending>,
    /// The payloads of the cells arrived and not yet despawned, by cell.
    pub payloads: BTreeMap<u32, Arc<Payload>>,
    /// The remap table, partitioned by instance: local identifier to runtime identifier.
    pub remap: BTreeMap<u32, Vec<u64>>,
    pub log: InputLog,
    next_request: u64,
    remap_entries: usize,
    pub max_remap_entries: usize,
    pub max_loaded_cells: usize,
    /// Pending entries dropped because the cell's record no longer expected
    /// them (a bug the self-checks refute on).
    pub dropped_pending: u64,
}

impl Instances {
    pub fn new(cell_count: u32) -> Instances {
        Instances {
            residency: vec![Residency::Unloaded; cell_count as usize],
            pending: VecDeque::new(),
            payloads: BTreeMap::new(),
            remap: BTreeMap::new(),
            log: InputLog::default(),
            next_request: 1,
            remap_entries: 0,
            max_remap_entries: 0,
            max_loaded_cells: 0,
            dropped_pending: 0,
        }
    }

    pub fn loaded_cells(&self) -> usize {
        self.remap.len()
    }

    #[cfg(test)]
    pub fn remap_entries(&self) -> usize {
        self.remap_entries
    }

    fn log(&mut self, tick: u32, record: Record, accepted: bool) -> bool {
        self.log.records.push(LoggedRecord { tick, record, accepted });
        accepted
    }

    /// A load command: accepted for an unloaded cell, which becomes requested
    /// under a new request identifier (or the one a replayed record carries).
    pub fn apply_load(&mut self, tick: u32, cell: u32, request: Option<u64>) -> Option<u64> {
        let request = request.unwrap_or(self.next_request);
        let accepted = (cell as usize) < self.residency.len() && self.residency[cell as usize] == Residency::Unloaded;
        if accepted {
            self.residency[cell as usize] = Residency::Requested(request);
            self.next_request = self.next_request.max(request + 1);
        }
        self.log(tick, Record::Load { cell, request }, accepted).then_some(request)
    }

    /// An unload command: a requested cell is abandoned at once (its arrival
    /// will be ignored); an arrived, spawning or loaded cell queues its
    /// despawn behind the pending changes; anything else is rejected.
    pub fn apply_unload(&mut self, tick: u32, cell: u32) -> bool {
        let accepted = match self.residency.get(cell as usize) {
            Some(Residency::Requested(_)) => {
                self.residency[cell as usize] = Residency::Unloaded;
                true
            }
            Some(Residency::Arrived(_) | Residency::Spawning(_) | Residency::Loaded) => {
                self.residency[cell as usize] = Residency::Unloading;
                self.pending.push_back(Pending::Despawn { cell });
                true
            }
            _ => false,
        };
        self.log(tick, Record::Unload { cell }, accepted)
    }

    /// An arrival: accepted when the cell awaits this request and the payload
    /// is the cell's and hashes to the announced content hash; the spawn is
    /// then queued. Anything else is ignored, deterministically.
    pub fn apply_arrival(&mut self, tick: u32, request: u64, cell: u32, payload: Arc<Payload>, announced: u64) -> bool {
        let awaited = matches!(self.residency.get(cell as usize), Some(Residency::Requested(r)) if *r == request);
        let accepted = awaited && payload.cell == cell && payload.verify(announced);
        if accepted {
            self.residency[cell as usize] = Residency::Arrived(request);
            self.payloads.insert(cell, payload);
            self.pending.push_back(Pending::Spawn { cell, request, next_local: 0, partition: Vec::new(), elapsed: Duration::ZERO });
        }
        self.log(tick, Record::Arrival { request, cell, content_hash: announced }, accepted)
    }

    /// Whether the spawn at the front of the queue is still expected by its
    /// cell's record (arrived or spawning under the same request, or
    /// unloading with the despawn queued behind it).
    pub fn spawn_expected(&self, cell: u32, request: u64) -> bool {
        match self.residency[cell as usize] {
            Residency::Arrived(r) | Residency::Spawning(r) => r == request,
            Residency::Unloading => true,
            _ => false,
        }
    }

    pub fn mark_spawning(&mut self, cell: u32, request: u64) {
        if self.residency[cell as usize] != Residency::Unloading {
            self.residency[cell as usize] = Residency::Spawning(request);
        }
    }

    /// The last slice is done: the partition enters the remap table and the
    /// instance reads as present from the next tick on.
    pub fn complete_spawn(&mut self, cell: u32, partition: Vec<u64>) {
        self.remap_entries += partition.len();
        self.remap.insert(cell, partition);
        if self.residency[cell as usize] != Residency::Unloading {
            self.residency[cell as usize] = Residency::Loaded;
        }
        self.max_remap_entries = self.max_remap_entries.max(self.remap_entries);
        self.max_loaded_cells = self.max_loaded_cells.max(self.remap.len());
    }

    /// Takes the partition of a cell about to be despawned, if complete.
    pub fn take_partition(&mut self, cell: u32) -> Option<Vec<u64>> {
        let partition = self.remap.remove(&cell)?;
        self.remap_entries -= partition.len();
        Some(partition)
    }

    pub fn complete_despawn(&mut self, cell: u32) {
        self.residency[cell as usize] = Residency::Unloaded;
        self.payloads.remove(&cell);
    }

    pub fn drop_pending(&mut self) {
        self.pending.pop_front();
        self.dropped_pending += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{FullReference, GridConfig};

    fn payload(cell: u32) -> Arc<Payload> {
        Arc::new(Payload::import(&GridConfig::new(3, 5, 1), cell))
    }

    #[test]
    fn the_records_follow_the_residency_state_machine() {
        let mut instances = Instances::new(9);
        assert_eq!(instances.apply_load(1, 4, None), Some(1));
        assert_eq!(instances.apply_load(1, 4, None), None, "a duplicate is rejected");
        assert_eq!(instances.apply_load(1, 99, None), None, "outside the grid");
        assert_eq!(instances.residency[4], Residency::Requested(1));
        let p = payload(4);
        assert!(!instances.apply_arrival(2, 2, 4, p.clone(), p.content_hash), "another request");
        assert!(!instances.apply_arrival(2, 1, 4, p.clone(), p.content_hash ^ 1), "a wrong hash");
        assert!(!instances.apply_arrival(2, 1, 4, payload(5), payload(5).content_hash), "another cell's payload");
        assert!(instances.apply_arrival(2, 1, 4, p.clone(), p.content_hash));
        assert_eq!(instances.residency[4], Residency::Arrived(1));
        assert!(matches!(instances.pending.front(), Some(Pending::Spawn { cell: 4, request: 1, .. })));
        assert!(instances.spawn_expected(4, 1));
        instances.mark_spawning(4, 1);
        assert_eq!(instances.residency[4], Residency::Spawning(1));
        instances.complete_spawn(4, vec![runtime_id(0, 0), runtime_id(1, 0)]);
        assert_eq!(instances.residency[4], Residency::Loaded);
        assert_eq!((instances.loaded_cells(), instances.remap_entries(), instances.max_remap_entries), (1, 2, 2));
        assert!(instances.apply_unload(3, 4));
        assert_eq!(instances.residency[4], Residency::Unloading);
        assert!(!instances.apply_unload(3, 4), "already unloading");
        assert!(!instances.apply_unload(3, 0), "never loaded");
        assert_eq!(instances.take_partition(4).map(|p| p.len()), Some(2));
        assert_eq!(instances.remap_entries(), 0);
        instances.complete_despawn(4);
        assert_eq!(instances.residency[4], Residency::Unloaded);
        assert!(instances.payloads.is_empty());
        // A requested cell unloaded before its arrival: the arrival is ignored.
        assert_eq!(instances.apply_load(5, 2, None), Some(2));
        assert!(instances.apply_unload(6, 2));
        assert_eq!(instances.residency[2], Residency::Unloaded);
        assert!(!instances.apply_arrival(7, 2, 2, payload(2), payload(2).content_hash));
        assert_eq!(instances.apply_load(8, 2, Some(40)), Some(40), "a replayed request keeps its identifier");
        assert_eq!(instances.apply_load(8, 3, None), Some(41));
        assert_eq!(instances.log.records.len(), 15);
        assert_eq!(instances.log.rejected(), 8);
    }

    #[test]
    fn a_reference_resolves_to_the_target_or_to_absent_and_never_to_a_stale_slot() {
        let mut state = State::empty(PointOfInterest { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0 });
        let slot = state.push_slot();
        state.write_authored(slot, 4, &AuthoredEntity { local: 3, x: 1.0, y: 2.0, vx: 0.0, vy: 0.0, mass: 1.0, reference: Some(FullReference { cell: 5, local: 0 }) });
        assert_eq!(state.observed[slot], NOT_PRESENT);
        let mut remap = BTreeMap::new();
        assert_eq!(resolve(&state, &remap, 4, 3), Resolution::Absent);
        remap.insert(4, vec![0, 0, 0, runtime_id(slot, 0)]);
        assert_eq!(resolve(&state, &remap, 4, 3), Resolution::Present(1.0, 2.0));
        assert_eq!(resolve(&state, &remap, 4, 7), Resolution::Stale, "a local identifier outside the partition");
        state.free_slot(slot);
        assert_eq!(resolve(&state, &remap, 4, 3), Resolution::Stale, "the slot was freed");
        assert_eq!(state.generation[slot], 1);
        let mut counts = ReadCounts::default();
        assert_eq!(counts.record(Resolution::Absent), NOT_PRESENT);
        assert_eq!(counts.record(Resolution::Present(1.0, 2.0)), crate::streaming::packed_position(1.0, 2.0));
        assert_eq!(counts.record(Resolution::Stale), NOT_PRESENT);
        assert_eq!(counts, ReadCounts { absent: 1, present: 1, stale: 1 });
        assert_eq!((slot_of(runtime_id(7, 9)), generation_of(runtime_id(7, 9))), (7, 9));
    }

    #[test]
    fn the_hash_covers_live_slots_and_the_point() {
        let point = PointOfInterest { x: 3.0, y: 4.0, vx: 0.0, vy: 0.0 };
        let mut state = State::empty(point);
        let empty = state.hash();
        let slot = state.push_slot();
        let dead = state.hash();
        assert_ne!(empty, dead, "a dead slot still hashes its generation and liveness");
        state.write_authored(slot, 1, &AuthoredEntity { local: 0, x: 1.0, y: 2.0, vx: 0.5, vy: 0.5, mass: 2.0, reference: None });
        let live = state.hash();
        assert_ne!(dead, live);
        state.observed[slot] = 1;
        assert_ne!(live, state.hash(), "the observed word is in the hash");
        state.observed[slot] = NO_REFERENCE;
        state.point.x = 5.0;
        assert_ne!(live, state.hash(), "the point is in the hash");
        let mut copy = State::empty(point);
        copy.push_slot();
        copy.copy_static_from(&state, slot);
        assert_eq!((copy.alive[slot], copy.cell[slot], copy.mass[slot]), (true, 1, 2.0));
        assert_eq!(copy.x[slot], 0.0, "dynamic columns are the passes' to write");
    }
}
