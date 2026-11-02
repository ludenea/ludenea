//! Reference workload, version 1: the sequential reference implementation of
//! the synthetic simulation the research spikes measure against (see
//! README.md for the specification), plus the primitives a spike reuses so
//! that its numbers compare with the reference: the pseudo-random stream, the
//! state and run hashes, the per-slot kernels of each system, and the writer
//! of the JSON report.
//!
//! It is a spike fixture, never product code: plain arrays and loops, no
//! design the engine inherits. Every floating-point operation follows the
//! order the specification states; nothing here may be "simplified" without a
//! new workload version.

use std::time::{Duration, Instant};

pub const VERSION: u32 = 1;
pub const DEFAULT_SEED: u64 = 0x4c75_6465_6e65_6121;
pub const DEFAULT_TICK_COUNT: u32 = 1_000;
pub const WARM_UP_TICKS: u32 = 10;

pub const TIME_STEP: f32 = 0.015_625; // 2^-6
pub const WORLD_SIDE: f32 = 1_024.0;
pub const CELL_SIZE: f32 = 16.0;
pub const GRID_SIDE: usize = 64; // WORLD_SIDE / CELL_SIZE
pub const INTERACTION_RADIUS: f32 = 8.0;
pub const REPULSION_STRENGTH: f32 = 64.0;
pub const SPEED_LIMIT: f32 = 64.0;
pub const DAMPING: f32 = 0.996_093_75; // 1 - 2^-8
pub const KILLS_PER_TICK: usize = 8;
pub const SPAWNS_PER_TICK: usize = 8;
pub const CHUNK: usize = 256;
pub const WANDER_ANGLE_PERIOD: usize = 4_096;
pub const WANDER_ANGLE_SCALE: f32 = 0.015_625; // 2^-6
pub const WANDER_STRENGTH: f32 = 0.25;

const TWO_POW_MINUS_24: f32 = 5.960_464_477_539_062_5e-8;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The three population sizes the specification names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Small,
    Default,
    Large,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::Small, Size::Default, Size::Large];

    pub fn entity_count(self) -> usize {
        match self {
            Size::Small => 1_000,
            Size::Default => 10_000,
            Size::Large => 100_000,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Default => "default",
            Size::Large => "large",
        }
    }
}

/// The variables of a run. Everything else is fixed by the specification.
#[derive(Clone, Copy, Debug)]
pub struct Parameters {
    pub seed: u64,
    pub size: Size,
    pub tick_count: u32,
    pub wander: bool,
}

impl Parameters {
    pub fn new(size: Size, tick_count: u32, wander: bool) -> Parameters {
        Parameters { seed: DEFAULT_SEED, size, tick_count, wander }
    }

    pub fn entity_count(&self) -> usize {
        self.size.entity_count()
    }
}

/// The global SplitMix64 stream of the specification.
#[derive(Clone, Copy, Debug)]
pub struct SplitMix64 {
    pub state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> SplitMix64 {
        SplitMix64 { state: seed }
    }

    pub fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A uniform value in [0, 1) with 24 significant bits: exact in `f32`.
    pub fn u01(&mut self) -> f32 {
        ((self.next() >> 40) as f32) * TWO_POW_MINUS_24
    }

    pub fn mass(&mut self) -> f32 {
        [1.0, 2.0, 4.0][(self.next() % 3) as usize]
    }
}

/// The complete simulation state: one entry per slot, plus the two globals.
#[derive(Clone, Debug)]
pub struct State {
    pub generation: Vec<u32>,
    pub alive: Vec<bool>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub mass: Vec<f32>,
    pub rng: SplitMix64,
    pub energy: f32,
}

impl State {
    /// The initialization of the specification: every slot drawn in order.
    pub fn initialize(parameters: &Parameters) -> State {
        let n = parameters.entity_count();
        let mut state = State {
            generation: vec![0; n],
            alive: vec![true; n],
            x: vec![0.0; n],
            y: vec![0.0; n],
            vx: vec![0.0; n],
            vy: vec![0.0; n],
            mass: vec![0.0; n],
            rng: SplitMix64::new(parameters.seed),
            energy: 0.0,
        };
        for slot in 0..n {
            state.draw(slot);
        }
        state
    }

    pub fn len(&self) -> usize {
        self.alive.len()
    }

    pub fn is_empty(&self) -> bool {
        self.alive.is_empty()
    }

    pub fn chunk_count(&self) -> usize {
        self.len().div_ceil(CHUNK)
    }

    /// Draws position, velocity and mass for one slot, in the specified order.
    fn draw(&mut self, slot: usize) {
        self.x[slot] = self.rng.u01() * WORLD_SIDE;
        self.y[slot] = self.rng.u01() * WORLD_SIDE;
        self.vx[slot] = ((self.rng.u01() * 2.0) - 1.0) * 32.0;
        self.vy[slot] = ((self.rng.u01() * 2.0) - 1.0) * 32.0;
        self.mass[slot] = self.rng.mass();
    }

    /// System 7: `KILLS_PER_TICK` kills then `SPAWNS_PER_TICK` spawns from the
    /// global stream. Sequential by nature: the stream is consumed in order.
    pub fn structural_changes(&mut self) {
        let n = self.len();
        for _ in 0..KILLS_PER_TICK {
            let slot = (self.rng.next() % n as u64) as usize;
            self.alive[slot] = false;
        }
        for _ in 0..SPAWNS_PER_TICK {
            let Some(slot) = self.alive.iter().position(|alive| !alive) else {
                return;
            };
            self.draw(slot);
            self.generation[slot] += 1;
            self.alive[slot] = true;
        }
    }

    /// System 8: the tick hash `h_t`, FNV-1a 64 over the canonical bytes.
    pub fn hash(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.len() {
            hasher.write(&self.generation[slot].to_le_bytes());
            hasher.write(&[u8::from(self.alive[slot])]);
            if self.alive[slot] {
                hasher.write(&self.x[slot].to_le_bytes());
                hasher.write(&self.y[slot].to_le_bytes());
                hasher.write(&self.vx[slot].to_le_bytes());
                hasher.write(&self.vy[slot].to_le_bytes());
                hasher.write(&self.mass[slot].to_le_bytes());
            }
        }
        hasher.write(&self.energy.to_le_bytes());
        hasher.write(&self.rng.state.to_le_bytes());
        hasher.finish()
    }
}

/// The slot range of chunk `chunk` (the last chunk may be shorter).
pub fn chunk_range(chunk: usize, entity_count: usize) -> std::ops::Range<usize> {
    let start = chunk * CHUNK;
    start..(start + CHUNK).min(entity_count)
}

/// System 1: the cell lists, as a counting sort. Cell (cx, cy) lists the alive
/// slots whose position falls in it, in increasing index order.
#[derive(Clone, Debug, Default)]
pub struct Grid {
    cell_start: Vec<u32>,
    slots: Vec<u32>,
}

impl Grid {
    pub fn new() -> Grid {
        Grid { cell_start: vec![0; GRID_SIDE * GRID_SIDE + 1], slots: Vec::new() }
    }

    pub fn cell_of(x: f32, y: f32) -> (usize, usize) {
        ((x / CELL_SIZE).floor() as usize, (y / CELL_SIZE).floor() as usize)
    }

    pub fn build(&mut self, state: &State) {
        let cells = GRID_SIDE * GRID_SIDE;
        self.cell_start.clear();
        self.cell_start.resize(cells + 1, 0);
        for slot in 0..state.len() {
            if state.alive[slot] {
                let (cx, cy) = Grid::cell_of(state.x[slot], state.y[slot]);
                self.cell_start[cy * GRID_SIDE + cx + 1] += 1;
            }
        }
        for cell in 0..cells {
            self.cell_start[cell + 1] += self.cell_start[cell];
        }
        let mut cursor = self.cell_start.clone();
        self.slots.clear();
        self.slots.resize(self.cell_start[cells] as usize, 0);
        for slot in 0..state.len() {
            if state.alive[slot] {
                let (cx, cy) = Grid::cell_of(state.x[slot], state.y[slot]);
                let cell = cy * GRID_SIDE + cx;
                self.slots[cursor[cell] as usize] = slot as u32;
                cursor[cell] += 1;
            }
        }
    }

    pub fn slots_in(&self, cx: usize, cy: usize) -> &[u32] {
        let cell = cy * GRID_SIDE + cx;
        &self.slots[self.cell_start[cell] as usize..self.cell_start[cell + 1] as usize]
    }
}

/// System 2 for one slot: the repulsion force on slot `i`, visiting the 3 x 3
/// neighborhood with the y offset as the outer loop and the x offset as the
/// inner loop, and the slots of each cell in increasing index order.
pub fn repulsion_force(state: &State, grid: &Grid, i: usize) -> (f32, f32) {
    let (xi, yi) = (state.x[i], state.y[i]);
    let (cx, cy) = Grid::cell_of(xi, yi);
    let mut fx = 0.0_f32;
    let mut fy = 0.0_f32;
    for b in -1_isize..=1 {
        for a in -1_isize..=1 {
            let (gx, gy) = (cx as isize + a, cy as isize + b);
            if gx < 0 || gy < 0 || gx >= GRID_SIDE as isize || gy >= GRID_SIDE as isize {
                continue;
            }
            for &j in grid.slots_in(gx as usize, gy as usize) {
                let j = j as usize;
                if j == i {
                    continue;
                }
                let dx = state.x[j] - xi;
                let dy = state.y[j] - yi;
                let d2 = (dx * dx) + (dy * dy);
                if d2 < INTERACTION_RADIUS * INTERACTION_RADIUS && d2 > 0.0 {
                    let d = d2.sqrt();
                    let s = (REPULSION_STRENGTH * (INTERACTION_RADIUS - d)) / d;
                    fx -= dx * s;
                    fy -= dy * s;
                }
            }
        }
    }
    (fx, fy)
}

/// System 2, second half, for one velocity component.
pub fn repelled_velocity(v: f32, force: f32, mass: f32) -> f32 {
    let acceleration = force / mass;
    ((v + (acceleration * TIME_STEP)) * DAMPING).clamp(-SPEED_LIMIT, SPEED_LIMIT)
}

/// System 3 for one slot: the platform's `f32` cosine and sine, on purpose.
pub fn wandered_velocity(vx: f32, vy: f32, tick: u32, i: usize) -> (f32, f32) {
    let theta = (((tick as usize + i) % WANDER_ANGLE_PERIOD) as f32) * WANDER_ANGLE_SCALE;
    (
        (vx + (theta.cos() * WANDER_STRENGTH)).clamp(-SPEED_LIMIT, SPEED_LIMIT),
        (vy + (theta.sin() * WANDER_STRENGTH)).clamp(-SPEED_LIMIT, SPEED_LIMIT),
    )
}

/// System 4 for one axis.
pub fn integrated(p: f32, v: f32) -> f32 {
    p + (v * TIME_STEP)
}

/// System 5 for one axis: one reflection suffices, and the last line keeps
/// `p == WORLD_SIDE` inside the grid.
pub fn bounced(mut p: f32, mut v: f32) -> (f32, f32) {
    if p < 0.0 {
        p = -p;
        v = -v;
    }
    if p >= WORLD_SIDE {
        p = (2.0 * WORLD_SIDE) - p;
        v = -v;
    }
    if p >= WORLD_SIDE {
        p = WORLD_SIDE - 0.0625;
    }
    (p, v)
}

/// System 6 for one slot.
pub fn kinetic_energy(vx: f32, vy: f32, mass: f32) -> f32 {
    ((vx * vx) + (vy * vy)) * (0.5 * mass)
}

/// System 6 for one chunk: the alive slots' energies added in index order.
pub fn chunk_energy(state: &State, chunk: usize) -> f32 {
    let mut sum = 0.0_f32;
    for slot in chunk_range(chunk, state.len()) {
        if state.alive[slot] {
            sum += kinetic_energy(state.vx[slot], state.vy[slot], state.mass[slot]);
        }
    }
    sum
}

/// Systems 2 to 6 as whole-population sequential passes, in the specified order.
pub fn repulsion(state: &mut State, grid: &Grid) {
    for i in 0..state.len() {
        if state.alive[i] {
            let (fx, fy) = repulsion_force(state, grid, i);
            state.vx[i] = repelled_velocity(state.vx[i], fx, state.mass[i]);
            state.vy[i] = repelled_velocity(state.vy[i], fy, state.mass[i]);
        }
    }
}

pub fn wander(state: &mut State, tick: u32) {
    for i in 0..state.len() {
        if state.alive[i] {
            (state.vx[i], state.vy[i]) = wandered_velocity(state.vx[i], state.vy[i], tick, i);
        }
    }
}

pub fn integration(state: &mut State) {
    for i in 0..state.len() {
        if state.alive[i] {
            state.x[i] = integrated(state.x[i], state.vx[i]);
            state.y[i] = integrated(state.y[i], state.vy[i]);
        }
    }
}

pub fn boundary(state: &mut State) {
    for i in 0..state.len() {
        if state.alive[i] {
            (state.x[i], state.vx[i]) = bounced(state.x[i], state.vx[i]);
            (state.y[i], state.vy[i]) = bounced(state.y[i], state.vy[i]);
        }
    }
}

pub fn energy(state: &mut State) {
    let mut total = 0.0_f32;
    for chunk in 0..state.chunk_count() {
        total += chunk_energy(state, chunk);
    }
    state.energy = total;
}

/// The names of the phases the sequential reference times, one per system.
pub const SEQUENTIAL_PHASES: [&str; 8] = [
    "spatial_hashing",
    "repulsion",
    "wander",
    "integration",
    "boundary",
    "energy",
    "structural_changes",
    "hash",
];

/// Wall-clock accounting per phase, in nanoseconds, plus the total.
#[derive(Clone, Debug)]
pub struct Timing {
    pub phases: Vec<(&'static str, u64)>,
    pub total: Duration,
}

impl Timing {
    pub fn new(phases: &[&'static str]) -> Timing {
        Timing { phases: phases.iter().map(|name| (*name, 0)).collect(), total: Duration::ZERO }
    }

    /// Runs `work`, charging its duration to the named phase.
    pub fn charge<T>(&mut self, phase: &'static str, work: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let result = work();
        let elapsed = start.elapsed().as_nanos() as u64;
        let slot = self.phases.iter_mut().find(|(name, _)| *name == phase);
        slot.expect("phase registered in Timing::new").1 += elapsed;
        result
    }
}

/// One tick of the sequential reference, every system timed.
pub fn tick(state: &mut State, grid: &mut Grid, tick: u32, wander_on: bool, timing: &mut Timing) {
    timing.charge("spatial_hashing", || grid.build(state));
    timing.charge("repulsion", || repulsion(state, grid));
    if wander_on {
        timing.charge("wander", || wander(state, tick));
    }
    timing.charge("integration", || integration(state));
    timing.charge("boundary", || boundary(state));
    timing.charge("energy", || energy(state));
    timing.charge("structural_changes", || state.structural_changes());
}

/// The outcome of a run: the tick hashes `h_0` to `h_T`, the run hash `H`
/// and the timing of the `T` ticks.
#[derive(Clone, Debug)]
pub struct Run {
    pub parameters: Parameters,
    pub tick_hashes: Vec<u64>,
    pub run_hash: u64,
    pub timing: Timing,
}

impl Run {
    pub fn initial_hash(&self) -> u64 {
        self.tick_hashes[0]
    }

    pub fn final_hash(&self) -> u64 {
        *self.tick_hashes.last().expect("a run has at least the initial hash")
    }

    pub fn ticks_per_second(&self) -> f64 {
        f64::from(self.parameters.tick_count) / self.timing.total.as_secs_f64()
    }
}

/// The sequential reference run: `WARM_UP_TICKS` ticks of a throwaway
/// instance warm the machine up, then the `T` ticks of the measured instance
/// are hashed and timed.
pub fn run_sequential(parameters: &Parameters) -> Run {
    let mut grid = Grid::new();
    let mut warm_up = State::initialize(parameters);
    let mut discarded = Timing::new(&SEQUENTIAL_PHASES);
    for t in 1..=WARM_UP_TICKS {
        tick(&mut warm_up, &mut grid, t, parameters.wander, &mut discarded);
    }

    let mut state = State::initialize(parameters);
    let mut timing = Timing::new(&SEQUENTIAL_PHASES);
    let mut tick_hashes = Vec::with_capacity(parameters.tick_count as usize + 1);
    let start = Instant::now();
    tick_hashes.push(timing.charge("hash", || state.hash()));
    for t in 1..=parameters.tick_count {
        tick(&mut state, &mut grid, t, parameters.wander, &mut timing);
        tick_hashes.push(timing.charge("hash", || state.hash()));
    }
    timing.total = start.elapsed();
    let run_hash = run_hash(&tick_hashes);
    Run { parameters: *parameters, tick_hashes, run_hash, timing }
}

/// The run hash `H`: FNV-1a 64 over the tick hashes as little-endian bytes.
pub fn run_hash(tick_hashes: &[u64]) -> u64 {
    let mut hasher = Fnv1a::new();
    for hash in tick_hashes {
        hasher.write(&hash.to_le_bytes());
    }
    hasher.finish()
}

/// The first tick whose hash differs between two runs, if any.
pub fn first_divergent_tick(a: &[u64], b: &[u64]) -> Option<usize> {
    (0..a.len().max(b.len())).find(|&t| a.get(t) != b.get(t))
}

pub struct Fnv1a(u64);

impl Fnv1a {
    pub fn new() -> Fnv1a {
        Fnv1a(FNV_OFFSET_BASIS)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
        }
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Fnv1a {
    fn default() -> Fnv1a {
        Fnv1a::new()
    }
}

pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.write(bytes);
    hasher.finish()
}

/// Minimal JSON writer: flat objects, arrays, strings, integers, decimals,
/// booleans and nested objects, enough for a run report without a dependency.
pub struct Json {
    buf: String,
    needs_comma: bool,
}

impl Default for Json {
    fn default() -> Json {
        Json::new()
    }
}

impl Json {
    pub fn new() -> Json {
        Json { buf: String::new(), needs_comma: false }
    }

    pub fn object_start(&mut self) {
        self.separator();
        self.buf.push('{');
        self.needs_comma = false;
    }

    pub fn object_end(&mut self) {
        self.buf.push('}');
        self.needs_comma = true;
    }

    pub fn array_start(&mut self) {
        self.separator();
        self.buf.push('[');
        self.needs_comma = false;
    }

    pub fn array_end(&mut self) {
        self.buf.push(']');
        self.needs_comma = true;
    }

    pub fn key(&mut self, key: &str) {
        self.separator();
        self.quoted(key);
        self.buf.push(':');
        self.needs_comma = false;
    }

    pub fn string(&mut self, key: &str, value: &str) {
        self.key(key);
        self.quoted(value);
        self.needs_comma = true;
    }

    pub fn u64(&mut self, key: &str, value: u64) {
        self.key(key);
        self.buf.push_str(&value.to_string());
        self.needs_comma = true;
    }

    pub fn i64(&mut self, key: &str, value: i64) {
        self.key(key);
        self.buf.push_str(&value.to_string());
        self.needs_comma = true;
    }

    /// A decimal with three digits after the point (durations, rates).
    pub fn decimal(&mut self, key: &str, value: f64) {
        self.key(key);
        self.buf.push_str(&format!("{value:.3}"));
        self.needs_comma = true;
    }

    pub fn bool(&mut self, key: &str, value: bool) {
        self.key(key);
        self.buf.push_str(if value { "true" } else { "false" });
        self.needs_comma = true;
    }

    pub fn hex(&mut self, key: &str, bits: u64) {
        self.string(key, &format!("0x{bits:016x}"));
    }

    fn separator(&mut self) {
        if self.needs_comma {
            self.buf.push(',');
        }
    }

    fn quoted(&mut self, text: &str) {
        self.buf.push('"');
        for character in text.chars() {
            match character {
                '"' => self.buf.push_str("\\\""),
                '\\' => self.buf.push_str("\\\\"),
                control if (control as u32) < 0x20 => {
                    self.buf.push_str(&format!("\\u{:04x}", control as u32));
                }
                other => self.buf.push(other),
            }
        }
        self.buf.push('"');
    }

    pub fn finish(self) -> String {
        self.buf
    }
}

/// The platform and build facts every workload report carries, in the shape
/// of the platform facts spike so that reports read alike.
pub fn write_platform_and_build(out: &mut Json) {
    out.key("platform");
    out.object_start();
    out.string("os", std::env::consts::OS);
    out.string("arch", std::env::consts::ARCH);
    out.u64(
        "available_parallelism",
        std::thread::available_parallelism().map_or(0, |n| n.get() as u64),
    );
    out.object_end();
    out.key("build");
    out.object_start();
    out.bool("debug_assertions", cfg!(debug_assertions));
    out.string(
        "target_env",
        if cfg!(target_env = "msvc") {
            "msvc"
        } else if cfg!(target_env = "gnu") {
            "gnu"
        } else {
            ""
        },
    );
    out.key("compile_time_target_features");
    out.object_start();
    out.bool("sse2", cfg!(target_feature = "sse2"));
    out.bool("avx2", cfg!(target_feature = "avx2"));
    out.bool("fma", cfg!(target_feature = "fma"));
    out.bool("neon", cfg!(target_feature = "neon"));
    out.object_end();
    out.object_end();
}

/// The parameters and hashes of a run, as the specification's "What a spike
/// reports" lists them. The caller adds the fields of its own experiment.
pub fn write_run_core(out: &mut Json, run: &Run) {
    let p = &run.parameters;
    out.u64("workload_version", u64::from(VERSION));
    out.string("size", p.size.name());
    out.u64("entity_count", p.entity_count() as u64);
    out.u64("tick_count", u64::from(p.tick_count));
    out.hex("seed", p.seed);
    out.bool("wander", p.wander);
    out.hex("run_hash", run.run_hash);
    out.hex("initial_hash", run.initial_hash());
    out.hex("final_hash", run.final_hash());
    out.u64("warm_up_ticks", u64::from(WARM_UP_TICKS));
    out.decimal("total_ms", run.timing.total.as_secs_f64() * 1_000.0);
    out.decimal("ticks_per_second", run.ticks_per_second());
    out.key("phase_ms");
    out.object_start();
    for (phase, nanos) in &run.timing.phases {
        out.decimal(phase, *nanos as f64 / 1_000_000.0);
    }
    out.object_end();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(ticks: u32) -> Parameters {
        Parameters::new(Size::Small, ticks, false)
    }

    #[test]
    fn splitmix64_matches_the_reference_sequence() {
        let mut stream = SplitMix64::new(0);
        assert_eq!(stream.next(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(stream.next(), 0x6e78_9e6a_a1b9_65f4);
        assert_eq!(stream.next(), 0x06c4_5d18_8009_454f);
        assert_eq!(stream.next(), 0xf88b_b8a8_724c_81ec);
        let mut stream = SplitMix64::new(DEFAULT_SEED);
        assert_eq!(stream.next(), 0x1101_79df_a6c6_8a0f);
    }

    #[test]
    fn uniform_draws_are_exact_and_in_range() {
        let mut stream = SplitMix64::new(DEFAULT_SEED);
        for _ in 0..10_000 {
            let u = stream.u01();
            assert!((0.0..1.0).contains(&u));
            let scaled = u * 16_777_216.0;
            assert_eq!(scaled, scaled.trunc(), "24 significant bits at most");
        }
    }

    #[test]
    fn fnv1a_matches_the_reference_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn state_hash_follows_the_byte_layout() {
        let mut state = State::initialize(&Parameters::new(Size::Small, 0, false));
        state.generation.truncate(2);
        state.alive.truncate(2);
        state.x.truncate(2);
        state.y.truncate(2);
        state.vx.truncate(2);
        state.vy.truncate(2);
        state.mass.truncate(2);
        state.generation[1] = 7;
        state.alive[1] = false;
        state.energy = 1.5;
        state.rng.state = 0x0102_0304_0506_0708;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.push(1);
        for value in [state.x[0], state.y[0], state.vx[0], state.vy[0], state.mass[0]] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&7_u32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&1.5_f32.to_le_bytes());
        bytes.extend_from_slice(&0x0102_0304_0506_0708_u64.to_le_bytes());
        assert_eq!(state.hash(), fnv1a(&bytes));
    }

    /// The hashes below were computed by `oracle.py`, an independent
    /// implementation of the specification in Python that rounds every
    /// operation to binary32 through the IEEE 754 encoding.
    #[test]
    fn small_run_matches_the_independent_implementation() {
        let run = run_sequential(&small(10));
        assert_eq!(run.initial_hash(), 0x1c64_5a13_a62c_9cd9);
        assert_eq!(run.tick_hashes[1], 0x1ca2_9161_8326_fc18);
        assert_eq!(run.final_hash(), 0x3bc8_55b8_1d68_5e28);
        assert_eq!(run.run_hash, 0xe647_eae6_c979_ab97);
    }

    #[test]
    fn longer_small_run_matches_the_independent_implementation() {
        let run = run_sequential(&small(100));
        assert_eq!(run.tick_hashes[99], 0x4074_eeb2_a829_ce24);
        assert_eq!(run.final_hash(), 0x9710_4971_5293_bfd0);
        assert_eq!(run.run_hash, 0x66c0_8d4b_074a_af88);
    }

    #[test]
    fn default_run_matches_the_independent_implementation() {
        let run = run_sequential(&Parameters::new(Size::Default, 5, false));
        assert_eq!(run.initial_hash(), 0x8680_05e0_d753_a5ae);
        assert_eq!(run.tick_hashes[1], 0x3511_fd84_fe5a_ff37);
        assert_eq!(run.tick_hashes[2], 0x2770_5c65_7b2d_b3ca);
        assert_eq!(run.tick_hashes[4], 0xdf82_80ae_365d_84ed);
        assert_eq!(run.final_hash(), 0x9220_e922_4813_6314);
        assert_eq!(run.run_hash, 0xc2e4_e236_e597_bffb);
    }

    #[test]
    fn two_sequential_runs_agree_with_and_without_wander() {
        for wander in [false, true] {
            let parameters = Parameters::new(Size::Small, 30, wander);
            let first = run_sequential(&parameters);
            let second = run_sequential(&parameters);
            assert_eq!(first.tick_hashes, second.tick_hashes);
            assert_eq!(first.run_hash, second.run_hash);
        }
        let off = run_sequential(&small(5));
        let on = run_sequential(&Parameters::new(Size::Small, 5, true));
        assert_eq!(first_divergent_tick(&off.tick_hashes, &on.tick_hashes), Some(1));
    }

    #[test]
    fn positions_stay_inside_the_world_and_slots_turn_over() {
        let parameters = small(200);
        let mut state = State::initialize(&parameters);
        let mut grid = Grid::new();
        let mut timing = Timing::new(&SEQUENTIAL_PHASES);
        for t in 1..=parameters.tick_count {
            tick(&mut state, &mut grid, t, true, &mut timing);
            for slot in 0..state.len() {
                if state.alive[slot] {
                    assert!((0.0..WORLD_SIDE).contains(&state.x[slot]));
                    assert!((0.0..WORLD_SIDE).contains(&state.y[slot]));
                    assert!(state.vx[slot].abs() <= SPEED_LIMIT);
                    assert!(state.vy[slot].abs() <= SPEED_LIMIT);
                    let (cx, cy) = Grid::cell_of(state.x[slot], state.y[slot]);
                    assert!(cx < GRID_SIDE && cy < GRID_SIDE);
                }
            }
        }
        assert!(state.generation.iter().any(|&g| g > 0), "spawns reuse killed slots");
        assert!(state.alive.iter().filter(|&&alive| alive).count() <= state.len());
        assert!(state.energy > 0.0);
    }

    #[test]
    fn bounce_reflects_once_and_keeps_the_edge_inside() {
        assert_eq!(bounced(-0.5, -3.0), (0.5, 3.0));
        assert_eq!(bounced(1024.5, 2.0), (1023.5, -2.0));
        assert_eq!(bounced(1024.0, 2.0), (WORLD_SIDE - 0.0625, -2.0));
        assert_eq!(bounced(512.0, 2.0), (512.0, 2.0));
    }

    #[test]
    fn json_writer_escapes_and_separates() {
        let mut out = Json::new();
        out.object_start();
        out.string("a", "x\"y\\z");
        out.key("b");
        out.array_start();
        out.object_start();
        out.u64("c", 1);
        out.bool("d", false);
        out.i64("e", -1);
        out.decimal("f", 2.5);
        out.object_end();
        out.array_end();
        out.hex("g", 0xff);
        out.object_end();
        assert_eq!(
            out.finish(),
            r#"{"a":"x\"y\\z","b":[{"c":1,"d":false,"e":-1,"f":2.500}],"g":"0x00000000000000ff"}"#
        );
    }
}
