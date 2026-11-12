//! The sequential oracle of the asset table spike. Throwaway, never product
//! code.
//!
//! The reference implementation's systems in the specification's order, the
//! repulsion of every slot reading the parameters of the asset the slot
//! names. The oracle knows nothing of wants, batches or gates: a scenario
//! hands it the schedule it expects (which asset reads which parameters from
//! which tick, none meaning the declared default), written by hand from the
//! rules under test, so that a world's hashes are compared with a statement
//! of what the rules should give and not with a second run of the same code.

use reference_workload::{Grid, KILLS_PER_TICK, Parameters, State, repelled_velocity};

use crate::table::RepulsionTable;
use crate::world::repulsion_force;

/// From `tick` on, the slots that name the asset of index `asset` in the
/// pool read `table`, or the declared default where it is none.
#[derive(Clone, Copy, Debug)]
pub struct Switch {
    pub tick: u32,
    pub asset: usize,
    pub table: Option<RepulsionTable>,
}

pub fn switch(tick: u32, asset: usize, table: Option<RepulsionTable>) -> Switch {
    Switch { tick, asset, table }
}

/// The component hashes of the ticks 0 to the parameters' tick count, slot
/// `i` naming the asset of index `i % assets`, every asset unavailable until
/// its first switch.
pub fn oracle(parameters: &Parameters, assets: usize, schedule: &[Switch]) -> Vec<u64> {
    let mut state = State::initialize(parameters);
    let mut grid = Grid::new();
    let mut tables = vec![RepulsionTable::DEFAULT; assets];
    let mut hashes = Vec::with_capacity(parameters.tick_count as usize + 1);
    hashes.push(state.hash());
    for t in 1..=parameters.tick_count {
        for switch in schedule.iter().filter(|switch| switch.tick == t) {
            tables[switch.asset] = switch.table.unwrap_or(RepulsionTable::DEFAULT);
        }
        grid.build(&state);
        for i in 0..state.len() {
            if state.alive[i] {
                let (fx, fy) = repulsion_force(&state, &grid, i, tables[i % assets]);
                state.vx[i] = repelled_velocity(state.vx[i], fx, state.mass[i]);
                state.vy[i] = repelled_velocity(state.vy[i], fy, state.mass[i]);
            }
        }
        reference_workload::integration(&mut state);
        reference_workload::boundary(&mut state);
        reference_workload::energy(&mut state);
        state.structural_changes();
        hashes.push(state.hash());
    }
    hashes
}

/// The tick at which each slot is first killed by the workload's structural
/// changes, from the global stream alone: the kills never depend on a position.
pub fn first_kill_ticks(parameters: &Parameters) -> Vec<Option<u32>> {
    let mut state = State::initialize(parameters);
    let n = state.len();
    let mut first = vec![None; n];
    for t in 1..=parameters.tick_count {
        let mut peek = state.rng;
        for _ in 0..KILLS_PER_TICK {
            let slot = (peek.next() % n as u64) as usize;
            first[slot].get_or_insert(t);
        }
        state.structural_changes();
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use reference_workload::{Size, run_sequential};

    #[test]
    fn the_oracle_with_the_specification_parameters_from_the_first_tick_is_the_reference_run() {
        let parameters = Parameters::new(Size::Small, 40, false);
        let schedule: Vec<Switch> = (0..8).map(|asset| switch(1, asset, Some(RepulsionTable::SPECIFICATION))).collect();
        assert_eq!(oracle(&parameters, 8, &schedule), run_sequential(&parameters).tick_hashes);
        let late: Vec<Switch> = (0..8).map(|asset| switch(2, asset, Some(RepulsionTable::SPECIFICATION))).collect();
        assert_ne!(oracle(&parameters, 8, &late), run_sequential(&parameters).tick_hashes);
    }

    #[test]
    fn the_first_kills_are_the_ticks_at_which_slots_die() {
        let parameters = Parameters::new(Size::Small, 40, false);
        let first = first_kill_ticks(&parameters);
        let killed_at_the_first_tick = first.iter().filter(|tick| **tick == Some(1)).count();
        assert!((1..=KILLS_PER_TICK).contains(&killed_at_the_first_tick));
        assert!(first.iter().any(Option::is_none), "some slots survive forty ticks");
    }
}
