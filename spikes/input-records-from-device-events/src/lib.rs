//! Input records from device events spike: the model. Throwaway, never
//! product code.
//!
//! Three parts, as the research under test separates them:
//!
//! - device events with the host's timestamps (`DeviceEvent`);
//! - a mapping layer outside the simulation (`Mapping`) that turns the events
//!   gathered since the last tick into one action record (`Record`);
//! - the simulation's side (`Domain`): one player slot that applies at most
//!   one record per tick and derives what gameplay reads from it.
//!
//! A record carries held values and cumulative wrapping counters, so that the
//! same record applied twice has no second effect.

use reference_workload::Fnv1a;

/// Buttons of the action set. Actions 0 to 5 belong to the first context,
/// actions 6 and 7 to the second.
pub const BUTTONS: usize = 8;
pub const CONTEXT_ON_FOOT: u8 = 0b01;
pub const CONTEXT_MENU: u8 = 0b10;
/// A record whose press or release count moved by more than this since the
/// previous applied record is rejected.
pub const COUNT_BOUND: u8 = 32;
/// The same for the relative motion accumulator, per axis.
pub const MOTION_BOUND: i32 = 1 << 20;
/// One tick of the simulation, in nanoseconds of the host's clock.
pub const TICK_NS: u64 = 16_000_000;
pub const STEP: f32 = 0.016;

pub fn context_of(action: usize) -> u8 {
    if action < 6 { CONTEXT_ON_FOOT } else { CONTEXT_MENU }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventKind {
    KeyDown(u8),
    KeyUp(u8),
    Stick { x: f32, y: f32 },
    MouseMotion { dx: i32, dy: i32 },
    FocusLost,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceEvent {
    pub time_ns: u64,
    pub kind: EventKind,
}

/// The action record of one player slot for one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Record {
    pub epoch: u64,
    pub held: u8,
    pub presses: [u8; BUTTONS],
    pub releases: [u8; BUTTONS],
    pub move_x: f32,
    pub move_y: f32,
    pub look_x: i32,
    pub look_y: i32,
    pub aim_yaw: f32,
    pub aim_pitch: f32,
}

impl Record {
    pub fn neutral() -> Record {
        Record {
            epoch: 0,
            held: 0,
            presses: [0; BUTTONS],
            releases: [0; BUTTONS],
            move_x: 0.0,
            move_y: 0.0,
            look_x: 0,
            look_y: 0,
            aim_yaw: 0.0,
            aim_pitch: 0.0,
        }
    }

    /// The canonical bytes: fields in declaration order, little endian.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(RECORD_BYTES);
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.push(self.held);
        bytes.extend_from_slice(&self.presses);
        bytes.extend_from_slice(&self.releases);
        bytes.extend_from_slice(&self.move_x.to_bits().to_le_bytes());
        bytes.extend_from_slice(&self.move_y.to_bits().to_le_bytes());
        bytes.extend_from_slice(&self.look_x.to_le_bytes());
        bytes.extend_from_slice(&self.look_y.to_le_bytes());
        bytes.extend_from_slice(&self.aim_yaw.to_bits().to_le_bytes());
        bytes.extend_from_slice(&self.aim_pitch.to_bits().to_le_bytes());
        bytes
    }

    fn is_finite(&self) -> bool {
        self.move_x.is_finite() && self.move_y.is_finite() && self.aim_yaw.is_finite() && self.aim_pitch.is_finite()
    }
}

/// Bytes of a record with an epoch of 8 bytes.
pub const RECORD_BYTES: usize = 8 + 1 + BUTTONS + BUTTONS + 4 + 4 + 4 + 4 + 4 + 4;

/// The mapping layer of one writer: bindings, shaping and the cumulative
/// values. Outside the simulation.
#[derive(Clone, Debug)]
pub struct Mapping {
    /// The key bound to each action.
    pub binding: [u8; BUTTONS],
    pub dead_zone: f32,
    pub sensitivity: f32,
    held: u8,
    presses: [u8; BUTTONS],
    releases: [u8; BUTTONS],
    move_x: f32,
    move_y: f32,
    look_x: i32,
    look_y: i32,
    aim_yaw: f32,
    aim_pitch: f32,
}

impl Mapping {
    pub fn new(binding: [u8; BUTTONS]) -> Mapping {
        Mapping {
            binding,
            dead_zone: 0.2,
            sensitivity: 1.0 / 4096.0,
            held: 0,
            presses: [0; BUTTONS],
            releases: [0; BUTTONS],
            move_x: 0.0,
            move_y: 0.0,
            look_x: 0,
            look_y: 0,
            aim_yaw: 0.0,
            aim_pitch: 0.0,
        }
    }

    fn action_of(&self, key: u8) -> Option<usize> {
        self.binding.iter().position(|bound| *bound == key)
    }

    pub fn feed(&mut self, event: &DeviceEvent) {
        match event.kind {
            EventKind::KeyDown(key) => {
                if let Some(action) = self.action_of(key) {
                    self.held |= 1 << action;
                    self.presses[action] = self.presses[action].wrapping_add(1);
                }
            }
            EventKind::KeyUp(key) => {
                if let Some(action) = self.action_of(key) {
                    self.held &= !(1 << action);
                    self.releases[action] = self.releases[action].wrapping_add(1);
                }
            }
            EventKind::Stick { x, y } => {
                let length = (x * x + y * y).sqrt();
                if length <= self.dead_zone {
                    self.move_x = 0.0;
                    self.move_y = 0.0;
                } else {
                    let shaped = ((length - self.dead_zone) / (1.0 - self.dead_zone)).min(1.0);
                    self.move_x = x / length * shaped;
                    self.move_y = y / length * shaped;
                }
            }
            EventKind::MouseMotion { dx, dy } => {
                self.look_x = self.look_x.wrapping_add(dx);
                self.look_y = self.look_y.wrapping_add(dy);
                self.aim_yaw = (self.aim_yaw + dx as f32 * self.sensitivity).rem_euclid(1.0);
                self.aim_pitch = (self.aim_pitch + dy as f32 * self.sensitivity).clamp(-0.25, 0.25);
            }
            // A forced release: the held parts drop and no release is counted,
            // so that an action that fires on release does not fire.
            EventKind::FocusLost => {
                self.held = 0;
                self.move_x = 0.0;
                self.move_y = 0.0;
            }
        }
    }

    pub fn record(&self, epoch: u64) -> Record {
        Record {
            epoch,
            held: self.held,
            presses: self.presses,
            releases: self.releases,
            move_x: self.move_x,
            move_y: self.move_y,
            look_x: self.look_x,
            look_y: self.look_y,
            aim_yaw: self.aim_yaw,
            aim_pitch: self.aim_pitch,
        }
    }
}

/// What enters the simulation for the slot at one tick, in the order applied:
/// the unbind, the bind, then the record.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TickInputs {
    pub unbind: bool,
    /// A bind sets the slot's epoch and carries the writer's values at that
    /// moment as the baseline.
    pub bind: Option<Record>,
    pub record: Option<Record>,
    /// A change of the slot's active contexts, written by the game.
    pub contexts: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Applied,
    /// No record this tick: the previous applied record is applied again.
    Missing,
    RejectedEpoch,
    RejectedNotFinite,
    RejectedBound,
}

/// The simulation's side: one player slot and what a toy gameplay derives
/// from it. Everything here is state: it is hashed, snapshotted and restored.
#[derive(Clone, Debug, PartialEq)]
pub struct Domain {
    pub tick: u64,
    pub epoch: Option<u64>,
    pub applied: Record,
    pub active_contexts: u8,
    pub presses_seen: [u64; BUTTONS],
    pub releases_seen: [u64; BUTTONS],
    pub held_ticks: [u64; BUTTONS],
    pub look_seen: (i64, i64),
    pub position: (f32, f32),
    pub rejections: u64,
}

impl Default for Domain {
    fn default() -> Domain {
        Domain::new()
    }
}

impl Domain {
    pub fn new() -> Domain {
        Domain {
            tick: 0,
            epoch: None,
            applied: Record::neutral(),
            active_contexts: CONTEXT_ON_FOOT | CONTEXT_MENU,
            presses_seen: [0; BUTTONS],
            releases_seen: [0; BUTTONS],
            held_ticks: [0; BUTTONS],
            look_seen: (0, 0),
            position: (0.0, 0.0),
            rejections: 0,
        }
    }

    /// Applies one tick's inputs and steps the toy gameplay.
    pub fn step(&mut self, inputs: &TickInputs) -> Verdict {
        if let Some(contexts) = inputs.contexts {
            self.active_contexts = contexts;
        }
        if inputs.unbind {
            self.epoch = None;
            self.applied.held = 0;
            self.applied.move_x = 0.0;
            self.applied.move_y = 0.0;
        }
        if let Some(baseline) = inputs.bind {
            self.epoch = Some(baseline.epoch);
            let held = self.applied.held;
            self.applied = baseline;
            // The baseline gives the counters; the held parts start released
            // until the first record says otherwise.
            self.applied.held = held & baseline.held;
        }
        let mut presses = [0u8; BUTTONS];
        let mut releases = [0u8; BUTTONS];
        let mut look = (0i32, 0i32);
        let verdict = match inputs.record {
            None => Verdict::Missing,
            Some(record) if Some(record.epoch) != self.epoch => Verdict::RejectedEpoch,
            Some(record) if !record.is_finite() => Verdict::RejectedNotFinite,
            Some(record) => {
                let mut within = true;
                for action in 0..BUTTONS {
                    presses[action] = record.presses[action].wrapping_sub(self.applied.presses[action]);
                    releases[action] = record.releases[action].wrapping_sub(self.applied.releases[action]);
                    within &= presses[action] <= COUNT_BOUND && releases[action] <= COUNT_BOUND;
                }
                look = (record.look_x.wrapping_sub(self.applied.look_x), record.look_y.wrapping_sub(self.applied.look_y));
                within &= look.0.abs() <= MOTION_BOUND && look.1.abs() <= MOTION_BOUND;
                if within {
                    self.applied = record;
                    Verdict::Applied
                } else {
                    presses = [0; BUTTONS];
                    releases = [0; BUTTONS];
                    look = (0, 0);
                    Verdict::RejectedBound
                }
            }
        };
        if !matches!(verdict, Verdict::Applied | Verdict::Missing) {
            self.rejections += 1;
        }
        // Gameplay reads actions of the active contexts only; the others are
        // neutral at application, whoever wrote them.
        for action in 0..BUTTONS {
            if context_of(action) & self.active_contexts == 0 {
                continue;
            }
            self.presses_seen[action] += u64::from(presses[action]);
            self.releases_seen[action] += u64::from(releases[action]);
            if self.applied.held & (1 << action) != 0 {
                self.held_ticks[action] += 1;
            }
        }
        self.look_seen.0 += i64::from(look.0);
        self.look_seen.1 += i64::from(look.1);
        self.position.0 += self.applied.move_x * STEP;
        self.position.1 += self.applied.move_y * STEP;
        self.tick += 1;
        verdict
    }

    pub fn hash(&self) -> u64 {
        let mut hash = Fnv1a::new();
        hash.write(&self.tick.to_le_bytes());
        hash.write(&self.epoch.unwrap_or(u64::MAX).to_le_bytes());
        hash.write(&[u8::from(self.epoch.is_some()), self.active_contexts]);
        hash.write(&self.applied.to_bytes());
        for action in 0..BUTTONS {
            hash.write(&self.presses_seen[action].to_le_bytes());
            hash.write(&self.releases_seen[action].to_le_bytes());
            hash.write(&self.held_ticks[action].to_le_bytes());
        }
        hash.write(&self.look_seen.0.to_le_bytes());
        hash.write(&self.look_seen.1.to_le_bytes());
        hash.write(&self.position.0.to_bits().to_le_bytes());
        hash.write(&self.position.1.to_bits().to_le_bytes());
        hash.write(&self.rejections.to_le_bytes());
        hash.finish()
    }
}

/// How the host's frames fall against the ticks.
#[derive(Clone, Copy, Debug)]
pub enum Frames {
    /// One frame every `period_ns`.
    Fixed { period_ns: u64 },
    /// Frames of irregular length between a quarter of a tick and three
    /// ticks, from a seed, so that some frames run no tick and some several.
    Irregular { seed: u64 },
}

/// One press as the run placed it: the tick during which the key went down
/// and the tick whose record first counted it.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub happened_in_tick: u64,
    pub applied_in_tick: u64,
}

pub struct HostRun {
    /// One record per tick, in tick order.
    pub records: Vec<Record>,
    pub placements: Vec<Placement>,
    pub frames: u64,
    pub frames_without_a_tick: u64,
    pub most_ticks_in_a_frame: u64,
}

/// Runs the host loop over the events: at each frame, zero or more ticks; for
/// each tick, the events assigned to it are fed to the mapping layer and one
/// record is taken. With timestamps, the earlier ticks of a frame take the
/// events up to their own end and the last tick takes every event up to the
/// frame's instant; without timestamps, the first tick takes them all.
pub fn run_host(events: &[DeviceEvent], frames: Frames, with_timestamps: bool, mapping: &mut Mapping, epoch: u64) -> HostRun {
    let last_event = events.last().map_or(0, |event| event.time_ns);
    let mut generator = reference_workload::SplitMix64::new(match frames {
        Frames::Irregular { seed } => seed,
        Frames::Fixed { .. } => 0,
    });
    let mut run = HostRun { records: Vec::new(), placements: Vec::new(), frames: 0, frames_without_a_tick: 0, most_ticks_in_a_frame: 0 };
    let mut now = 0u64;
    let mut next_event = 0usize;
    let mut ticks_done = 0u64;
    loop {
        now += match frames {
            Frames::Fixed { period_ns } => period_ns,
            Frames::Irregular { .. } => TICK_NS / 4 + generator.next() % (TICK_NS * 11 / 4),
        };
        run.frames += 1;
        let due = now / TICK_NS - ticks_done;
        if due == 0 {
            run.frames_without_a_tick += 1;
        }
        run.most_ticks_in_a_frame = run.most_ticks_in_a_frame.max(due);
        for index in 0..due {
            let tick = ticks_done + index;
            let limit = if !with_timestamps {
                if index == 0 { now } else { 0 }
            } else if index + 1 == due {
                now
            } else {
                (tick + 1) * TICK_NS
            };
            while next_event < events.len() && events[next_event].time_ns <= limit {
                let event = &events[next_event];
                if matches!(event.kind, EventKind::KeyDown(key) if mapping.action_of(key).is_some()) {
                    run.placements.push(Placement { happened_in_tick: event.time_ns / TICK_NS, applied_in_tick: tick });
                }
                mapping.feed(event);
                next_event += 1;
            }
            run.records.push(mapping.record(epoch));
        }
        ticks_done += due;
        if next_event == events.len() && now > last_event && due > 0 {
            return run;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time_ns: u64, down: bool, key: u8) -> DeviceEvent {
        DeviceEvent { time_ns, kind: if down { EventKind::KeyDown(key) } else { EventKind::KeyUp(key) } }
    }

    #[test]
    fn a_tap_shorter_than_a_tick_is_one_press_and_one_release() {
        let events = [key(1_000_000, true, 10), key(3_000_000, false, 10)];
        let mut mapping = Mapping::new([10, 11, 12, 13, 14, 15, 16, 17]);
        let baseline = mapping.record(7);
        let run = run_host(&events, Frames::Fixed { period_ns: TICK_NS }, true, &mut mapping, 7);
        let mut domain = Domain::new();
        domain.step(&TickInputs { bind: Some(baseline), ..TickInputs::default() });
        for record in &run.records {
            assert_eq!(domain.step(&TickInputs { record: Some(*record), ..TickInputs::default() }), Verdict::Applied);
        }
        assert_eq!(domain.presses_seen[0], 1);
        assert_eq!(domain.releases_seen[0], 1);
        assert_eq!(domain.held_ticks[0], 0);
    }

    #[test]
    fn the_same_record_applied_twice_presses_once() {
        let mut mapping = Mapping::new([10, 11, 12, 13, 14, 15, 16, 17]);
        let baseline = mapping.record(7);
        mapping.feed(&key(1, true, 10));
        let record = mapping.record(7);
        let mut domain = Domain::new();
        domain.step(&TickInputs { bind: Some(baseline), ..TickInputs::default() });
        domain.step(&TickInputs { record: Some(record), ..TickInputs::default() });
        domain.step(&TickInputs { record: Some(record), ..TickInputs::default() });
        domain.step(&TickInputs::default());
        assert_eq!(domain.presses_seen[0], 1);
        assert_eq!(domain.held_ticks[0], 3);
    }

    #[test]
    fn a_record_of_another_epoch_is_rejected() {
        let mapping = Mapping::new([10, 11, 12, 13, 14, 15, 16, 17]);
        let mut domain = Domain::new();
        domain.step(&TickInputs { bind: Some(mapping.record(7)), ..TickInputs::default() });
        assert_eq!(domain.step(&TickInputs { record: Some(mapping.record(8)), ..TickInputs::default() }), Verdict::RejectedEpoch);
    }
}
