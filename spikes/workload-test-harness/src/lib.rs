//! Library of the test harness spike, for the game testing research.
//! Throwaway, never product code.
//!
//! What a test of the standard harness needs to run the reference workload
//! as a test: the settings read once from the environment (the size, where
//! to write reports, whether the failing test is armed), the shared fixture
//! (the sequential reference run and the snapshot of the world at tick 0,
//! computed once per process), the test case (two worlds restored from the
//! fixture at two worker counts, one of them carrying an injected divergence
//! or an injected panic), the comparison that steps both worlds and reports
//! the first divergent tick, the first divergent system and the two
//! snapshots of that tick, and the report a test writes as JSON for the
//! orchestrator to collect.

pub mod world;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Once, OnceLock};
use std::time::{Duration, Instant};

use reference_workload::{DEFAULT_TICK_COUNT, Json, Run, first_divergent_tick, run_hash, run_sequential};
pub use world::*;

pub const SIZE_VARIABLE: &str = "TEST_HARNESS_SPIKE_SIZE";
pub const REPORT_DIR_VARIABLE: &str = "TEST_HARNESS_SPIKE_REPORT_DIR";
pub const PANIC_VARIABLE: &str = "TEST_HARNESS_SPIKE_PANIC";
/// The run every injection tick is stated for; the ticks scale to shorter runs.
pub const STATED_TICKS: u32 = DEFAULT_TICK_COUNT;
const SMALL_TICKS: u32 = 40;
/// The batch a comparison without an injection steps by; a comparison with
/// an injection steps one tick at a time so that it stands exactly at the
/// first divergent tick when it sees it.
pub const BATCH_WITHOUT_INJECTION: u32 = 10;

/// The settings of a test process, read once from the environment.
pub struct Settings {
    pub size: Size,
    pub ticks: u32,
    pub report_dir: Option<PathBuf>,
    pub panic_armed: bool,
}

pub fn settings() -> &'static Settings {
    static SETTINGS: OnceLock<Settings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        let (size, ticks) = match env::var(SIZE_VARIABLE).as_deref() {
            Ok("default") => (Size::Default, DEFAULT_TICK_COUNT),
            Ok("large") => (Size::Large, DEFAULT_TICK_COUNT),
            _ => (Size::Small, SMALL_TICKS),
        };
        Settings {
            size,
            ticks,
            report_dir: env::var_os(REPORT_DIR_VARIABLE).map(PathBuf::from),
            panic_armed: env::var(PANIC_VARIABLE).as_deref() == Ok("1"),
        }
    })
}

/// A tick stated for the 1 000 tick run, scaled to a run of `ticks` ticks.
pub fn scaled_tick(stated: u32, ticks: u32) -> u32 {
    ((u64::from(stated) * u64::from(ticks) / u64::from(STATED_TICKS)) as u32).clamp(1, ticks)
}

/// What every test of a process shares: the sequential reference of the run
/// and the fixture, the snapshot of the world at tick 0, plus the time both
/// took to build.
pub struct Shared {
    pub reference: Run,
    pub fixture: Snapshot,
    pub setup: Duration,
}

pub fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| {
        let settings = settings();
        let started = Instant::now();
        let reference = run_sequential(&Parameters::new(settings.size, settings.ticks, false));
        let fixture = World::new(WorldConfig::new(settings.size, 1), Capabilities::default()).snapshot().expect("a new world snapshots");
        Shared { reference, fixture, setup: started.elapsed() }
    })
}

/// The presentation capability of this spike: the runtime skeleton spike's
/// stub extraction node, registered by the fixture when the feature is on.
#[cfg(feature = "presentation")]
pub mod presentation {
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::world::{Extraction, State};

    #[derive(Default)]
    pub struct StubExtraction {
        extracted_ticks: AtomicU64,
    }

    impl StubExtraction {
        pub fn extracted_ticks(&self) -> u64 {
            self.extracted_ticks.load(Ordering::Relaxed)
        }
    }

    impl Extraction for StubExtraction {
        fn extract(&self, _tick: u32, state: &State) {
            let _alive = state.alive.iter().filter(|&&alive| alive).count();
            self.extracted_ticks.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub const PRESENTATION: bool = cfg!(feature = "presentation");

/// The capabilities this build registers at world creation, and a reader of
/// the extracted tick count when the build has the presentation stub.
fn capabilities() -> (Capabilities, Option<Arc<dyn Fn() -> u64 + Send + Sync>>) {
    #[cfg(feature = "presentation")]
    {
        let stub = Arc::new(presentation::StubExtraction::default());
        let reader = stub.clone();
        (Capabilities { extraction: Some(stub) }, Some(Arc::new(move || reader.extracted_ticks())))
    }
    #[cfg(not(feature = "presentation"))]
    {
        (Capabilities::default(), None)
    }
}

/// One test: two worlds from the fixture, world B carrying the injections.
/// The ticks are stated for the 1 000 tick run.
#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub name: &'static str,
    pub workers_a: usize,
    pub workers_b: usize,
    pub divergence_from: Option<u32>,
    pub panic_at: Option<u32>,
}

impl Case {
    pub const fn new(name: &'static str, workers_a: usize, workers_b: usize) -> Case {
        Case { name, workers_a, workers_b, divergence_from: None, panic_at: None }
    }

    pub const fn diverging_from(self, tick: u32) -> Case {
        Case { divergence_from: Some(tick), ..self }
    }

    pub const fn panicking_at(self, tick: u32) -> Case {
        Case { panic_at: Some(tick), ..self }
    }
}

/// What a comparison found at the first divergent tick.
#[derive(Clone, Debug)]
pub struct DivergenceReport {
    pub first_divergent_tick: u32,
    /// The tick both snapshots were taken at: the divergent tick when the
    /// comparison steps one tick at a time, the end of the batch otherwise.
    pub snapshot_tick: u32,
    pub snapshot_bytes_a: usize,
    pub snapshot_bytes_b: usize,
    pub first_differing_byte: Option<usize>,
    pub groups_differing: Vec<&'static str>,
    pub first_divergent_group: Option<(&'static str, &'static str)>,
}

/// The report of one test, written as one flat JSON object.
#[derive(Clone, Debug)]
pub struct Report {
    pub name: &'static str,
    pub workers_a: usize,
    pub workers_b: usize,
    pub injection_tick: Option<u32>,
    pub panic_tick: Option<u32>,
    pub batch: u32,
    pub ticks: u32,
    pub size: Size,
    pub history_a: Vec<u64>,
    pub history_b: Vec<u64>,
    pub reference_run_hash: u64,
    pub divergence: Option<DivergenceReport>,
    pub failure: Option<Failure>,
    pub restore_a: Duration,
    pub restore_b: Duration,
    pub stepping_a: Duration,
    pub stepping_b: Duration,
    pub wall: Duration,
    pub extracted_ticks: Option<(u64, u64)>,
    pub shared_setup: Duration,
}

impl Report {
    pub fn run_hash_a(&self) -> u64 {
        run_hash(&self.history_a)
    }

    pub fn run_hash_b(&self) -> u64 {
        run_hash(&self.history_b)
    }

    /// Whether a history equals the sequential reference over the ticks it covers.
    fn matches_reference(history: &[u64]) -> bool {
        first_divergent_tick(history, &shared().reference.tick_hashes[..history.len()]).is_none()
    }

    pub fn a_matches_reference(&self) -> bool {
        Report::matches_reference(&self.history_a)
    }

    pub fn b_matches_reference(&self) -> bool {
        self.failure.is_none() && Report::matches_reference(&self.history_b)
    }

    pub fn json(&self) -> String {
        let mut out = Json::new();
        out.object_start();
        out.string("name", self.name);
        out.u64("workers_a", self.workers_a as u64);
        out.u64("workers_b", self.workers_b as u64);
        out.i64("injection_tick", self.injection_tick.map_or(-1, i64::from));
        out.i64("panic_tick", self.panic_tick.map_or(-1, i64::from));
        out.u64("batch", u64::from(self.batch));
        out.u64("ticks", u64::from(self.ticks));
        out.string("size", self.size.name());
        out.hex("reference_run_hash", self.reference_run_hash);
        out.u64("tick_a", self.history_a.len() as u64 - 1);
        out.u64("tick_b", self.history_b.len() as u64 - 1);
        out.hex("final_hash_a", *self.history_a.last().expect("tick 0 at least"));
        out.hex("final_hash_b", *self.history_b.last().expect("tick 0 at least"));
        out.hex("run_hash_a", self.run_hash_a());
        out.hex("run_hash_b", self.run_hash_b());
        out.bool("a_matches_reference", self.a_matches_reference());
        out.bool("b_matches_reference", self.b_matches_reference());
        out.bool("divergence_found", self.divergence.is_some());
        let d = self.divergence.as_ref();
        out.i64("first_divergent_tick", d.map_or(-1, |d| i64::from(d.first_divergent_tick)));
        out.bool("reported_tick_equals_injected", d.zip(self.injection_tick).is_some_and(|(d, t)| d.first_divergent_tick == t));
        out.i64("snapshot_tick", d.map_or(-1, |d| i64::from(d.snapshot_tick)));
        out.u64("snapshot_bytes_a", d.map_or(0, |d| d.snapshot_bytes_a as u64));
        out.u64("snapshot_bytes_b", d.map_or(0, |d| d.snapshot_bytes_b as u64));
        out.bool("snapshots_identical", d.is_some_and(|d| d.first_differing_byte.is_none()));
        out.i64("first_differing_byte", d.and_then(|d| d.first_differing_byte).map_or(-1, |b| b as i64));
        out.string("groups_differing", &d.map_or(String::new(), |d| d.groups_differing.join(",")));
        out.string("first_divergent_group", d.and_then(|d| d.first_divergent_group).map_or("", |g| g.0));
        out.string("first_divergent_system", d.and_then(|d| d.first_divergent_group).map_or("", |g| g.1));
        out.bool("failure_reported", self.failure.is_some());
        if let Some(failure) = &self.failure {
            out.u64("failure_tick", u64::from(failure.tick));
            out.string("failure_node", failure.node);
            out.u64("failure_chunk", failure.chunk as u64);
            out.u64("failure_worker", failure.worker as u64);
            out.string("failure_message", &failure.message);
        }
        out.decimal("restore_us_a", self.restore_a.as_secs_f64() * 1_000_000.0);
        out.decimal("restore_us_b", self.restore_b.as_secs_f64() * 1_000_000.0);
        out.decimal("stepping_ms_a", self.stepping_a.as_secs_f64() * 1_000.0);
        out.decimal("stepping_ms_b", self.stepping_b.as_secs_f64() * 1_000.0);
        out.decimal("wall_ms", self.wall.as_secs_f64() * 1_000.0);
        out.bool("extraction_registered", self.extracted_ticks.is_some());
        out.u64("extracted_ticks_a", self.extracted_ticks.map_or(0, |e| e.0));
        out.u64("extracted_ticks_b", self.extracted_ticks.map_or(0, |e| e.1));
        out.decimal("shared_setup_ms", self.shared_setup.as_secs_f64() * 1_000.0);
        out.object_end();
        out.finish()
    }

    /// Writes the report as `<name>.json` in the report directory, when one is set.
    pub fn write(&self) {
        if let Some(dir) = &settings().report_dir {
            fs::create_dir_all(dir).expect("the report directory is creatable");
            fs::write(dir.join(format!("{}.json", self.name)), self.json()).expect("the report is writable");
        }
    }
}

/// A panic hook, installed once per process, that keeps the injected panics
/// of a failing world out of the output and hands every other panic to the
/// hook that was installed before (the harness's capture included).
pub fn silence_injected_panics() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info.payload();
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("");
            if !message.starts_with("injected panic") {
                previous(info);
            }
        }));
    });
}

fn restore(workers: usize, divergence: Option<Divergence>, injection: Option<Injection>) -> (World, Option<Arc<dyn Fn() -> u64 + Send + Sync>>, Duration) {
    let shared = shared();
    let mut config = WorldConfig::new(settings().size, workers);
    config.divergence = divergence;
    config.injection = injection;
    let (capabilities, extracted) = capabilities();
    let started = Instant::now();
    let world = World::restore(&shared.fixture, config, capabilities).expect("the fixture restores");
    (world, extracted, started.elapsed())
}

/// Runs one case: restores the two worlds from the fixture, steps them to
/// the end in batches, compares every completed tick's hash, and at the first
/// divergent tick snapshots both worlds and names the first divergent group.
/// A failure of world B ends the comparison with the failure in the report.
pub fn run_case(case: &Case) -> Report {
    let settings = settings();
    let shared = shared();
    let ticks = settings.ticks;
    let divergence = case.divergence_from.map(|t| Divergence { from_tick: scaled_tick(t, ticks) });
    let injection = case
        .panic_at
        .filter(|_| settings.panic_armed)
        .map(|t| Injection { tick: scaled_tick(t, ticks), node: Node::Repulsion, chunk: 2 });
    if injection.is_some() {
        silence_injected_panics();
    }
    let batch = if divergence.is_some() { 1 } else { BATCH_WITHOUT_INJECTION };
    let started = Instant::now();
    let (mut a, extracted_a, restore_a) = restore(case.workers_a, None, None);
    let (mut b, extracted_b, restore_b) = restore(case.workers_b, divergence, injection);
    let mut report = None;
    let mut failure = None;
    while a.tick() < ticks {
        let n = batch.min(ticks - a.tick());
        a.step(n).expect("world A carries no injection");
        if let Err(WorldFailed(failed)) = b.step(n) {
            failure = Some(failed);
            break;
        }
        let from = a.tick() - n + 1;
        if let Some(tick) = (from..=a.tick()).find(|&t| a.tick_hash(t) != b.tick_hash(t)) {
            let (snapshot_a, snapshot_b) = (a.snapshot().expect("A is healthy"), b.snapshot().expect("B is healthy"));
            let (ha, hb) = (group_hashes(a.state()), group_hashes(b.state()));
            report = Some(DivergenceReport {
                first_divergent_tick: tick,
                snapshot_tick: a.tick(),
                snapshot_bytes_a: snapshot_a.bytes.len(),
                snapshot_bytes_b: snapshot_b.bytes.len(),
                first_differing_byte: snapshot_a.first_difference(&snapshot_b),
                groups_differing: (0..GROUPS.len()).filter(|&g| ha[g] != hb[g]).map(|g| GROUPS[g].0).collect(),
                first_divergent_group: first_divergent_group(a.state(), b.state()),
            });
            break;
        }
    }
    let wall = started.elapsed();
    Report {
        name: case.name,
        workers_a: case.workers_a,
        workers_b: case.workers_b,
        injection_tick: divergence.map(|d| d.from_tick),
        panic_tick: injection.map(|i| i.tick),
        batch,
        ticks,
        size: settings.size,
        history_a: a.history(),
        history_b: b.history(),
        reference_run_hash: shared.reference.run_hash,
        divergence: report,
        failure,
        restore_a,
        restore_b,
        stepping_a: a.stepping(),
        stepping_b: b.stepping(),
        wall,
        extracted_ticks: extracted_a.zip(extracted_b).map(|(ea, eb)| (ea(), eb())),
        shared_setup: shared.setup,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stated_ticks_scale_to_the_run_and_stay_inside_it() {
        assert_eq!(scaled_tick(500, 1_000), 500);
        assert_eq!(scaled_tick(500, 40), 20);
        assert_eq!(scaled_tick(1, 40), 1);
        assert_eq!(scaled_tick(999, 40), 39);
        assert_eq!(scaled_tick(1_000, 40), 40);
        assert_eq!(scaled_tick(100, 40), 4);
    }

    #[test]
    fn a_report_is_one_flat_json_object_with_the_case_in_it() {
        let report = run_case(&Case::new("unit_1_vs_2", 1, 2));
        let json = report.json();
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"name\":\"unit_1_vs_2\""));
        assert!(json.contains("\"divergence_found\":false"));
        assert!(json.contains("\"a_matches_reference\":true"));
        assert!(json.contains("\"b_matches_reference\":true"));
        assert_eq!(json.contains("\"extraction_registered\":true"), PRESENTATION);
        assert_eq!(report.run_hash_a(), report.run_hash_b());
        assert_eq!(report.run_hash_a(), shared().reference.run_hash);
    }
}
