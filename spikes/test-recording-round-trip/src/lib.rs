//! Recording round trip spike. Throwaway, never product code.
//!
//! The library the tests of the standard harness and the orchestrating binary
//! share: the world (`world`), the two pool kinds (`pool`), the container and
//! its description-driven client (`container`), a recording and its replay
//! (`recording`), the run's directory and the assembly (`run`), and, here, the
//! fixture as a recipe with its snapshot as a cache, the scripted session that
//! steps a world one tick per call and keeps its input log, and the invariant
//! whose failure a test records.

pub mod container;
pub mod pool;
pub mod recording;
pub mod run;
pub mod world;

use std::sync::OnceLock;

use reference_workload::{SPEED_LIMIT, VERSION, WORLD_SIDE};

use crate::pool::{Executor, Runtime};
use crate::recording::{InputRecord, Recording, Start};
use crate::world::{Kick, SNAPSHOT_FORMAT, Size, Snapshot, State, World, WorldConfig};

/// The ticks of the fixture's preparation: the starting state of every test.
pub const PREPARATION_TICKS: u32 = 60;
/// The identity of the one source of input records, the test's own session.
pub const SESSION_SOURCE: u32 = 1;
/// The script of the fixture's preparation; a test's body uses its own.
pub const PREPARATION_SCRIPT: u32 = 0;
/// The handles the failing test watches, and how many may go stale.
pub const WATCHED: usize = 16;
pub const STALE_ALLOWED: usize = 8;
/// The workers of the runtime instance the tests of one process share.
pub const TEST_WORKERS: usize = 4;

/// The runtime instance of the process: one persistent pool, shared by every
/// test thread, as the harness host of the game testing research has it.
pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new(TEST_WORKERS))
}

/// The input record a scripted session gives for `tick`, from the state at
/// the tick boundary: one kick on the first live entity at or after a slot
/// the tick and the script choose (always applied), and one on another slot
/// whatever lives there (applied, or dropped as stale when the slot is dead).
pub fn scripted_kicks(state: &State, tick: u32, script: u32) -> Vec<Kick> {
    let n = state.len();
    let first = (tick as usize * 37 + script as usize * 101) % n;
    let live = (0..n).map(|offset| (first + offset) % n).find(|&slot| state.alive[slot]).expect("the workload never empties");
    let other = (tick as usize * 53 + script as usize * 17 + 11) % n;
    let delta = |period: u32| ((tick % period) as f32 + 1.0) * 0.25;
    vec![
        Kick { index: live as u32, generation: state.generation[live], delta_vx: delta(7), delta_vy: -delta(5) },
        Kick { index: other as u32, generation: state.generation[other], delta_vx: -delta(3), delta_vy: delta(11) },
    ]
}

/// A fixture: a recipe (the creation values and the input records of its
/// preparation) and the snapshot of the state the recipe gives, its cache.
pub struct Fixture {
    pub config: WorldConfig,
    pub preparation: Vec<InputRecord>,
    pub snapshot: Snapshot,
    pub start_hash: u64,
}

impl Fixture {
    /// Runs the recipe: a world created from the size's default values, then
    /// the preparation, one tick per call since each record reads the state.
    pub fn build(size: Size, executor: &dyn Executor) -> Fixture {
        let config = WorldConfig::new(size);
        let mut session = Session { world: World::new(config), inputs: Vec::new(), script: PREPARATION_SCRIPT };
        for _ in 0..PREPARATION_TICKS {
            session.step_one(executor);
        }
        let snapshot = session.world.snapshot().expect("the preparation does not fail");
        let start_hash = session.world.tick_hash(PREPARATION_TICKS).expect("the last tick is hashed");
        Fixture { config, preparation: session.inputs, snapshot, start_hash }
    }
}

/// The small fixture of the process, built once and restored by every test.
pub fn small_fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| Fixture::build(Size::Small, runtime().pool()))
}

/// A world stepped one tick per call by a script, with its input log.
pub struct Session {
    pub world: World,
    pub inputs: Vec<InputRecord>,
    pub script: u32,
}

impl Session {
    /// A session on a world restored from the fixture's snapshot.
    pub fn start(fixture: &Fixture, script: u32) -> Session {
        let world = World::restore(&fixture.snapshot, fixture.config).expect("the fixture's snapshot restores");
        Session { world, inputs: Vec::new(), script }
    }

    /// One tick: the record the script gives for the state at the boundary,
    /// logged, then the one-tick step.
    pub fn step_one(&mut self, executor: &dyn Executor) {
        let tick = self.world.tick() + 1;
        let kicks = scripted_kicks(self.world.state(), tick, self.script);
        self.inputs.push(InputRecord { tick, source: SESSION_SOURCE, kicks: kicks.clone() });
        self.world.step(vec![kicks], executor).expect("no scenario of the spike fails a world");
    }

    /// The recording of the session from the fixture's starting state to the
    /// current tick, in the self-contained form or in the reference form.
    pub fn recording(&self, fixture: &Fixture, reference_form: bool, test: &str, writer_cell: &str) -> Recording {
        let start = if reference_form { Start::Recipe(fixture.preparation.clone()) } else { Start::Snapshot(fixture.snapshot.bytes.clone()) };
        Recording {
            workload_version: VERSION,
            snapshot_format: SNAPSHOT_FORMAT,
            seed: fixture.config.seed,
            entity_count: fixture.config.size.entity_count() as u32,
            wander: fixture.config.wander,
            start_tick: PREPARATION_TICKS,
            start_hash: fixture.start_hash,
            end_tick: self.world.tick(),
            test: test.to_string(),
            writer_cell: writer_cell.to_string(),
            writer_profile: run::profile().to_string(),
            start,
            inputs: self.inputs.clone(),
            hashes: self.world.history(),
        }
    }
}

/// The handles of the first live entities of a state, in slot order.
pub fn watched_handles(state: &State) -> Vec<(usize, u32)> {
    (0..state.len()).filter(|&slot| state.alive[slot]).take(WATCHED).map(|slot| (slot, state.generation[slot])).collect()
}

/// How many of the handles no longer name a live entity of their generation.
pub fn stale_handles(state: &State, handles: &[(usize, u32)]) -> usize {
    handles.iter().filter(|&&(slot, generation)| !state.alive[slot] || state.generation[slot] != generation).count()
}

/// The scripted run a failing test makes: from the fixture, one tick per
/// call, with the invariant read after every tick that at most
/// [`STALE_ALLOWED`] of the handles watched at the start have gone stale. The
/// workload kills entities every tick, so the invariant fails at a tick the
/// seed decides. Returns the session at the failing tick, or at the last tick
/// when the invariant held to the end.
pub fn run_until_the_watched_group_thins(fixture: &Fixture, script: u32, max_ticks: u32, executor: &dyn Executor) -> (Session, Option<String>) {
    let mut session = Session::start(fixture, script);
    let watched = watched_handles(session.world.state());
    for _ in 0..max_ticks {
        session.step_one(executor);
        let stale = stale_handles(session.world.state(), &watched);
        if stale > STALE_ALLOWED {
            let message = format!("at tick {} {stale} of the {WATCHED} watched handles are stale, more than {STALE_ALLOWED}", session.world.tick());
            return (session, Some(message));
        }
    }
    (session, None)
}

/// An invariant over a query of every entity: each live one is inside the
/// world and under the speed limit. What the cost measurement reads between
/// one-tick steps.
pub fn every_entity_is_inside_and_under_the_speed_limit(state: &State) -> bool {
    let mut holds = true;
    for slot in 0..state.len() {
        if state.alive[slot] {
            holds &= (0.0..=WORLD_SIDE).contains(&state.x[slot])
                && (0.0..=WORLD_SIDE).contains(&state.y[slot])
                && state.vx[slot].abs() <= SPEED_LIMIT
                && state.vy[slot].abs() <= SPEED_LIMIT;
        }
    }
    holds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::StepScoped;
    use crate::recording::{Verdict, replay, replay_recording};
    use crate::world::oracle;

    fn records_of(session_inputs: &[InputRecord]) -> Vec<Vec<Kick>> {
        session_inputs.iter().map(|record| record.kicks.clone()).collect()
    }

    #[test]
    fn the_world_with_kicks_matches_the_sequential_oracle_at_every_worker_count_and_batch() {
        let fixture = Fixture::build(Size::Small, &StepScoped::new(3));
        let (session, _) = run_until_the_watched_group_thins(&fixture, 5, 40, &StepScoped::new(2));
        let mut records = records_of(&fixture.preparation);
        records.extend(records_of(&session.inputs));
        let expected = oracle(&fixture.config, &records);
        assert_eq!(fixture.start_hash, expected[PREPARATION_TICKS as usize]);
        for (tick, hash) in session.world.history() {
            assert_eq!(hash, expected[tick as usize], "tick {tick}");
        }
        let without = oracle(&fixture.config, &vec![Vec::new(); records.len()]);
        assert_ne!(without[1], expected[1], "the first kick already changes the state");
        for workers in [1, 2, 3, 8] {
            let runtime = Runtime::new(workers);
            for batch in [1, 7, records.len()] {
                let mut world = World::new(fixture.config);
                for chunk in records.chunks(batch) {
                    world.step(chunk.to_vec(), runtime.pool()).unwrap();
                }
                let hashes: Vec<u64> = world.history().into_iter().map(|(_, hash)| hash).collect();
                assert_eq!(hashes, expected, "{workers} workers, batches of {batch}");
            }
        }
    }

    #[test]
    fn a_hash_cadence_keeps_the_hashed_ticks_and_their_values() {
        let executor = StepScoped::new(2);
        let mut every = World::new(WorldConfig::new(Size::Small));
        let mut sparse = World::new(WorldConfig::new(Size::Small).hashing_every(60));
        every.step_empty(130, &executor).unwrap();
        for _ in 0..130 {
            sparse.step_empty(1, &executor).unwrap();
        }
        assert_eq!(sparse.history().iter().map(|&(tick, _)| tick).collect::<Vec<_>>(), [0, 60, 120]);
        for (tick, hash) in sparse.history() {
            assert_eq!(every.tick_hash(tick), Some(hash));
        }
        assert_eq!(sparse.tick_hash(61), None);
        assert_eq!(every.snapshot().unwrap(), sparse.snapshot().unwrap());
    }

    #[test]
    fn both_forms_round_trip_through_their_file_and_replay_verified() {
        let executor = StepScoped::new(3);
        let fixture = Fixture::build(Size::Small, &executor);
        let (session, _) = run_until_the_watched_group_thins(&fixture, 3, 30, &executor);
        for reference_form in [false, true] {
            let recording = session.recording(&fixture, reference_form, "a_test", "a_cell");
            let bytes = recording.encode();
            assert_eq!(Recording::decode(&bytes).unwrap(), recording);
            let outcome = replay(&bytes, &StepScoped::new(2));
            assert_eq!(outcome.verdict, Verdict::Verified { last_tick: PREPARATION_TICKS + 30, compared: 30 });
            assert_eq!(outcome.records_applied, 30);
        }
    }

    #[test]
    fn a_changed_run_record_diverges_at_its_tick_and_a_changed_preparation_record_is_refused_before_any_record() {
        let executor = StepScoped::new(3);
        let fixture = Fixture::build(Size::Small, &executor);
        let (session, _) = run_until_the_watched_group_thins(&fixture, 3, 30, &executor);
        let mut changed = session.recording(&fixture, false, "a_test", "a_cell");
        changed.inputs[12].kicks[0].delta_vx = -changed.inputs[12].kicks[0].delta_vx;
        let tick = changed.inputs[12].tick;
        match replay_recording(&changed, &executor).verdict {
            Verdict::Diverged { tick: reported, .. } => assert_eq!(reported, tick),
            other => panic!("expected a divergence at tick {tick}, got {other:?}"),
        }
        // A changed preparation record either changes the starting state, and
        // the replay is refused before any record of the run, or its effect
        // has died out by the starting tick (the kicked entity was killed and
        // moved no other), and the state the replay starts from is the same.
        let mut refused = 0;
        for index in 0..PREPARATION_TICKS as usize {
            let mut changed = session.recording(&fixture, true, "a_test", "a_cell");
            let Start::Recipe(preparation) = &mut changed.start else { unreachable!("the reference form") };
            preparation[index].kicks[0].delta_vx = -preparation[index].kicks[0].delta_vx;
            let outcome = replay_recording(&changed, &executor);
            match &outcome.verdict {
                Verdict::Refused { reason } => {
                    assert!(reason.contains("starting state's hash"), "{reason}");
                    assert_eq!(outcome.records_applied, 0);
                    refused += 1;
                }
                Verdict::Verified { .. } => {}
                other => panic!("preparation record {index} changed: {other:?}"),
            }
        }
        assert!(refused > PREPARATION_TICKS as usize / 2, "{refused} of the changed preparations are refused");
    }

    #[test]
    fn a_recording_without_hashes_is_never_called_verified_and_a_gap_in_the_ticks_is_refused() {
        let executor = StepScoped::new(2);
        let fixture = Fixture::build(Size::Small, &executor);
        let (session, _) = run_until_the_watched_group_thins(&fixture, 3, 10, &executor);
        let mut bare = session.recording(&fixture, true, "a_test", "a_cell");
        bare.hashes.clear();
        assert_eq!(replay_recording(&bare, &executor).verdict, Verdict::NothingCompared { last_tick: PREPARATION_TICKS + 10 });
        let mut gap = session.recording(&fixture, false, "a_test", "a_cell");
        gap.inputs[4].tick += 1;
        let outcome = replay_recording(&gap, &executor);
        assert!(matches!(outcome.verdict, Verdict::Refused { .. }));
        assert_eq!(outcome.records_applied, 0);
    }

    #[test]
    fn the_watched_group_thins_at_one_tick_whatever_the_workers() {
        let fixture = Fixture::build(Size::Small, &StepScoped::new(1));
        let ticks: Vec<u32> = [1, 2, 4]
            .iter()
            .map(|&workers| {
                let (session, message) = run_until_the_watched_group_thins(&fixture, 3, 2_000, &StepScoped::new(workers));
                assert!(message.is_some(), "the invariant fails within the bound");
                session.world.tick()
            })
            .collect();
        assert!(ticks.iter().all(|&tick| tick == ticks[0]), "{ticks:?}");
        assert!(every_entity_is_inside_and_under_the_speed_limit(World::new(fixture.config).state()));
    }
}
