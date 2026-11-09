//! The sequential oracle: the same schedule of records (a recorded input
//! log) stepped single-threaded by a plain loop over one buffer, with the
//! spawn, the despawn, the slot allocation and the slicing written again
//! rather than shared with the tick graph, and an independent check of every
//! reference read against the oracle's own residency. Throwaway, never
//! product code.

use std::collections::BTreeMap;

use reference_workload::{CHUNK, chunk_range, integrated, kinetic_energy, repelled_velocity};

use crate::grid::{GridConfig, NO_CELL, Path, PayloadStore, SpatialHash, bounced_in, repulsion_force};
use crate::state::{Instances, Pending, ReadCounts, Resolution, State, generation_of, resolve, runtime_id, slot_of};
use crate::streaming::{InputLog, NO_REFERENCE, Record, Residency, StreamingParameters};

/// What the oracle gives: every tick's truth and its own checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OracleRun {
    pub hashes: Vec<u64>,
    pub reads: ReadCounts,
    /// Reads whose presence disagreed with the oracle's residency, or that
    /// resolved to a slot that is not the target.
    pub check_failures: u64,
    /// Replayed records whose acceptance differed from the log's.
    pub record_mismatches: u64,
    pub dropped_pending: u64,
    pub max_remap_entries: usize,
    pub max_loaded_cells: usize,
    pub spawned_entities: u64,
    pub despawned_entities: u64,
}

impl OracleRun {
    pub fn clean(&self) -> bool {
        self.reads.stale == 0 && self.check_failures == 0 && self.record_mismatches == 0 && self.dropped_pending == 0
    }
}

/// Steps the log's records for `ticks` ticks and returns the hashes of the
/// ticks 0 to `ticks`.
pub fn run(grid: &GridConfig, parameters: &StreamingParameters, path: &Path, store: &PayloadStore, log: &InputLog, ticks: u32) -> OracleRun {
    let mut state = State::empty(path.position_at(grid, 0));
    let mut instances = Instances::new(grid.cell_count());
    let mut spatial = SpatialHash::new(grid.bins_per_side());
    // The free slots, sorted descending so that the last is the lowest.
    let mut free: Vec<usize> = Vec::new();
    // The oracle's own residency: the cells whose partition is complete.
    let mut complete = vec![false; grid.cell_count() as usize];
    let mut remap_snapshot: BTreeMap<u32, Vec<u64>>;
    let mut cursor = 0;
    let mut run = OracleRun {
        hashes: vec![state.hash()],
        reads: ReadCounts::default(),
        check_failures: 0,
        record_mismatches: 0,
        dropped_pending: 0,
        max_remap_entries: 0,
        max_loaded_cells: 0,
        spawned_entities: 0,
        despawned_entities: 0,
    };
    for tick in 1..=ticks {
        // The input fixed point of the tick: the log's records in order.
        while let Some(logged) = log.records.get(cursor) {
            if logged.tick > tick {
                break;
            }
            let accepted = match logged.record {
                Record::Load { cell, request } => instances.apply_load(tick, cell, Some(request)).is_some(),
                Record::Unload { cell } => instances.apply_unload(tick, cell),
                Record::Arrival { request, cell, content_hash } => instances.apply_arrival(tick, request, cell, store.fetch(cell), content_hash),
            };
            if accepted != logged.accepted || logged.tick != tick {
                run.record_mismatches += 1;
            }
            cursor += 1;
        }

        // The passes, over the state after the previous tick.
        let n = state.len();
        spatial.build(&state.alive, &state.x, &state.y);
        for i in 0..n {
            if state.alive[i] {
                let (fx, fy) = repulsion_force(&state.x, &state.y, &spatial, i);
                state.vx[i] = repelled_velocity(state.vx[i], fx, state.mass[i]);
                state.vy[i] = repelled_velocity(state.vy[i], fy, state.mass[i]);
            }
        }
        // The reference system reads the positions before they move, and the
        // table as the last fixed point left it.
        remap_snapshot = instances.remap.clone();
        for i in 0..n {
            if !state.alive[i] {
                continue;
            }
            let target = state.reference_cell[i];
            if target == NO_CELL {
                state.observed[i] = NO_REFERENCE;
                continue;
            }
            let resolution = resolve(&state, &remap_snapshot, target, state.reference_local[i]);
            let present = matches!(resolution, Resolution::Present(_, _));
            if present != complete[target as usize] || resolution == Resolution::Stale {
                run.check_failures += 1;
            }
            state.observed[i] = run.reads.record(resolution);
        }
        let mut energy = 0.0_f32;
        for chunk in 0..n.div_ceil(CHUNK) {
            let mut sum = 0.0_f32;
            for i in chunk_range(chunk, n) {
                if state.alive[i] {
                    let (ox, oy) = grid.origin(state.cell[i]);
                    let (x, vx) = bounced_in(integrated(state.x[i], state.vx[i]), state.vx[i], ox, ox + grid.cell_side);
                    let (y, vy) = bounced_in(integrated(state.y[i], state.vy[i]), state.vy[i], oy, oy + grid.cell_side);
                    state.x[i] = x;
                    state.y[i] = y;
                    state.vx[i] = vx;
                    state.vy[i] = vy;
                    sum += kinetic_energy(vx, vy, state.mass[i]);
                }
            }
            energy += sum;
        }
        state.energy = energy;

        // The structural fixed point in record order, the spawn sliced.
        let mut budget = parameters.spawn_slice.map_or(usize::MAX, |slice| slice as usize);
        while let Some(front) = instances.pending.front() {
            match front {
                Pending::Spawn { cell, request, next_local, .. } => {
                    let (cell, request, from) = (*cell, *request, *next_local as usize);
                    if !instances.spawn_expected(cell, request) {
                        instances.drop_pending();
                        continue;
                    }
                    if budget == 0 {
                        break;
                    }
                    let payload = instances.payloads[&cell].clone();
                    let to = (from + budget).min(payload.count as usize);
                    let mut ids = Vec::with_capacity(to - from);
                    for local in from..to {
                        let slot = free.pop().unwrap_or_else(|| state.push_slot());
                        state.write_authored(slot, cell, &payload.entity(local as u32));
                        ids.push(runtime_id(slot, state.generation[slot]));
                    }
                    budget -= to - from;
                    run.spawned_entities += (to - from) as u64;
                    let Some(Pending::Spawn { next_local, partition, .. }) = instances.pending.front_mut() else { unreachable!() };
                    partition.extend(ids);
                    *next_local = to as u32;
                    if to == payload.count as usize {
                        let Some(Pending::Spawn { partition, .. }) = instances.pending.pop_front() else { unreachable!() };
                        instances.complete_spawn(cell, partition);
                        complete[cell as usize] = true;
                    } else {
                        instances.mark_spawning(cell, request);
                        break;
                    }
                }
                Pending::Despawn { cell } => {
                    let cell = *cell;
                    if instances.residency[cell as usize] != Residency::Unloading {
                        instances.drop_pending();
                        continue;
                    }
                    let Some(partition) = instances.take_partition(cell) else {
                        instances.drop_pending();
                        continue;
                    };
                    for id in partition {
                        let slot = slot_of(id);
                        assert_eq!(state.generation[slot], generation_of(id), "the oracle's own identifiers agree");
                        state.free_slot(slot);
                        free.push(slot);
                        run.despawned_entities += 1;
                    }
                    free.sort_unstable_by(|a, b| b.cmp(a));
                    instances.pending.pop_front();
                    instances.complete_despawn(cell);
                    complete[cell as usize] = false;
                }
            }
        }
        state.point = path.position_at(grid, tick);
        run.hashes.push(state.hash());
    }
    run.dropped_pending = instances.dropped_pending;
    run.max_remap_entries = instances.max_remap_entries;
    run.max_loaded_cells = instances.max_loaded_cells;
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{World, WorldConfig};
    use reference_workload::first_divergent_tick;

    fn grid() -> GridConfig {
        GridConfig::new(3, 50, 0x5ce)
    }

    fn setup(slice: Option<u32>) -> (WorldConfig, std::sync::Arc<PayloadStore>) {
        let streaming = StreamingParameters { entering: 1.0, leaving: 1.5, spawn_slice: slice, scripted_delay_ticks: Some(3) };
        let path = Path::from_moves((1, 1), &[(1, 0), (-1, 0), (1, 0), (-1, 0)], 10);
        (WorldConfig::new(grid(), streaming, path, 3), PayloadStore::new(grid()))
    }

    #[test]
    fn the_oracle_gives_the_streamed_world_s_hashes_at_every_tick_sliced_and_unsliced() {
        for slice in [Some(20), None] {
            let (config, store) = setup(slice);
            let mut world = World::new(config.clone(), store.clone(), None);
            world.step(40).unwrap();
            let oracle = run(&config.grid, &config.streaming, &config.path, &store, world.log(), 40);
            assert_eq!(first_divergent_tick(&world.hashes(), &oracle.hashes), None, "slice {slice:?}");
            assert!(oracle.clean(), "{oracle:?}");
            assert_eq!(oracle.reads, world.reads());
            assert_eq!(oracle.max_remap_entries, world.instances().max_remap_entries);
            assert_eq!(oracle.max_loaded_cells, world.instances().max_loaded_cells);
            assert!(oracle.spawned_entities > oracle.despawned_entities && oracle.despawned_entities > 0);
        }
    }

    #[test]
    fn a_log_with_a_record_moved_gives_another_truth() {
        let (config, store) = setup(Some(20));
        let mut world = World::new(config.clone(), store.clone(), None);
        world.step(40).unwrap();
        let mut log = world.log().clone();
        let arrival = log.records.iter().position(|r| matches!(r.record, Record::Arrival { .. })).unwrap();
        log.records[arrival].tick += 2;
        log.records.sort_by_key(|r| r.tick);
        let moved = run(&config.grid, &config.streaming, &config.path, &store, &log, 40);
        assert_ne!(moved.hashes, world.hashes(), "an arrival two ticks later is another input");
    }
}
