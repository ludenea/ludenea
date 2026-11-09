//! The stepped agent of the interference measurement. Throwaway, never
//! product code.
//!
//! The toy spike's agent pattern, through the typed in-process path: the
//! agent owns the step call of its world, observes sixteen entities at every
//! batch boundary, kicks them toward the center of the world from their
//! centroid, and steps a batch of ten ticks. Its decisions are a function of
//! what it observes, so an oracle that runs the same script on its own
//! observations must reach the same hashes.

use std::time::{Duration, Instant};

use crate::world::{Command, Oracle, State, World, WorldConfig};

pub const WATCHED: u32 = 16;
pub const BATCH: u32 = 10;

/// The agent's decision from the state it observes: one impulse toward the
/// center of the world for the watched entities, from their centroid.
pub fn decide(state: &State) -> Vec<Command> {
    let present: Vec<usize> = (0..WATCHED as usize).filter(|&slot| slot < state.len() && state.alive[slot]).collect();
    if present.is_empty() {
        return Vec::new();
    }
    let (mut sx, mut sy) = (0.0_f32, 0.0_f32);
    for &slot in &present {
        sx += state.x[slot];
        sy += state.y[slot];
    }
    let n = present.len() as f32;
    let (cx, cy) = (sx / n, sy / n);
    let (delta_vx, delta_vy) = (if cx < 512.0 { 8.0 } else { -8.0 }, if cy < 512.0 { 8.0 } else { -8.0 });
    present.into_iter().map(|slot| Command::Kick { index: slot as u32, generation: state.generation[slot], delta_vx, delta_vy }).collect()
}

/// Steps `world` by `ticks` ticks in batches, the kicks decided at every
/// batch boundary as the first tick's record. Returns the wall time.
pub fn run(world: &mut World, ticks: u32) -> Duration {
    let started = Instant::now();
    while world.tick() < ticks {
        let batch = BATCH.min(ticks - world.tick()) as usize;
        let mut records = vec![Vec::new(); batch];
        records[0] = decide(world.state());
        world.step(records).expect("the agent's world steps");
    }
    started.elapsed()
}

/// The same script on the sequential oracle: the hashes of every tick.
pub fn oracle(config: &WorldConfig, ticks: u32) -> Vec<u64> {
    let mut oracle = Oracle::new(config);
    for tick in 0..ticks {
        let record = if tick % BATCH == 0 { decide(oracle.state()) } else { Vec::new() };
        oracle.step(&record);
    }
    oracle.tick_hashes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::Pool;
    use crate::world::Size;
    use reference_workload::{Parameters, first_divergent_tick, run_sequential};

    #[test]
    fn the_agent_kicks_what_it_observes_and_its_world_matches_the_oracle_of_the_same_script() {
        let config = WorldConfig::new(Size::Small, 3);
        let expected = oracle(&config, 45);
        let plain = run_sequential(&Parameters::new(Size::Small, 45, false));
        assert_eq!(first_divergent_tick(&expected, &plain.tick_hashes), Some(1), "the first kicks land at the first tick");
        let mut world = World::new(config, Pool::new(2));
        run(&mut world, 45);
        assert_eq!(world.tick(), 45);
        assert_eq!(first_divergent_tick(&world.tick_hashes(), &expected), None);
        assert_eq!(decide(world.state()).len(), WATCHED as usize);
    }
}
