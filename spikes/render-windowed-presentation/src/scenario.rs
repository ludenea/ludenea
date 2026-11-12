//! What the scenarios share: their arguments, the state the platform thread
//! publishes for the presenter (the size of the window, the last pump), the
//! phases a gesture puts the window in, and the statistics of the presenter's
//! turns per phase. Throwaway, never product code.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::clock::{Durations, now_ns};
use crate::host::WindowSpec;
use crate::json::Json;
use crate::presenter::{FrameOutcome, Mode, Presenter};

/// The arguments of a child process: `key=value` words.
pub struct Arguments(BTreeMap<String, String>);

impl Arguments {
    pub fn parse(words: &[String]) -> Arguments {
        Arguments(words.iter().filter_map(|word| word.split_once('=')).map(|(key, value)| (key.to_string(), value.to_string())).collect())
    }

    pub fn text(&self, key: &str, default: &str) -> String {
        self.0.get(key).cloned().unwrap_or_else(|| default.to_string())
    }

    pub fn number(&self, key: &str, default: u64) -> u64 {
        self.0.get(key).and_then(|value| value.parse().ok()).unwrap_or(default)
    }

    pub fn flag(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(|value| value == "yes")
    }

    pub fn mode(&self) -> Result<Mode, String> {
        Mode::parse(&self.text("mode", "unsynchronized"))
    }

    /// The window every scenario opens: small, above the other windows, and
    /// without taking the keyboard focus unless asked.
    pub fn window(&self, title: &str) -> WindowSpec {
        WindowSpec { title: title.to_string(), borderless: self.text("window", "windowed") == "borderless", width: 640, height: 360, activate: self.flag("activate"), force_x11: self.flag("force_x11") }
    }
}

/// The states a gesture puts the window in.
pub const PHASES: [&str; 5] = ["shown", "being moved", "being resized", "minimized", "covered by another window"];
pub const SHOWN: u32 = 0;
pub const MOVING: u32 = 1;
pub const RESIZING: u32 = 2;
pub const MINIMIZED: u32 = 3;
pub const COVERED: u32 = 4;

/// What the platform thread, the presenter, the gestures and the watchdog
/// share.
#[derive(Default)]
pub struct Shared {
    pub last_pump_ns: AtomicU64,
    pub last_frame_ns: AtomicU64,
    pub frames_presented: AtomicU64,
    pub phase: AtomicU32,
    pub gesture_cycles: AtomicU32,
    pub presenter_done: AtomicBool,
    pub stop: AtomicBool,
    /// A gesture the platform thread makes by a call, where no input can be
    /// injected: 1 asks for a smaller window, 2 for the first size again.
    pub call_request: AtomicU32,
}

impl Shared {
    pub fn pumped(&self) {
        self.last_pump_ns.store(now_ns(), Ordering::Relaxed);
    }
}

/// The presenter's turns in one phase.
#[derive(Default)]
struct PhaseStatistics {
    turns: u64,
    presented: u64,
    statuses: BTreeMap<String, u64>,
    acquire: Durations,
    submit: Durations,
    wait: Durations,
    present: Durations,
    offscreen_frame: Durations,
    offscreen_period: Durations,
}

/// A presenter and the record of its turns: each turn follows the window's
/// size, presents one frame to the window and serves one frame of the
/// offscreen stream.
pub struct Turns {
    pub presenter: Box<dyn Presenter>,
    phases: Vec<PhaseStatistics>,
    index: u64,
    pub presented: u64,
    pub turns: u64,
    size: (u32, u32),
    pub sizes_followed: u64,
    last_offscreen_ns: u64,
    last_phase: u32,
}

impl Turns {
    pub fn new(presenter: Box<dyn Presenter>, size: (u32, u32)) -> Turns {
        Turns { presenter, phases: PHASES.iter().map(|_| PhaseStatistics::default()).collect(), index: 0, presented: 0, turns: 0, size, sizes_followed: 0, last_offscreen_ns: 0, last_phase: u32::MAX }
    }

    pub fn turn(&mut self, shared: &Shared) -> Result<FrameOutcome, String> {
        let size = crate::host::live_size();
        if size != self.size && size.0 != 0 && size.1 != 0 {
            self.presenter.resize(size.0, size.1)?;
            self.size = size;
            self.sizes_followed += 1;
        }
        let phase = shared.phase.load(Ordering::Relaxed).min(PHASES.len() as u32 - 1);
        let outcome = self.presenter.frame(self.index)?;
        self.index += 1;
        self.turns += 1;
        let statistics = &mut self.phases[phase as usize];
        statistics.turns += 1;
        *statistics.statuses.entry(outcome.status.clone()).or_default() += 1;
        statistics.acquire.add(outcome.acquire_ns);
        if outcome.presented {
            statistics.presented += 1;
            statistics.submit.add(outcome.submit_ns);
            statistics.wait.add(outcome.wait_ns);
            statistics.present.add(outcome.present_ns);
            self.presented += 1;
            shared.frames_presented.store(self.presented, Ordering::Relaxed);
            shared.last_frame_ns.store(now_ns(), Ordering::Relaxed);
        }
        let before_offscreen = now_ns();
        self.presenter.offscreen(self.index)?;
        let completed = now_ns();
        self.phases[phase as usize].offscreen_frame.add(completed - before_offscreen);
        // The period of the offscreen stream is counted between two of its
        // frames of the same phase.
        if self.last_phase == phase && self.last_offscreen_ns != 0 {
            self.phases[phase as usize].offscreen_period.add(completed - self.last_offscreen_ns);
        }
        self.last_offscreen_ns = completed;
        self.last_phase = phase;
        Ok(outcome)
    }

    pub fn phases_json(&self) -> Json {
        Json::List(
            self.phases
                .iter()
                .enumerate()
                .filter(|(_, statistics)| statistics.turns > 0)
                .map(|(index, statistics)| {
                    Json::object()
                        .with("window", PHASES[index])
                        .with("turns", statistics.turns)
                        .with("presented", statistics.presented)
                        .with("outcomes", Json::List(statistics.statuses.iter().map(|(status, count)| Json::object().with("outcome", status.clone()).with("turns", *count)).collect()))
                        .with("acquisition", statistics.acquire.summary())
                        .with("submission", statistics.submit.summary())
                        .with("wait_for_the_device", statistics.wait.summary())
                        .with("presentation", statistics.present.summary())
                        .with("offscreen_stream_frame", statistics.offscreen_frame.summary())
                        .with("offscreen_stream_period", statistics.offscreen_period.summary())
                })
                .collect(),
        )
    }
}

/// A presenter handed to the thread that uses it.
pub struct SendPresenter(pub Box<dyn Presenter>);

// SAFETY: a presenter is used by one thread at a time, which the scenarios
// arrange with a lock or by moving it. The objects it holds are either
// free-threaded (Direct3D 12, wgpu) or externally synchronized (Vulkan), and
// none is bound to the thread that created it.
unsafe impl Send for SendPresenter {}

/// Counts the events a library delivered, by name.
#[derive(Default)]
pub struct EventCounts(BTreeMap<&'static str, u64>);

impl EventCounts {
    pub fn add(&mut self, name: &'static str) {
        *self.0.entry(name).or_default() += 1;
    }

    pub fn json(&self) -> Json {
        Json::List(self.0.iter().map(|(name, count)| Json::object().with("event", *name).with("count", *count)).collect())
    }
}
