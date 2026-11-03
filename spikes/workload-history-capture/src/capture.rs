//! The history capture. Throwaway, never product code.
//!
//! What the core would carry always on: the per-tick input log (the record of
//! kicks of every tick, as canonical bytes), the per-tick hash, the event log
//! (the wall bounces of every tick, merged in chunk order by the world) and a
//! snapshot every `cadence` ticks, kept for a retention window. Nothing else:
//! no per-write log, no per-system history. A causal query restores the
//! nearest snapshot before the tick asked about into a harness world that
//! watches the entity, replays the input log to that tick, and answers with
//! the chain the instrumentation recorded: the kicks that reached the entity,
//! the neighbors that repelled it, the systems that changed each of its
//! components and the walls it bounced on, tick by tick. A tick older than
//! the retained snapshots is a structured error, never a guess.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use crate::world::{BounceEvent, Kick, Row, Snapshot, StepOutput, Trace, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureConfig {
    /// A snapshot every `cadence` ticks; 0 takes none.
    pub cadence: u32,
    /// Snapshots older than `retention_ticks` before the last tick are
    /// dropped, with the records before the oldest kept snapshot; 0 keeps all.
    pub retention_ticks: u32,
}

/// One tick's always-on capture.
#[derive(Clone, Debug, PartialEq)]
pub struct TickRecord {
    pub tick: u32,
    pub kicks: Vec<Kick>,
    pub hash: u64,
    pub events: Vec<BounceEvent>,
}

impl TickRecord {
    /// The bytes of the input record: the tick, a count and 16 bytes per kick.
    pub fn log_bytes(&self) -> usize {
        4 + 4 + 16 * self.kicks.len()
    }

    /// The bytes of the event record: a count and 4 bytes per event.
    pub fn event_bytes(&self) -> usize {
        4 + 4 * self.events.len()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureTiming {
    pub log: Duration,
    pub events: Duration,
    pub snapshots: Duration,
    pub snapshot_count: u64,
}

impl CaptureTiming {
    pub fn total(&self) -> Duration {
        self.log + self.events + self.snapshots
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureError {
    pub code: &'static str,
    pub message: String,
    pub context: Vec<(&'static str, String)>,
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        for (key, value) in &self.context {
            write!(f, "; {key}={value}")?;
        }
        Ok(())
    }
}

pub struct Capture {
    pub config: CaptureConfig,
    records: VecDeque<TickRecord>,
    snapshots: VecDeque<(u32, Snapshot)>,
    pub timing: CaptureTiming,
    /// The log and event bytes ever written, retained or not.
    pub log_bytes_written: usize,
    pub event_bytes_written: usize,
    pub events_written: u64,
    last_tick: u32,
}

impl Capture {
    pub fn new(config: CaptureConfig) -> Capture {
        Capture {
            config,
            records: VecDeque::new(),
            snapshots: VecDeque::new(),
            timing: CaptureTiming::default(),
            log_bytes_written: 0,
            event_bytes_written: 0,
            events_written: 0,
            last_tick: 0,
        }
    }

    /// Records the ticks of a step that took `records` as inputs and gave
    /// `output`, then snapshots the world when its tick is on the cadence and
    /// trims to the retention window. Called between steps, with the world at
    /// the tick boundary the step reached.
    pub fn after_step(&mut self, world: &World, records: &[Vec<Kick>], output: &StepOutput) {
        let to = world.tick();
        let from = to - records.len() as u32;
        let logging = Instant::now();
        for (offset, kicks) in records.iter().enumerate() {
            let tick = from + offset as u32 + 1;
            let record = TickRecord { tick, kicks: kicks.clone(), hash: world.tick_hash(tick).expect("a completed tick has a hash"), events: Vec::new() };
            self.log_bytes_written += record.log_bytes();
            self.records.push_back(record);
        }
        self.timing.log += logging.elapsed();
        let eventing = Instant::now();
        let first = self.records.len() - records.len();
        for (offset, events) in output.events.iter().enumerate() {
            let record = &mut self.records[first + offset];
            record.events = events.clone();
            self.event_bytes_written += record.event_bytes();
            self.events_written += events.len() as u64;
        }
        self.timing.events += eventing.elapsed();
        self.last_tick = to;
        if self.config.cadence > 0 && to.is_multiple_of(self.config.cadence) {
            let taking = Instant::now();
            let snapshot = world.snapshot().expect("a healthy world snapshots");
            self.timing.snapshots += taking.elapsed();
            self.timing.snapshot_count += 1;
            self.snapshots.push_back((to, snapshot));
        }
        self.trim();
    }

    /// Drops the snapshots older than the window, then the records nobody can
    /// replay from (before the oldest snapshot kept, or all of them when no
    /// snapshot is ever taken and a window applies).
    fn trim(&mut self) {
        if self.config.retention_ticks == 0 {
            return;
        }
        let oldest_kept = self.last_tick.saturating_sub(self.config.retention_ticks);
        while self.snapshots.len() > 1 && self.snapshots[0].0 < oldest_kept {
            self.snapshots.pop_front();
        }
        let floor = self.snapshots.front().map_or(oldest_kept, |(tick, _)| *tick);
        while self.records.front().is_some_and(|r| r.tick <= floor) {
            self.records.pop_front();
        }
    }

    pub fn last_tick(&self) -> u32 {
        self.last_tick
    }

    pub fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    pub fn snapshot_ticks(&self) -> Vec<u32> {
        self.snapshots.iter().map(|(tick, _)| *tick).collect()
    }

    /// The oldest tick a causal query can answer: the oldest kept snapshot's.
    pub fn oldest_answerable_tick(&self) -> Option<u32> {
        self.snapshots.front().map(|(tick, _)| *tick)
    }

    #[cfg(test)]
    pub fn records_retained(&self) -> usize {
        self.records.len()
    }

    /// The bytes retained now: snapshots, the input log, the hashes, the events.
    pub fn memory(&self) -> Memory {
        Memory {
            snapshots: self.snapshots.len(),
            snapshot_bytes: self.snapshots.iter().map(|(_, s)| s.bytes.len()).sum(),
            log_bytes: self.records.iter().map(TickRecord::log_bytes).sum(),
            hash_bytes: 12 * self.records.len(),
            event_bytes: self.records.iter().map(TickRecord::event_bytes).sum(),
        }
    }

    /// "Which inputs led here": the records between two ticks, a read.
    pub fn inputs_between(&self, from: u32, to: u32) -> Result<Vec<(u32, Vec<Kick>)>, CaptureError> {
        if from > to || to > self.last_tick {
            return Err(CaptureError { code: "invalid_range", message: format!("ticks {from} to {to} are not a range of the capture"), context: vec![("last_tick", self.last_tick.to_string())] });
        }
        let oldest = self.records.front().map_or(self.last_tick + 1, |r| r.tick);
        if from < oldest {
            return Err(self.outside_retention(from));
        }
        Ok(self.records.iter().filter(|r| r.tick >= from && r.tick <= to).map(|r| (r.tick, r.kicks.clone())).collect())
    }

    fn outside_retention(&self, tick: u32) -> CaptureError {
        CaptureError {
            code: "outside_retention",
            message: format!("tick {tick} is older than the retained history"),
            context: vec![
                ("requested_tick", tick.to_string()),
                ("oldest_answerable_tick", self.oldest_answerable_tick().map_or("none".to_string(), |t| t.to_string())),
                ("last_tick", self.last_tick.to_string()),
                ("retention_ticks", self.config.retention_ticks.to_string()),
            ],
        }
    }

    /// The snapshot to replay from for a query at `tick` (the nearest one at
    /// or before it) and the records up to the tick, cloned so that the
    /// replay runs outside any lock on the capture.
    pub fn plan(&self, tick: u32) -> Result<ReplayPlan, CaptureError> {
        if tick > self.last_tick {
            return Err(CaptureError { code: "in_the_future", message: format!("tick {tick} has not happened"), context: vec![("last_tick", self.last_tick.to_string())] });
        }
        let Some((snapshot_tick, _)) = self.snapshots.iter().rev().find(|(t, _)| *t <= tick) else {
            return Err(self.outside_retention(tick));
        };
        self.plan_from(*snapshot_tick, tick)
    }

    /// A replay from the snapshot taken at `snapshot_tick` to `tick`.
    pub fn plan_from(&self, snapshot_tick: u32, tick: u32) -> Result<ReplayPlan, CaptureError> {
        let Some((_, snapshot)) = self.snapshots.iter().find(|(t, _)| *t == snapshot_tick) else {
            return Err(CaptureError { code: "no_snapshot", message: format!("no snapshot at tick {snapshot_tick}"), context: vec![("snapshot_ticks", format!("{:?}", self.snapshot_ticks()))] });
        };
        if tick < snapshot_tick {
            return Err(CaptureError { code: "invalid_range", message: format!("tick {tick} is before the snapshot at {snapshot_tick}"), context: vec![] });
        }
        let records: Vec<TickRecord> = self.records.iter().filter(|r| r.tick > snapshot_tick && r.tick <= tick).cloned().collect();
        if records.len() as u32 != tick - snapshot_tick {
            return Err(CaptureError {
                code: "log_incomplete",
                message: format!("{} records retained for ticks {} to {tick}", records.len(), snapshot_tick + 1),
                context: vec![("snapshot_tick", snapshot_tick.to_string())],
            });
        }
        Ok(ReplayPlan { snapshot_tick, snapshot: snapshot.clone(), records })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub snapshots: usize,
    pub snapshot_bytes: usize,
    pub log_bytes: usize,
    pub hash_bytes: usize,
    pub event_bytes: usize,
}

impl Memory {
    pub fn total(&self) -> usize {
        self.snapshot_bytes + self.log_bytes + self.hash_bytes + self.event_bytes
    }
}

/// What a causal query replays: the snapshot and the records after it.
#[derive(Clone, Debug)]
pub struct ReplayPlan {
    pub snapshot_tick: u32,
    pub snapshot: Snapshot,
    pub records: Vec<TickRecord>,
}

/// What one tick did to the entity, as data: which systems changed which of
/// its components, from which inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct CausalTick {
    pub tick: u32,
    pub kicks: Vec<(f32, f32)>,
    pub neighbors: Vec<u32>,
    /// The systems that changed a component, as "system: component" pairs.
    pub changes: Vec<(&'static str, &'static str)>,
    pub bounced_x: bool,
    pub bounced_y: bool,
    pub state_after: Row,
}

impl CausalTick {
    fn from_trace(trace: &Trace) -> CausalTick {
        let mut changes = Vec::new();
        if !trace.before.alive {
            if trace.after.alive {
                changes.push(("structural_changes", "spawned"));
            }
        } else {
            let (mut vx, mut vy) = (trace.before.vx, trace.before.vy);
            for (dvx, dvy) in &trace.kicks {
                vx = crate::world::kicked_velocity(vx, *dvx);
                vy = crate::world::kicked_velocity(vy, *dvy);
            }
            if !trace.kicks.is_empty() && (vx, vy) != (trace.before.vx, trace.before.vy) {
                changes.push(("input", "velocity"));
            }
            if trace.after_repulsion != (vx, vy) {
                changes.push(("repulsion", "velocity"));
            }
            if (trace.after_motion.x, trace.after_motion.y) != (trace.before.x, trace.before.y) {
                changes.push(("motion", "position"));
            }
            if (trace.after_motion.vx, trace.after_motion.vy) != trace.after_repulsion {
                changes.push(("motion", "velocity"));
            }
            if !trace.after.alive {
                changes.push(("structural_changes", "despawned"));
            } else if trace.after.generation != trace.before.generation {
                changes.push(("structural_changes", "respawned"));
            }
        }
        CausalTick {
            tick: trace.tick,
            kicks: trace.kicks.clone(),
            neighbors: trace.neighbors.clone(),
            changes,
            bounced_x: trace.bounced_x,
            bounced_y: trace.bounced_y,
            state_after: trace.after,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CausalAnswer {
    pub index: u32,
    pub tick: u32,
    pub snapshot_tick: u32,
    pub replayed_ticks: u32,
    /// Whether the replayed hash at the tick equals the recorded one: the
    /// divergence detection the capture gives for free.
    pub hash_matches: bool,
    pub chain: Vec<CausalTick>,
    pub restore: Duration,
    pub replay: Duration,
    pub total: Duration,
}

impl CausalAnswer {
    pub fn kicks(&self) -> usize {
        self.chain.iter().map(|t| t.kicks.len()).sum()
    }

    pub fn neighbor_ticks(&self) -> usize {
        self.chain.iter().filter(|t| !t.neighbors.is_empty()).count()
    }

    pub fn changes(&self) -> usize {
        self.chain.iter().map(|t| t.changes.len()).sum()
    }
}

/// Answers "why did the entity at `index` reach its state at `tick`" from a
/// plan: a harness world restored from the snapshot, watching the entity,
/// replays the records to the tick; the chain is what it recorded.
pub fn causal_query(plan: &ReplayPlan, index: u32, tick: u32, workers: usize) -> Result<CausalAnswer, CaptureError> {
    let started = Instant::now();
    let restoring = Instant::now();
    let mut harness = World::restore(&plan.snapshot, workers, Some(index as usize))
        .map_err(|e| CaptureError { code: "snapshot_rejected", message: e.0, context: vec![("snapshot_tick", plan.snapshot_tick.to_string())] })?;
    let restore = restoring.elapsed();
    let replaying = Instant::now();
    let records: Vec<Vec<Kick>> = plan.records.iter().map(|r| r.kicks.clone()).collect();
    if !records.is_empty() {
        harness.step(records).map_err(|e| CaptureError { code: "world_failed", message: e.to_string(), context: vec![] })?;
    }
    let replay = replaying.elapsed();
    let traces = harness.take_traces();
    let recorded_hash = plan.records.last().map(|r| r.hash);
    let hash_matches = recorded_hash.is_none_or(|h| h == harness.hash());
    Ok(CausalAnswer {
        index,
        tick,
        snapshot_tick: plan.snapshot_tick,
        replayed_ticks: plan.records.len() as u32,
        hash_matches,
        chain: traces.iter().map(CausalTick::from_trace).collect(),
        restore,
        replay,
        total: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Size, WorldConfig};

    fn kicks_for(tick: u32) -> Vec<Kick> {
        if tick % 5 == 0 { (0..12).map(|i| Kick { index: i, generation: 0, delta_vx: 8.0, delta_vy: -8.0 }).collect() } else { Vec::new() }
    }

    /// A live world of 60 ticks in batches of 6, captured with the given config.
    fn captured(config: CaptureConfig) -> (World, Capture) {
        let mut world = World::new(WorldConfig::new(Size::Small, 2));
        let mut capture = Capture::new(config);
        while world.tick() < 60 {
            let records: Vec<Vec<Kick>> = (world.tick() + 1..=world.tick() + 6).map(kicks_for).collect();
            let output = world.step(records.clone()).unwrap();
            capture.after_step(&world, &records, &output);
        }
        (world, capture)
    }

    #[test]
    fn snapshots_fall_on_the_cadence_and_the_replay_from_each_reaches_the_live_hashes_and_events() {
        let (world, capture) = captured(CaptureConfig { cadence: 12, retention_ticks: 0 });
        assert_eq!(capture.snapshot_ticks(), [12, 24, 36, 48, 60]);
        assert_eq!(capture.records_retained(), 60);
        assert_eq!(capture.timing.snapshot_count, 5);
        for snapshot_tick in [12, 24, 36, 48] {
            for workers in [1, 3] {
                let plan = capture.plan_from(snapshot_tick, snapshot_tick + 12).unwrap();
                assert_eq!(plan.snapshot_tick, snapshot_tick);
                assert_eq!(plan.records.len(), 12);
                let mut replayed = World::restore(&plan.snapshot, workers, None).unwrap();
                let output = replayed.step(plan.records.iter().map(|r| r.kicks.clone()).collect()).unwrap();
                for r in &plan.records {
                    assert_eq!(replayed.tick_hash(r.tick), Some(r.hash), "tick {} from {snapshot_tick} at {workers} workers", r.tick);
                    assert_eq!(replayed.tick_hash(r.tick), world.tick_hash(r.tick));
                }
                assert_eq!(output.events, plan.records.iter().map(|r| r.events.clone()).collect::<Vec<_>>());
            }
        }
        // A plan for a tick between snapshots replays from the one before it,
        // and a plan for a snapshot's own tick replays nothing.
        assert_eq!(capture.plan(30).unwrap().snapshot_tick, 24);
        assert_eq!(capture.plan(30).unwrap().records.len(), 6);
        assert_eq!(capture.plan(24).unwrap().records.len(), 0);
        assert_eq!(capture.plan_from(13, 20).unwrap_err().code, "no_snapshot");
        assert_eq!(capture.plan(61).unwrap_err().code, "in_the_future");
        assert_eq!(capture.plan(5).unwrap_err().code, "outside_retention", "no snapshot before tick 12");
        assert!(capture.events_written > 0);
        let memory = capture.memory();
        assert_eq!(memory.snapshots, 5);
        assert_eq!(memory.log_bytes, 60 * 8 + 12 * 16 * 12);
        assert_eq!(memory.hash_bytes, 60 * 12);
        assert_eq!(memory.event_bytes, 60 * 4 + 4 * capture.events_written as usize);
    }

    #[test]
    fn the_retention_window_drops_old_snapshots_and_names_what_it_cannot_answer() {
        let (_, capture) = captured(CaptureConfig { cadence: 12, retention_ticks: 20 });
        assert_eq!(capture.snapshot_ticks(), [48, 60]);
        assert_eq!(capture.oldest_answerable_tick(), Some(48));
        assert_eq!(capture.records_retained(), 12, "the records after the oldest kept snapshot");
        let error = capture.plan(47).unwrap_err();
        assert_eq!(error.code, "outside_retention");
        assert!(error.context.contains(&("oldest_answerable_tick", "48".to_string())));
        assert!(capture.plan(48).is_ok() && capture.plan(55).is_ok());
        assert_eq!(capture.inputs_between(49, 60).unwrap().len(), 12);
        assert_eq!(capture.inputs_between(40, 60).unwrap_err().code, "outside_retention");
        let (_, none) = captured(CaptureConfig { cadence: 0, retention_ticks: 0 });
        assert_eq!(none.snapshot_count(), 0);
        assert_eq!(none.plan(10).unwrap_err().code, "outside_retention");
        assert_eq!(none.inputs_between(1, 60).unwrap().len(), 60);
    }

    #[test]
    fn a_causal_query_replays_the_chain_of_a_kicked_entity_and_detects_the_hash() {
        let (world, capture) = captured(CaptureConfig { cadence: 12, retention_ticks: 0 });
        let plan = capture.plan(30).unwrap();
        let answer = causal_query(&plan, 3, 30, 3).unwrap();
        assert_eq!((answer.snapshot_tick, answer.replayed_ticks, answer.chain.len()), (24, 6, 6));
        assert!(answer.hash_matches);
        assert_eq!(answer.chain[0].tick, 25);
        let kicked: Vec<&CausalTick> = answer.chain.iter().filter(|t| !t.kicks.is_empty()).collect();
        assert_eq!(kicked.iter().map(|t| t.tick).collect::<Vec<_>>(), [25, 30], "slot 3 is kicked at every fifth tick");
        assert!(kicked[0].changes.contains(&("input", "velocity")));
        assert!(answer.chain.iter().all(|t| t.changes.contains(&("motion", "position"))));
        // Replayed to the live world's tick, the chain ends on the live state.
        let to_the_end = causal_query(&capture.plan_from(48, 60).unwrap(), 3, 60, 2).unwrap();
        assert_eq!(to_the_end.chain.len(), 12);
        let state = world.state();
        let last = to_the_end.chain.last().unwrap().state_after;
        assert_eq!((last.x, last.y, last.vx, last.vy, last.alive, last.generation), (state.x[3], state.y[3], state.vx[3], state.vy[3], state.alive[3], state.generation[3]));
        // A query on a slot outside the world is a structured error.
        assert_eq!(causal_query(&plan, 5_000, 30, 1).unwrap_err().code, "snapshot_rejected");
        // A corrupt record makes the replayed hash differ: the divergence is detected.
        let mut corrupt = plan.clone();
        corrupt.records[0].kicks.push(Kick { index: 7, generation: 0, delta_vx: 3.0, delta_vy: 0.0 });
        assert!(!causal_query(&corrupt, 3, 30, 1).unwrap().hash_matches);
    }
}
