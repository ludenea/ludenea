//! The scene streaming spike of the scenes and levels research. Throwaway,
//! never product code.
//!
//! The scenarios and the measurements: see the README for the hypotheses
//! they serve. `measure` runs every scenario on the grids, densities, speeds
//! and worker counts of its settings (the self-check runs it on a 3 by 3 grid
//! of 50 entities per cell over 40 ticks) and `report::write` prints the
//! outcome as one JSON document, the per-tick log going to a file.

// The tick graph copied from the fixtures needs the one unsafe construct of
// the runtime skeleton (the split of a column into disjoint chunk slices);
// every other module refuses unsafe code.
#![deny(unsafe_code)]

mod grid;
mod oracle;
mod report;
mod state;
mod streaming;
mod world;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use reference_workload::{first_divergent_tick, fnv1a, run_hash};

use grid::{GridConfig, Path, PayloadStore};
use oracle::OracleRun;
use report::TickLog;
use state::ReadCounts;
use streaming::{InputLog, LiveLoader, StreamingParameters};
use world::{Instruments, NODE_NAMES, World, WorldConfig};

pub const SPIKE: &str = "workload-scene-streaming";
pub const SEED: u64 = 0x5ce6_e5ce_1150_2026;
/// The scripted path: twelve moves of one cell with border crossings back
/// and forth (east, east, west, east, east, west, then the same northward),
/// so that the hysteresis has something to save; a few dozen cells pass
/// within the entering distance, which bounds the run without crossing the
/// whole grid.
pub const PATH_MOVES: [(i32, i32); 12] = [(1, 0), (1, 0), (-1, 0), (1, 0), (1, 0), (-1, 0), (0, 1), (0, 1), (0, -1), (0, 1), (0, 1), (0, -1)];
/// The self-check's path on the 3 by 3 grid: back and forth across one border.
pub const SMALL_PATH_MOVES: [(i32, i32); 8] = [(1, 0), (-1, 0), (1, 0), (-1, 0), (1, 0), (-1, 0), (1, 0), (-1, 0)];
pub const WORKER_COUNTS: [usize; 3] = [1, 4, 8];
/// The live world and the replays are stepped in batches of this many ticks,
/// so that arrivals land between steps as well as during them.
pub const LIVE_BATCH: u32 = 10;
/// The live loader's delay per request, drawn from the clock, milliseconds.
pub const LIVE_DELAY_MS: (u64, u64) = (2, 20);
/// The host's pause after every batch of a live world: a frame loop's sleep,
/// which gives the loader's delays real ticks to land in (a batch of ten
/// ticks of a sparse world takes well under a millisecond otherwise).
pub const LIVE_PAUSE: Duration = Duration::from_millis(5);
pub const ROUNDS: usize = 3;
/// The threshold of the remap cost hypothesis.
pub const REMAP_DIFFERENCE_LIMIT: f64 = 0.10;
/// The threshold of the hysteresis hypothesis: the loads saved along the path.
pub const HYSTERESIS_SAVING_LIMIT: f64 = 0.25;
/// The threshold of the cost hypothesis: the structural changes' share of the tick.
pub const STRUCTURAL_SHARE_LIMIT: f64 = 0.05;

/// What a run of the spike measures: the grids, densities and speeds crossed
/// into scenarios, the worker counts, the streaming parameters, the path.
#[derive(Clone, Debug)]
pub struct Settings {
    pub grids: Vec<u32>,
    pub densities: Vec<u32>,
    /// Ticks per cell of the point of interest: the first is the default speed, the others the controls.
    pub speeds: Vec<u32>,
    pub worker_counts: Vec<usize>,
    pub moves: Vec<(i32, i32)>,
    pub entering: f32,
    pub leaving: f32,
    pub spawn_slice: u32,
    pub scripted_delay_ticks: u32,
    pub rounds: usize,
    /// The live loader's delay per request, drawn from the clock, milliseconds.
    pub live_delay_ms: (u64, u64),
    /// The host's pause between two batches of a live world (a frame loop's
    /// sleep), so that a short run still sees arrivals land.
    pub live_pause: Duration,
    /// Where the per-tick log goes; none keeps it in memory only.
    pub log_path: Option<PathBuf>,
}

impl Settings {
    pub fn full(log_path: PathBuf) -> Settings {
        Settings {
            grids: vec![10, 20],
            densities: vec![250, 2_500],
            speeds: vec![240, 60],
            worker_counts: WORKER_COUNTS.to_vec(),
            moves: PATH_MOVES.to_vec(),
            entering: StreamingParameters::DEFAULT_ENTERING,
            leaving: StreamingParameters::DEFAULT_LEAVING,
            spawn_slice: StreamingParameters::DEFAULT_SLICE,
            scripted_delay_ticks: StreamingParameters::DEFAULT_SCRIPTED_DELAY,
            rounds: ROUNDS,
            live_delay_ms: LIVE_DELAY_MS,
            live_pause: LIVE_PAUSE,
            log_path: Some(log_path),
        }
    }

    /// The small grid the specification names for the self-checks: 3 by 3,
    /// 50 entities per cell, 40 ticks (eight moves of five ticks).
    pub fn small(log_path: Option<PathBuf>) -> Settings {
        Settings {
            grids: vec![3],
            densities: vec![50],
            speeds: vec![5],
            worker_counts: vec![1, 3],
            moves: SMALL_PATH_MOVES.to_vec(),
            entering: 1.0,
            leaving: 1.5,
            spawn_slice: 20,
            scripted_delay_ticks: 3,
            rounds: 2,
            live_delay_ms: (1, 3),
            live_pause: Duration::from_millis(4),
            log_path,
        }
    }

    fn start(&self, grid: u32) -> (u32, u32) {
        ((grid - 1) / 2, (grid - 1) / 2)
    }

    /// The worker count the timed scenarios use: 4, or the largest below it.
    fn timing_workers(&self) -> usize {
        self.worker_counts.iter().copied().filter(|&w| w <= 4).max().unwrap_or(1)
    }
}

/// One combination the matrix runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Scenario {
    pub grid: u32,
    pub density: u32,
    pub ticks_per_cell: u32,
    pub hysteresis: bool,
    pub sliced: bool,
    pub mirrored: bool,
}

impl Scenario {
    pub fn name(&self) -> String {
        format!(
            "grid{}-density{}-speed{}-{}-{}{}",
            self.grid,
            self.density,
            self.ticks_per_cell,
            if self.hysteresis { "hysteresis" } else { "no-hysteresis" },
            if self.sliced { "sliced" } else { "unsliced" },
            if self.mirrored { "-mirrored" } else { "" }
        )
    }

    pub fn grid_config(&self) -> GridConfig {
        GridConfig::new(self.grid, self.density, SEED)
    }

    pub fn path(&self, settings: &Settings) -> Path {
        let moves: Vec<(i32, i32)> = if self.mirrored { settings.moves.iter().map(|(x, y)| (-x, -y)).collect() } else { settings.moves.clone() };
        Path::from_moves(settings.start(self.grid), &moves, self.ticks_per_cell)
    }

    pub fn ticks(&self, settings: &Settings) -> u32 {
        self.path(settings).total_ticks()
    }

    pub fn streaming(&self, settings: &Settings, live: bool) -> StreamingParameters {
        StreamingParameters {
            entering: settings.entering,
            leaving: if self.hysteresis { settings.leaving } else { settings.entering },
            spawn_slice: self.sliced.then_some(settings.spawn_slice),
            scripted_delay_ticks: (!live).then_some(settings.scripted_delay_ticks),
        }
    }

    pub fn config(&self, settings: &Settings, live: bool, workers: usize) -> WorldConfig {
        WorldConfig::new(self.grid_config(), self.streaming(settings, live), self.path(settings), workers)
    }
}

/// How a world got its records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loader {
    Scripted,
    Live,
    Replay,
}

impl Loader {
    pub fn name(self) -> &'static str {
        match self {
            Loader::Scripted => "scripted",
            Loader::Live => "live",
            Loader::Replay => "replay",
        }
    }
}

/// The facts of one world's run.
#[derive(Clone, Debug)]
pub struct ModeRun {
    pub scenario: Scenario,
    pub loader: Loader,
    pub workers: usize,
    pub ticks: u32,
    pub hashes: Vec<u64>,
    pub log: InputLog,
    pub matches_oracle: bool,
    pub first_divergent_tick: Option<usize>,
    pub reads: ReadCounts,
    pub oracle: OracleRun,
    pub max_remap_entries: usize,
    pub max_loaded_cells: usize,
    pub replay_mismatches: u64,
    pub dropped_pending: u64,
    pub instruments: Instruments,
}

impl ModeRun {
    pub fn final_hash(&self) -> u64 {
        *self.hashes.last().expect("a run has its initial hash")
    }

    pub fn run_hash(&self) -> u64 {
        run_hash(&self.hashes)
    }

    pub fn wall_ms(&self) -> f64 {
        ms(self.instruments.stepping)
    }

    pub fn ticks_per_second(&self) -> f64 {
        f64::from(self.ticks) / self.instruments.stepping.as_secs_f64()
    }

    /// The mean time of a cell's spawn (all its slices), microseconds.
    pub fn spawn_us_per_cell(&self) -> f64 {
        mean_us(&self.instruments.spawn_cell_ns)
    }

    pub fn despawn_us_per_cell(&self) -> f64 {
        mean_us(&self.instruments.despawn_cell_ns)
    }

    /// The median time of a cell's spawn, microseconds: the figure of the
    /// remap cost hypothesis, since a few dozen events of some microseconds
    /// each let one preempted event move the mean by half.
    pub fn spawn_median_us_per_cell(&self) -> f64 {
        median_us(&self.instruments.spawn_cell_ns)
    }

    pub fn despawn_median_us_per_cell(&self) -> f64 {
        median_us(&self.instruments.despawn_cell_ns)
    }

    pub fn us_per_spawned_entity(&self) -> f64 {
        if self.instruments.spawned_entities == 0 { 0.0 } else { self.instruments.spawn_ns as f64 / 1_000.0 / self.instruments.spawned_entities as f64 }
    }

    /// The structural changes' (spawns and despawns) share of the stepping wall time.
    pub fn changes_share(&self) -> f64 {
        self.instruments.changes_ns as f64 / self.instruments.stepping.as_nanos() as f64
    }

    /// The whole structural node's share (changes, policy, input fixed point, the lagging buffer's update).
    pub fn structural_node_share(&self) -> f64 {
        self.instruments.structural_ns.iter().sum::<u64>() as f64 / self.instruments.stepping.as_nanos() as f64
    }

    pub fn peak_changes_us(&self) -> f64 {
        self.instruments.changes_per_tick_ns.iter().copied().max().unwrap_or(0) as f64 / 1_000.0
    }

    pub fn peak_structural_us(&self) -> f64 {
        self.instruments.structural_ns.iter().copied().max().unwrap_or(0) as f64 / 1_000.0
    }

    pub fn peak_tick_us(&self) -> f64 {
        self.instruments.tick_wall_ns.iter().copied().max().unwrap_or(0) as f64 / 1_000.0
    }

    pub fn mean_tick_us(&self) -> f64 {
        mean_us(&self.instruments.tick_wall_ns)
    }

    pub fn phase_ms(&self) -> Vec<(&'static str, f64)> {
        NODE_NAMES.iter().zip(self.instruments.phase_ns).map(|(name, ns)| (*name, ns as f64 / 1_000_000.0)).collect()
    }

    /// Whether the world agreed with its oracle on every count and check.
    pub fn clean(&self) -> bool {
        self.matches_oracle && self.reads == self.oracle.reads && self.reads.stale == 0 && self.oracle.clean() && self.replay_mismatches == 0 && self.dropped_pending == 0
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn mean_us(ns: &[u64]) -> f64 {
    if ns.is_empty() { 0.0 } else { ns.iter().sum::<u64>() as f64 / 1_000.0 / ns.len() as f64 }
}

fn median_us(ns: &[u64]) -> f64 {
    if ns.is_empty() {
        return 0.0;
    }
    let mut sorted = ns.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) { (sorted[middle - 1] + sorted[middle]) as f64 / 2.0 } else { sorted[middle] as f64 };
    median / 1_000.0
}

/// The oracles computed so far, by the world's parameters and the content
/// hash of the log they step (the scripted runs of one scenario share one
/// log, so one oracle).
#[derive(Default)]
struct Oracles {
    by_key: BTreeMap<(String, u64), OracleRun>,
}

impl Oracles {
    fn of(&mut self, config: &WorldConfig, store: &PayloadStore, log: &InputLog, ticks: u32) -> OracleRun {
        let key = (format!("{:?}/{:?}/{:?}/{ticks}", config.grid, config.streaming, config.path), fnv1a(&log.encode()));
        self.by_key.entry(key).or_insert_with(|| oracle::run(&config.grid, &config.streaming, &config.path, store, log, ticks)).clone()
    }
}

/// Everything the scenarios share: the settings, the oracles, the log file.
struct Bench {
    settings: Settings,
    oracles: Oracles,
    tick_log: TickLog,
}

impl Bench {
    /// Runs one world to the end of its path and compares it with the oracle
    /// of its log. A live world steps in batches while its loader delivers;
    /// a scripted one in one call; a replay in batches too.
    fn run(&mut self, scenario: &Scenario, loader: Loader, workers: usize, store: &Arc<PayloadStore>, live_loader: Option<&LiveLoader>, replay: Option<&InputLog>) -> ModeRun {
        let ticks = scenario.ticks(&self.settings);
        let mut config = scenario.config(&self.settings, loader == Loader::Live, workers);
        if let Some(log) = replay {
            config = config.replaying(log.clone(), workers);
        }
        let mut world = World::new(config.clone(), store.clone(), live_loader.map(LiveLoader::sender));
        match loader {
            Loader::Scripted => world.step(ticks).expect("the world steps"),
            Loader::Live | Loader::Replay => {
                while world.tick() < ticks {
                    world.step(LIVE_BATCH.min(ticks - world.tick())).expect("the world steps");
                    if loader == Loader::Live && !self.settings.live_pause.is_zero() {
                        std::thread::sleep(self.settings.live_pause);
                    }
                }
            }
        }
        let hashes = world.hashes();
        let log = world.log().clone();
        let oracle = self.oracles.of(&config, store, &log, ticks);
        let first_divergent_tick = first_divergent_tick(&hashes, &oracle.hashes);
        let run = ModeRun {
            scenario: scenario.clone(),
            loader,
            workers,
            ticks,
            hashes,
            log,
            matches_oracle: first_divergent_tick.is_none(),
            first_divergent_tick,
            reads: world.reads(),
            oracle,
            max_remap_entries: world.instances().max_remap_entries,
            max_loaded_cells: world.instances().max_loaded_cells,
            replay_mismatches: world.replay_mismatches(),
            dropped_pending: world.instances().dropped_pending,
            instruments: world.instruments(),
        };
        self.tick_log.record(&run);
        eprintln!(
            "{} {} workers {}: {} ticks in {:.0} ms, oracle {}, loads {}, unloads {}, reads absent {} present {} stale {}",
            run.scenario.name(),
            loader.name(),
            workers,
            ticks,
            run.wall_ms(),
            if run.matches_oracle { "matches" } else { "DIFFERS" },
            run.log.accepted_loads(),
            run.log.accepted_unloads(),
            run.reads.absent,
            run.reads.present,
            run.reads.stale
        );
        run
    }
}

/// One scenario of the matrix: the scripted runs at every worker count, the
/// live run and its replays at every worker count.
#[derive(Clone, Debug)]
pub struct ScenarioReport {
    pub scenario: Scenario,
    pub ticks: u32,
    pub scripted: Vec<ModeRun>,
    pub live: ModeRun,
    pub replays: Vec<ModeRun>,
}

impl ScenarioReport {
    pub fn scripted_identical(&self) -> bool {
        self.scripted.windows(2).all(|pair| pair[0].hashes == pair[1].hashes && pair[0].log == pair[1].log)
    }

    pub fn replays_identical_to_live(&self) -> bool {
        self.replays.iter().all(|replay| replay.hashes == self.live.hashes && replay.log == self.live.log)
    }

    pub fn holds(&self) -> bool {
        self.scripted_identical()
            && self.replays_identical_to_live()
            && self.scripted.iter().chain(std::iter::once(&self.live)).chain(&self.replays).all(ModeRun::clean)
    }

    pub fn every_run(&self) -> impl Iterator<Item = &ModeRun> {
        self.scripted.iter().chain(std::iter::once(&self.live)).chain(&self.replays)
    }
}

/// The remap cost hypothesis: the two grids at the highest density and the
/// default speed, in alternating rounds; the best (lowest) mean cost per
/// cell of each is the figure.
#[derive(Clone, Debug)]
pub struct RemapCost {
    pub density: u32,
    pub ticks_per_cell: u32,
    pub workers: usize,
    pub grids: Vec<RemapGrid>,
}

#[derive(Clone, Debug)]
pub struct RemapGrid {
    pub grid: u32,
    pub rounds: Vec<ModeRun>,
}

impl RemapGrid {
    /// The lowest median spawn time per cell over the rounds.
    pub fn best_spawn_us_per_cell(&self) -> f64 {
        self.rounds.iter().map(ModeRun::spawn_median_us_per_cell).fold(f64::INFINITY, f64::min)
    }

    pub fn best_despawn_us_per_cell(&self) -> f64 {
        self.rounds.iter().map(ModeRun::despawn_median_us_per_cell).fold(f64::INFINITY, f64::min)
    }

    /// The lowest mean spawn time per cell over the rounds, for information.
    pub fn best_mean_spawn_us_per_cell(&self) -> f64 {
        self.rounds.iter().map(ModeRun::spawn_us_per_cell).fold(f64::INFINITY, f64::min)
    }

    pub fn best_mean_despawn_us_per_cell(&self) -> f64 {
        self.rounds.iter().map(ModeRun::despawn_us_per_cell).fold(f64::INFINITY, f64::min)
    }

    pub fn max_remap_entries(&self) -> usize {
        self.rounds.iter().map(|r| r.max_remap_entries).max().unwrap_or(0)
    }

    pub fn max_loaded_cells(&self) -> usize {
        self.rounds.iter().map(|r| r.max_loaded_cells).max().unwrap_or(0)
    }
}

pub fn relative_difference(a: f64, b: f64) -> f64 {
    let largest = a.abs().max(b.abs());
    if largest == 0.0 { 0.0 } else { (a - b).abs() / largest }
}

impl RemapCost {
    pub fn measured(&self) -> bool {
        self.grids.len() >= 2
    }

    fn pair(&self) -> Option<(&RemapGrid, &RemapGrid)> {
        self.measured().then(|| (&self.grids[0], &self.grids[self.grids.len() - 1]))
    }

    pub fn spawn_difference(&self) -> f64 {
        self.pair().map_or(0.0, |(a, b)| relative_difference(a.best_spawn_us_per_cell(), b.best_spawn_us_per_cell()))
    }

    pub fn despawn_difference(&self) -> f64 {
        self.pair().map_or(0.0, |(a, b)| relative_difference(a.best_despawn_us_per_cell(), b.best_despawn_us_per_cell()))
    }

    pub fn table_difference(&self) -> f64 {
        self.pair().map_or(0.0, |(a, b)| relative_difference(a.max_remap_entries() as f64, b.max_remap_entries() as f64))
    }

    pub fn holds(&self) -> bool {
        self.measured()
            && self.spawn_difference() < REMAP_DIFFERENCE_LIMIT
            && self.despawn_difference() < REMAP_DIFFERENCE_LIMIT
            && self.table_difference() < REMAP_DIFFERENCE_LIMIT
            && self.grids.iter().all(|g| g.rounds.iter().all(ModeRun::clean))
    }
}

/// The hysteresis hypothesis: the smallest grid at the lowest density and
/// the default speed, with and without hysteresis.
#[derive(Clone, Debug)]
pub struct Hysteresis {
    pub with: ModeRun,
    pub without: ModeRun,
}

impl Hysteresis {
    pub fn saving(&self) -> f64 {
        let without = self.without.log.accepted_loads() as f64;
        if without == 0.0 { 0.0 } else { 1.0 - (self.with.log.accepted_loads() as f64 / without) }
    }

    pub fn holds(&self) -> bool {
        self.saving() >= HYSTERESIS_SAVING_LIMIT && self.with.clean() && self.without.clean()
    }
}

/// The cost hypothesis: the largest grid at the highest density and the
/// fastest traversal, sliced and unsliced.
#[derive(Clone, Debug)]
pub struct Cost {
    pub sliced: ModeRun,
    pub unsliced: ModeRun,
}

impl Cost {
    pub fn holds(&self) -> bool {
        self.sliced.changes_share() < STRUCTURAL_SHARE_LIMIT && self.sliced.clean() && self.unsliced.clean()
    }
}

/// The many worlds hypothesis: two worlds on one grid from two paths with
/// one shared payload store and one live loader.
#[derive(Clone, Debug)]
pub struct TwoWorlds {
    pub first: ModeRun,
    pub second: ModeRun,
    pub payloads: usize,
    pub imports: u64,
    pub requests_served: u64,
}

impl TwoWorlds {
    pub fn holds(&self) -> bool {
        self.first.clean() && self.second.clean() && self.imports == self.payloads as u64 && self.first.hashes != self.second.hashes
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub scenarios: Vec<ScenarioReport>,
    pub remap_cost: RemapCost,
    pub hysteresis: Hysteresis,
    pub cost: Cost,
    pub two_worlds: TwoWorlds,
}

impl Report {
    pub fn every_run(&self) -> Vec<&ModeRun> {
        let mut runs: Vec<&ModeRun> = self.scenarios.iter().flat_map(ScenarioReport::every_run).collect();
        runs.extend(self.remap_cost.grids.iter().flat_map(|g| g.rounds.iter()));
        runs.push(&self.hysteresis.without);
        runs.push(&self.cost.unsliced);
        runs.push(&self.two_worlds.first);
        runs.push(&self.two_worlds.second);
        runs
    }

    pub fn determinism_holds(&self) -> bool {
        self.scenarios.iter().all(ScenarioReport::holds)
    }

    pub fn reads(&self) -> ReadCounts {
        let mut total = ReadCounts::default();
        for run in self.every_run() {
            total.add(run.reads);
        }
        total
    }

    pub fn oracle_check_failures(&self) -> u64 {
        self.every_run().iter().map(|run| run.oracle.check_failures).sum()
    }

    pub fn not_present_holds(&self) -> bool {
        let reads = self.reads();
        reads.stale == 0 && reads.absent > 0 && reads.present > 0 && self.oracle_check_failures() == 0 && self.every_run().iter().all(|run| run.reads == run.oracle.reads)
    }
}

/// Runs every scenario of the settings.
pub fn measure(settings: &Settings) -> Report {
    let tick_log = TickLog::new(settings.log_path.clone());
    let mut bench = Bench { settings: settings.clone(), oracles: Oracles::default(), tick_log };
    let default_speed = settings.speeds[0];
    let timing_workers = settings.timing_workers();

    // The matrix, hysteresis on and the spawn sliced.
    let mut scenarios = Vec::new();
    for &grid in &settings.grids {
        for &density in &settings.densities {
            for &ticks_per_cell in &settings.speeds {
                let scenario = Scenario { grid, density, ticks_per_cell, hysteresis: true, sliced: true, mirrored: false };
                let store = PayloadStore::new(scenario.grid_config());
                let scripted: Vec<ModeRun> = settings.worker_counts.iter().map(|&workers| bench.run(&scenario, Loader::Scripted, workers, &store, None, None)).collect();
                let loader = LiveLoader::spawn(store.clone(), settings.live_delay_ms);
                let live = bench.run(&scenario, Loader::Live, timing_workers, &store, Some(&loader), None);
                loader.finish();
                let replays: Vec<ModeRun> = settings.worker_counts.iter().map(|&workers| bench.run(&scenario, Loader::Replay, workers, &store, None, Some(&live.log))).collect();
                scenarios.push(ScenarioReport { scenario, ticks: live.ticks, scripted, live, replays });
            }
        }
    }

    let highest_density = *settings.densities.iter().max().expect("a density");
    let lowest_density = *settings.densities.iter().min().expect("a density");
    let smallest_grid = *settings.grids.iter().min().expect("a grid");
    let largest_grid = *settings.grids.iter().max().expect("a grid");
    let fastest = *settings.speeds.iter().min().expect("a speed");

    // The remap cost: alternating rounds over the grids at the highest density.
    let cost_scenarios: Vec<Scenario> = settings
        .grids
        .iter()
        .map(|&grid| Scenario { grid, density: highest_density, ticks_per_cell: default_speed, hysteresis: true, sliced: true, mirrored: false })
        .collect();
    let stores: Vec<Arc<PayloadStore>> = cost_scenarios.iter().map(|s| PayloadStore::new(s.grid_config())).collect();
    let mut rounds: Vec<Vec<ModeRun>> = vec![Vec::new(); cost_scenarios.len()];
    for round in 0..settings.rounds {
        for (index, scenario) in cost_scenarios.iter().enumerate() {
            // The matrix already ran the first round of every grid.
            let run = if round == 0 {
                scenarios.iter().find(|s| s.scenario == *scenario).and_then(|s| s.scripted.iter().find(|r| r.workers == timing_workers).cloned())
            } else {
                None
            };
            rounds[index].push(run.unwrap_or_else(|| bench.run(scenario, Loader::Scripted, timing_workers, &stores[index], None, None)));
        }
    }
    let remap_cost = RemapCost {
        density: highest_density,
        ticks_per_cell: default_speed,
        workers: timing_workers,
        grids: cost_scenarios.iter().zip(rounds).map(|(s, rounds)| RemapGrid { grid: s.grid, rounds }).collect(),
    };

    // The hysteresis control: the smallest grid, the lowest density, the default speed.
    let with_scenario = Scenario { grid: smallest_grid, density: lowest_density, ticks_per_cell: default_speed, hysteresis: true, sliced: true, mirrored: false };
    let with = scenarios.iter().find(|s| s.scenario == with_scenario).map(|s| s.scripted[0].clone()).expect("in the matrix");
    let without_scenario = Scenario { hysteresis: false, ..with_scenario };
    let store = PayloadStore::new(without_scenario.grid_config());
    let without = bench.run(&without_scenario, Loader::Scripted, settings.worker_counts[0], &store, None, None);
    let hysteresis = Hysteresis { with, without };

    // The cost control: the largest grid, the highest density, the fastest traversal, unsliced.
    let sliced_scenario = Scenario { grid: largest_grid, density: highest_density, ticks_per_cell: fastest, hysteresis: true, sliced: true, mirrored: false };
    let sliced = scenarios
        .iter()
        .find(|s| s.scenario == sliced_scenario)
        .and_then(|s| s.scripted.iter().find(|r| r.workers == timing_workers).cloned())
        .expect("in the matrix");
    let unsliced_scenario = Scenario { sliced: false, ..sliced_scenario };
    let store = PayloadStore::new(unsliced_scenario.grid_config());
    let unsliced = bench.run(&unsliced_scenario, Loader::Scripted, timing_workers, &store, None, None);
    let cost = Cost { sliced, unsliced };

    // Two worlds: the smallest grid at the highest density and the fastest
    // traversal, one path and its mirror, one store, one live loader.
    let first_scenario = Scenario { grid: smallest_grid, density: highest_density, ticks_per_cell: fastest, hysteresis: true, sliced: true, mirrored: false };
    let second_scenario = Scenario { mirrored: true, ..first_scenario.clone() };
    let store = PayloadStore::new(first_scenario.grid_config());
    let loader = LiveLoader::spawn(store.clone(), settings.live_delay_ms);
    let (first, second) = bench.run_pair(&first_scenario, &second_scenario, timing_workers, &store, &loader);
    let requests_served = loader.finish();
    let two_worlds = TwoWorlds { first, second, payloads: store.len(), imports: store.imports(), requests_served };

    bench.tick_log.finish();
    Report { scenarios, remap_cost, hysteresis, cost, two_worlds }
}

impl Bench {
    /// Two live worlds stepped alternately in batches from one thread on one
    /// store and one loader, each compared with the oracle of its own log.
    fn run_pair(&mut self, first: &Scenario, second: &Scenario, workers: usize, store: &Arc<PayloadStore>, loader: &LiveLoader) -> (ModeRun, ModeRun) {
        let ticks = first.ticks(&self.settings);
        assert_eq!(ticks, second.ticks(&self.settings));
        let configs = [first.config(&self.settings, true, workers), second.config(&self.settings, true, workers)];
        let mut worlds: Vec<World> = configs.iter().map(|config| World::new(config.clone(), store.clone(), Some(loader.sender()))).collect();
        while worlds[0].tick() < ticks {
            for world in &mut worlds {
                world.step(LIVE_BATCH.min(ticks - world.tick())).expect("the world steps");
            }
            if !self.settings.live_pause.is_zero() {
                std::thread::sleep(self.settings.live_pause);
            }
        }
        let mut runs = Vec::new();
        for (world, (scenario, config)) in worlds.iter().zip([first, second].into_iter().zip(&configs)) {
            let hashes = world.hashes();
            let log = world.log().clone();
            let oracle = self.oracles.of(config, store, &log, ticks);
            let first_divergent_tick = first_divergent_tick(&hashes, &oracle.hashes);
            let run = ModeRun {
                scenario: scenario.clone(),
                loader: Loader::Live,
                workers,
                ticks,
                hashes,
                log,
                matches_oracle: first_divergent_tick.is_none(),
                first_divergent_tick,
                reads: world.reads(),
                oracle,
                max_remap_entries: world.instances().max_remap_entries,
                max_loaded_cells: world.instances().max_loaded_cells,
                replay_mismatches: world.replay_mismatches(),
                dropped_pending: world.instances().dropped_pending,
                instruments: world.instruments(),
            };
            self.tick_log.record(&run);
            eprintln!("two worlds, {}: oracle {}, loads {}", run.scenario.name(), if run.matches_oracle { "matches" } else { "DIFFERS" }, run.log.accepted_loads());
            runs.push(run);
        }
        let second = runs.pop().expect("two runs");
        let first = runs.pop().expect("two runs");
        (first, second)
    }
}

/// The directory the research CI publishes: `out/<cell>/` under the
/// workspace when the spike runs from there, the temporary directory otherwise.
fn output_dir(cell: &str) -> PathBuf {
    let candidate = FilePath::new("out").join(cell);
    if fs::create_dir_all(&candidate).is_ok() {
        return candidate;
    }
    let fallback = std::env::temp_dir().join(format!("{SPIKE}-{cell}"));
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let log_path = output_dir(&cell).join(format!("{SPIKE}-ticks.log"));
    // A build with debug assertions measures the small grid: the dev
    // profile's worlds of thousands of entities would take too long. The
    // report says which settings ran.
    let settings = if cfg!(debug_assertions) { Settings::small(Some(log_path)) } else { Settings::full(log_path) };
    let report = measure(&settings);
    println!("{}", report::write(&cell, &settings, &report));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_on_the_small_grid_passes_every_determinism_and_presence_check() {
        let settings = Settings::small(None);
        let report = measure(&settings);

        assert_eq!(report.scenarios.len(), 1);
        let scenario = &report.scenarios[0];
        assert_eq!(scenario.ticks, 40);
        assert_eq!(scenario.scripted.len(), 2, "1 and 3 workers");
        assert_eq!(scenario.replays.len(), 2);
        assert!(scenario.scripted_identical(), "the scripted runs at 1 and 3 workers give one history");
        assert!(scenario.replays_identical_to_live(), "the replays reproduce the live run at 1 and 3 workers");
        assert!(scenario.holds(), "{scenario:?}");
        assert!(report.determinism_holds());
        for run in report.every_run() {
            assert!(run.matches_oracle, "{} {} {}: diverges at {:?}", run.scenario.name(), run.loader.name(), run.workers, run.first_divergent_tick);
            assert_eq!(run.reads, run.oracle.reads);
            assert_eq!(run.reads.stale, 0);
            assert_eq!(run.oracle.check_failures, 0);
            assert_eq!(run.replay_mismatches, 0);
            assert_eq!(run.dropped_pending, 0);
            assert_eq!(run.log.rejected_commands(), 0, "the policy never issues a command the record refuses: {:?}", run.log);
            assert_eq!(run.max_remap_entries, run.max_loaded_cells * 50, "the table holds the loaded cells' entities and nothing else");
            if run.loader == Loader::Scripted {
                assert!(run.reads.absent > 0 && run.reads.present > 0, "{:?}", run.reads);
            }
        }
        assert!(scenario.scripted[0].max_loaded_cells >= 5, "{}", scenario.scripted[0].max_loaded_cells);
        assert!(report.not_present_holds());
        assert!(report.scenarios[0].live.log.accepted_arrivals() > 0, "the live loader landed something within the run");

        assert!(!report.remap_cost.measured(), "one grid only");
        assert_eq!(report.remap_cost.grids.len(), 1);
        assert_eq!(report.remap_cost.grids[0].rounds.len(), 2);

        // Eight crossings of one border: without hysteresis every crossing
        // reloads the two or three cells that fell out; with the leaving
        // distance half a cell farther, only the far cell behind does.
        assert_eq!(report.hysteresis.with.log.accepted_loads(), 11);
        assert_eq!(report.hysteresis.without.log.accepted_loads(), 25);
        assert!(report.hysteresis.saving() > 0.5, "{}", report.hysteresis.saving());
        assert!(report.hysteresis.holds());

        assert!(report.cost.sliced.clean() && report.cost.unsliced.clean());
        assert_ne!(report.cost.sliced.hashes, report.cost.unsliced.hashes, "the slice is a parameter of the result");
        assert!(report.cost.sliced.changes_share() > 0.0 && report.cost.sliced.peak_tick_us() > 0.0);

        assert!(report.two_worlds.holds(), "{:?}", (report.two_worlds.first.matches_oracle, report.two_worlds.second.matches_oracle, report.two_worlds.payloads, report.two_worlds.imports));
        assert!(report.two_worlds.payloads <= 9);

        let json = report::write("self-check", &settings, &report);
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"spike\":\"workload-scene-streaming\""));
        assert!(json.contains("\"cell\":\"self-check\""));
        assert!(!json.contains("\"matches_oracle\":false"));
        assert!(!json.contains("\"holds\":false,\"scenarios\""), "determinism holds");
    }

    #[test]
    fn the_paths_fit_their_grids_and_the_mirror_too() {
        let settings = Settings::full(PathBuf::from("unused"));
        for &grid in &settings.grids {
            for mirrored in [false, true] {
                let scenario = Scenario { grid, density: 250, ticks_per_cell: 240, hysteresis: true, sliced: true, mirrored };
                let path = scenario.path(&settings);
                assert!(path.fits(&scenario.grid_config()), "grid {grid} mirrored {mirrored}");
                assert_eq!(path.total_ticks(), 12 * 240);
                // Every cell within the entering distance of the path lies in
                // the grid: the two grids request the same cells (the
                // mirrored path serves the two worlds scenario alone).
                if !mirrored {
                    for (x, y) in &path.waypoints {
                        assert!(*x >= 2 && *y >= 2 && *x + 2 < grid && *y + 2 < grid, "({x}, {y}) on grid {grid}");
                    }
                }
            }
        }
        assert_eq!(Settings::small(None).timing_workers(), 3);
        assert_eq!(settings.timing_workers(), 4);
        assert_eq!(relative_difference(90.0, 100.0), 0.1);
        assert_eq!(relative_difference(0.0, 0.0), 0.0);
    }
}
