//! The bound above which work at load leaves the executor: the row of the
//! stand-in sheet judged on a magnitude, `load-bound`. Throwaway, never
//! product code.
//!
//! The claim: work at load above one millisecond belongs on the background
//! pool. It has inputs and no oracle, and no control. One world wants a root
//! whose closure holds a large part and a part whose form is built at load;
//! the instance stands under a bound of a tenth of a millisecond, of one
//! millisecond or of ten, with its background pool idle or saturated by a
//! stream of synthetic import actions. Two things are measured: the latency
//! of the executor's reads behind a computation, by read requests the
//! harness sends the store port every 250 microseconds while the world
//! waits, and the wall time the gated world waits for the batch of its
//! closure. The row is refuted, at one millisecond, by a read delayed by
//! more than one millisecond behind a computation, or by a gated world that
//! waits more than twice the time it waits with the pool idle.
//!
//! Each repetition of a case is a new process, with an empty resident store
//! and an empty cache of forms: the mode starts it, and reads what it
//! prints. The figures are the stand-in executor's.

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use asset_loader::{Attachment, LoadBound, Loader, Machine, Options, Place, RATE_BUFFER, Ran, Rates, Served};
use asset_pipeline::{Cache, Source};
use content_store::content_hash;
use prototype_core::forms::{Preparer, grind, local_platform};
use prototype_core::table::{AssetId, AssetType, Availability, Command, Maker, Owner, Part, Policy, Residents};
use prototype_core::world::{Extraction, Links, Mode, Size, Wait, World, WorldConfig};
use reference_workload::Json;

use crate::magnitude::{arguments_or_exit, conditions, finish, head, median, nanoseconds, write_ratio};
use crate::run::import;

/// The three bounds of the family, each with the name its cases carry.
const BOUNDS: [(&str, Duration); 3] =
    [("tenth-of-a-millisecond", Duration::from_micros(100)), ("one-millisecond", Duration::from_millis(1)), ("ten-milliseconds", Duration::from_millis(10))];
/// The bound of the claim, and of its refutation.
const BOUND_OF_THE_CLAIM: Duration = Duration::from_millis(1);
/// The worker threads of the background pool, and of the world.
const POOL_WORKERS: usize = 4;
const WORLD_WORKERS: usize = 4;
/// The file threads of the executor, as the loading capability starts them.
const FILE_THREADS: usize = 2;
/// The harness sends the store port one read request every 250 microseconds.
const PROBE_PERIOD: Duration = Duration::from_micros(250);
/// With the pool saturated, at least eight actions are queued behind the
/// running ones at every moment of a measurement: the stream keeps fourteen,
/// since four workers may each take one between two looks at the queue.
const QUEUED_AT_LEAST: usize = 8;
const QUEUED_KEPT: usize = 14;
/// A large piece of work that measures under two milliseconds alone does not
/// show the bound of one millisecond.
const SHOWS_THE_BOUND_FROM: Duration = Duration::from_millis(2);
/// How long a case waits for the batch before it gives up.
const PATIENCE: Duration = Duration::from_secs(120);
const HOST: Owner = Owner::World(Maker::Source(3));

const R: AssetId = 0x0052_0000_0000_0000_0000_0000_0000_0001;
const B: AssetId = 0x0042_0000_0000_0000_0000_0000_0000_0001;
const F: AssetId = 0x0046_0000_0000_0000_0000_0000_0000_0001;

/// The identifier of one of the small assets that stand outside the closure.
fn small(index: usize) -> AssetId {
    0x0053_0000_0000_0000_0000_0000_0000_0000 + index as u128 + 1
}

/// The sizes of the content: the root, the large part, the part whose form
/// is built at load, and the count of small assets of 1 024 bytes; with the
/// size of the reference workload the world steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    pub root: usize,
    pub large: usize,
    pub formed: usize,
    pub small_assets: usize,
    pub size: Size,
}

impl Scale {
    /// The sizes the sheet states.
    pub const SHEET: Scale = Scale { root: 1_024, large: 33_554_432, formed: 8_388_608, small_assets: 200, size: Size::Default };
    /// The sizes of the self-check, which judges nothing.
    pub const SMALL: Scale = Scale { root: 1_024, large: 262_144, formed: 65_536, small_assets: 20, size: Size::Small };

    pub fn of(small: bool) -> Scale {
        if small { Scale::SMALL } else { Scale::SHEET }
    }
}

/// Writes the content of the family as the sources of a content root and
/// imports it into a loose store, each part padded by the stand-in pipeline
/// to the size stated: `R.1` lists `B` under the first type and `F` under
/// `second`, and the small assets stand outside its closure. Returns the
/// directory of the loose store.
pub fn content(directory: &Path, scale: Scale) -> PathBuf {
    let root = directory.join("content");
    fs::create_dir_all(&root).expect("a content root is created");
    let source = |asset: AssetId, asset_type: AssetType, salt: u32, size: usize, dependencies: Vec<(AssetId, AssetType)>| Source {
        asset,
        asset_type,
        strength: 24.0 + salt as f32 % 64.0,
        radius: 8.0,
        salt,
        size: Some(size),
        manifest_size: None,
        dependencies,
    };
    let write = |name: &str, source: Source| fs::write(root.join(format!("{name}.asset")), source.text()).expect("a source is written");
    write("R", source(R, AssetType::Parameters, 1, scale.root, vec![(B, AssetType::Parameters), (F, AssetType::Second)]));
    write("B", source(B, AssetType::Parameters, 2, scale.large, Vec::new()));
    write("F", source(F, AssetType::Second, 3, scale.formed, Vec::new()));
    for index in 0..scale.small_assets {
        write(&format!("S{index:03}"), source(small(index), AssetType::Parameters, 10 + index as u32, 1_024, Vec::new()));
    }
    let store = directory.join("store");
    import(&root, &store);
    store
}

/// One read request of the harness, as the executor served it: the wait,
/// from the request to the moment a file thread started the read, and, of
/// that wait, the time during which every file thread was running a
/// computation and not a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    pub wait: Duration,
    pub behind_a_computation: Duration,
}

/// One piece of work at load, as a case ran it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub work: &'static str,
    pub content: &'static str,
    pub size: usize,
    pub estimate: Duration,
    pub duration: Duration,
    /// Whether it ran as an action of the pool; on a file thread otherwise.
    pub action: bool,
    /// The worker of the pool, or the file thread, that ran it.
    pub on: usize,
}

/// What one case gave, in one process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Played {
    /// The wall time the gated world waited: from the end of tick 1, when
    /// the want left the domain, to the moment the batch of the closure of
    /// `R` stood on the world's queue.
    pub wall: Duration,
    pub probes: Vec<Probe>,
    pub pieces: Vec<Piece>,
    /// The rates the instance measured when it started, in bytes per
    /// millisecond: verification, validation, form.
    pub rates: [f64; 3],
    /// The passes of the content hash one synthetic import action runs.
    pub import_passes: usize,
    /// The import actions submitted, and the fewest actions the stream saw
    /// queued behind the running ones while the world waited; none with the
    /// pool idle.
    pub imports: usize,
    pub fewest_queued: Option<usize>,
    /// A step without waiting, right after tick 1, ran no tick.
    pub gated: bool,
    /// The world was as stated: an empty table at tick 0, tick 2 run once
    /// the batch was posted, `R`, `B` and `F` available, and the form of
    /// `F` resident, as many bytes as its part.
    pub as_stated: bool,
}

fn largest(durations: impl Iterator<Item = Duration>) -> Duration {
    durations.max().unwrap_or(Duration::ZERO)
}

/// The 99th percentile of a set of durations: the smallest of them that at
/// least 99 in a hundred do not exceed.
fn percentile_99(durations: impl Iterator<Item = Duration>) -> Duration {
    let mut sorted: Vec<Duration> = durations.collect();
    sorted.sort_unstable();
    match sorted.len() {
        0 => Duration::ZERO,
        count => sorted[(count * 99).div_ceil(100).max(1) - 1],
    }
}

impl Played {
    pub fn largest_wait(&self) -> Duration {
        largest(self.probes.iter().map(|probe| probe.wait))
    }

    pub fn wait_at_the_99th(&self) -> Duration {
        percentile_99(self.probes.iter().map(|probe| probe.wait))
    }

    pub fn largest_delay(&self) -> Duration {
        largest(self.probes.iter().map(|probe| probe.behind_a_computation))
    }

    pub fn delay_at_the_99th(&self) -> Duration {
        percentile_99(self.probes.iter().map(|probe| probe.behind_a_computation))
    }
}

/// The moments during which every file thread was running a computation:
/// the intersection of the computations of the two file threads.
fn every_file_thread_computing(ran: &[Ran]) -> Vec<(Instant, Instant)> {
    let of = |thread: usize| ran.iter().filter(move |piece| piece.place == Place::FileThread(thread)).map(|piece| (piece.started, piece.ended));
    let mut together: Vec<(Instant, Instant)> = of(0).collect();
    for thread in 1..FILE_THREADS {
        let others: Vec<(Instant, Instant)> = of(thread).collect();
        together = together
            .iter()
            .flat_map(|(from, to)| others.iter().map(move |(other_from, other_to)| ((*from).max(*other_from), (*to).min(*other_to))))
            .filter(|(from, to)| from < to)
            .collect();
    }
    together
}

/// Of a wait, the time that stands inside the moments given.
fn inside(wait: (Instant, Instant), moments: &[(Instant, Instant)]) -> Duration {
    moments.iter().map(|(from, to)| wait.1.min(*to).saturating_duration_since(wait.0.max(*from))).sum()
}

type Extract = Arc<dyn Fn(Extraction) + Send + Sync>;

/// The stream of synthetic import actions: each one computation of about
/// two milliseconds, passes of the content hash over a buffer of 1 048 576
/// bytes, submitted to the pool so that actions stay queued behind the
/// running ones. Returns, once stopped, how many it submitted and the
/// fewest it saw queued while the world waited.
fn saturate(loader: &Loader, stop: Arc<AtomicBool>, waiting: Arc<OnceLock<Instant>>) -> thread::JoinHandle<(usize, Option<usize>)> {
    let bounded = loader.bounded().expect("an instance under a bound");
    let (pool, passes) = (bounded.pool(), bounded.rates.import_passes);
    let buffer = Arc::new(vec![0x5a_u8; RATE_BUFFER]);
    let stream = move || {
        let (mut submitted, mut fewest) = (0_usize, None::<usize>);
        while !stop.load(Ordering::Relaxed) {
            let queued = pool.queued();
            if waiting.get().is_some() {
                fewest = Some(fewest.map_or(queued, |fewest| fewest.min(queued)));
            }
            for _ in queued..QUEUED_KEPT {
                let buffer = buffer.clone();
                pool.submit(Box::new(move |_| {
                    for _ in 0..passes {
                        black_box(content_hash(black_box(&buffer)));
                    }
                }));
                submitted += 1;
            }
            let until = Instant::now() + Duration::from_micros(50);
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        }
        (submitted, fewest)
    };
    thread::Builder::new().name("import".to_string()).spawn(stream).expect("spawn the stream of import actions")
}

/// Plays one case in this process: one world over the store, under a bound,
/// with the pool idle or saturated. `forms` is the machine's cache of forms,
/// empty when the case starts.
pub fn play(store: &Path, forms: &Path, bound: Duration, saturated: bool, scale: Scale) -> Played {
    let _ = fs::remove_dir_all(forms);
    let revision = Cache::on_disk(store).and_then(|cache| cache.revision()).expect("the revision of the family is in place");
    let hash = |asset: AssetId| revision.domain[&asset].hash;
    let machine = Machine { forms: forms.to_path_buf(), platform: local_platform(), bound: Some(LoadBound { bound, workers: POOL_WORKERS }) };
    let loader = Loader::start_on(store, machine, Options::default()).expect("the loading capability starts");
    let bounded = loader.bounded().expect("an instance under a bound");

    // The world: an empty table at tick 0. The moment its want leaves the
    // domain is kept: the end of the tick that added it, where the world
    // extracts its state.
    let left_at: Arc<OnceLock<Instant>> = Arc::new(OnceLock::new());
    let slot: Arc<OnceLock<Extract>> = Arc::new(OnceLock::new());
    let extract = {
        let (left_at, slot) = (left_at.clone(), slot.clone());
        move |extraction: Extraction| {
            if !extraction.left.is_empty() {
                let _ = left_at.set(Instant::now());
            }
            if let Some(extract) = slot.get() {
                extract(extraction);
            }
        }
    };
    let mut links = Links::new(loader.residents());
    links.extract = Some(Arc::new(extract));
    let queue = links.queue.clone();
    let mut config = WorldConfig::new(scale.size, WORLD_WORKERS, Mode::Identifiers, vec![R]);
    config.asset_types = vec![AssetType::Parameters, AssetType::Second];
    let mut world = World::new(config, Vec::new(), links);
    let mut as_stated = world.entries().is_empty();
    // For this family the composition declares, for the type `second`, one
    // prepared form that is the same computation on every cell.
    let forms_declared = vec![(AssetType::Second, Preparer::Grind)];
    let attachment = Attachment { queue, set: None, entries: world.entries(), unanswered: world.unanswered(), restore: Vec::new(), standing: Vec::new(), checks: Vec::new(), forms: forms_declared };
    let _ = slot.set(loader.attach(attachment).expect("the world is attached"));
    world.post(Command::Want { asset: R, asset_type: AssetType::Parameters, owner: HOST, policy: Policy::Required });
    loader.settle();

    // With the pool saturated, the stream runs before the world steps, and
    // the measurement starts once actions are queued.
    let stop = Arc::new(AtomicBool::new(false));
    let stream = saturated.then(|| saturate(&loader, stop.clone(), left_at.clone()));
    let patience = Instant::now() + PATIENCE;
    while saturated && bounded.pool().queued() < QUEUED_AT_LEAST && Instant::now() < patience {
        thread::yield_now();
    }

    // The read requests of the harness: from the end of tick 1 until the
    // batch is posted, one every 250 microseconds, the small artifacts in
    // turn.
    let sent = Arc::new(AtomicUsize::new(0));
    let prober = {
        let (port, left_at, bounded, stop, sent) = (loader.port(), left_at.clone(), bounded.clone(), stop.clone(), sent.clone());
        let smalls: Vec<u64> = (0..scale.small_assets).map(|index| hash(small(index))).collect();
        let probe = move || -> Vec<Served> {
            let (reply, served) = channel();
            while left_at.get().is_none() && !stop.load(Ordering::Relaxed) {
                std::hint::spin_loop();
            }
            let mut next = left_at.get().copied().unwrap_or_else(Instant::now);
            while bounded.batches() == 0 && !stop.load(Ordering::Relaxed) {
                let turn = sent.fetch_add(1, Ordering::Relaxed);
                port.request(smalls[turn % smalls.len()], &reply);
                next += PROBE_PERIOD;
                while Instant::now() < next {
                    std::hint::spin_loop();
                }
            }
            drop(reply);
            served.iter().collect()
        };
        thread::Builder::new().name("probe".to_string()).spawn(probe).expect("spawn the read requests")
    };

    // Tick 1 applies the want: the world is gated from the boundary after it.
    as_stated &= world.step(1, Wait::No).is_ok_and(|result| result.ticks_ran == 1);
    let gated = world.step(1, Wait::No).is_ok_and(|result| result.ticks_ran == 0 && result.gated);
    while bounded.batches() == 0 && Instant::now() < patience {
        thread::sleep(Duration::from_micros(200));
    }
    stop.store(true, Ordering::Relaxed);
    let served = prober.join().expect("the read requests end");
    let (imports, fewest_queued) = stream.map_or((0, None), |stream| stream.join().expect("the stream of import actions ends"));
    loader.settle();
    // The world runs tick 2 once the batch of the closure of `R` is posted.
    if world.tick() < 2 {
        as_stated &= world.step(1, Wait::No).is_ok_and(|result| result.ticks_ran == 1);
    }
    let available = |asset: AssetId| world.entry(asset).is_some_and(|(availability, _)| availability == Availability::Available(hash(asset)));
    let form = loader.residents().form(hash(F), None);
    as_stated &= world.tick() == 2 && available(R) && available(B) && available(F) && form.is_some_and(|form| form.len() == scale.formed);
    as_stated &= served.len() == sent.load(Ordering::Relaxed) && served.iter().all(|served| served.found);

    let ran = bounded.ran();
    let posted = bounded.posted();
    let wall = match (left_at.get(), posted.first()) {
        (Some(left_at), Some(posted)) => posted.saturating_duration_since(*left_at),
        _ => {
            as_stated = false;
            Duration::ZERO
        }
    };
    let computing = every_file_thread_computing(&ran);
    let probes = served
        .iter()
        .map(|served| Probe { wait: served.started.saturating_duration_since(served.requested), behind_a_computation: inside((served.requested, served.started), &computing) })
        .collect();
    let content_of = |piece: &Ran| [(R, "R.1"), (B, "B.1"), (F, "F.1")].into_iter().find(|(asset, _)| hash(*asset) == piece.hash).map_or("another", |(_, name)| name);
    let pieces = ran
        .iter()
        .map(|piece| {
            let (action, on) = match piece.place {
                Place::Action(worker) => (true, worker),
                Place::FileThread(thread) => (false, thread),
            };
            Piece { work: piece.work.name(), content: content_of(piece), size: piece.size, estimate: piece.estimate, duration: piece.ended.saturating_duration_since(piece.started), action, on }
        })
        .collect();
    let Rates { verification, validation, form, import_passes } = bounded.rates;
    drop(world);
    drop(loader);
    let _ = fs::remove_dir_all(forms);
    Played { wall, probes, pieces, rates: [verification, validation, form], import_passes, imports, fewest_queued, gated, as_stated }
}

// ---------------------------------------------------------------------------
// A case as a process of its own
// ---------------------------------------------------------------------------

/// What a case gave, a line per fact, for the process that started it.
fn written(played: &Played) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "wall {}", played.wall.as_nanos());
    let _ = writeln!(out, "rates {} {} {} {}", played.rates[0], played.rates[1], played.rates[2], played.import_passes);
    let _ = writeln!(out, "imports {} {}", played.imports, played.fewest_queued.map_or_else(|| "none".to_string(), |fewest| fewest.to_string()));
    let _ = writeln!(out, "gated {}", played.gated);
    let _ = writeln!(out, "as-stated {}", played.as_stated);
    for probe in &played.probes {
        let _ = writeln!(out, "probe {} {}", probe.wait.as_nanos(), probe.behind_a_computation.as_nanos());
    }
    for piece in &played.pieces {
        let place = if piece.action { "action" } else { "file-thread" };
        let _ = writeln!(out, "piece {} {} {} {} {} {place} {}", piece.work, piece.content, piece.size, piece.estimate.as_nanos(), piece.duration.as_nanos(), piece.on);
    }
    out
}

fn parsed(text: &str) -> Result<Played, String> {
    let mut played = Played::default();
    let nanos = |word: &str| word.parse::<u64>().map(Duration::from_nanos).map_err(|_| format!("not a duration in nanoseconds: {word}"));
    let count = |word: &str| word.parse::<usize>().map_err(|_| format!("not a count: {word}"));
    let rate = |word: &str| word.parse::<f64>().map_err(|_| format!("not a rate: {word}"));
    let named = |word: &str, names: &[&'static str]| names.iter().copied().find(|name| *name == word).ok_or_else(|| format!("unknown name {word}"));
    let mut seen = 0;
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["wall", wall] => played.wall = nanos(wall)?,
            ["rates", verification, validation, form, passes] => {
                played.rates = [rate(verification)?, rate(validation)?, rate(form)?];
                played.import_passes = count(passes)?;
            }
            ["imports", imports, fewest] => {
                played.imports = count(imports)?;
                played.fewest_queued = if *fewest == "none" { None } else { Some(count(fewest)?) };
            }
            ["gated", gated] => played.gated = *gated == "true",
            ["as-stated", as_stated] => played.as_stated = *as_stated == "true",
            ["probe", wait, behind] => played.probes.push(Probe { wait: nanos(wait)?, behind_a_computation: nanos(behind)? }),
            ["piece", work, content, size, estimate, duration, place, on] => played.pieces.push(Piece {
                work: named(work, &["verification", "validation", "form"])?,
                content: named(content, &["R.1", "B.1", "F.1", "another"])?,
                size: count(size)?,
                estimate: nanos(estimate)?,
                duration: nanos(duration)?,
                action: *place == "action",
                on: count(on)?,
            }),
            _ => return Err(format!("not a fact of a case: {line}")),
        }
        seen += usize::from(matches!(words.first(), Some(&("wall" | "rates" | "imports" | "gated" | "as-stated"))));
    }
    if seen == 5 { Ok(played) } else { Err("the case did not print its five facts".to_string()) }
}

/// The process of one case: `--load-bound-case <bound> <idle|saturated>
/// <store> <forms> <small|stated>`, the bound by its place among the three.
pub fn child(arguments: &[String]) -> String {
    let [bound, pool, store, forms, size] = arguments else { panic!("a case is given a bound, a pool, a store, a cache of forms and a size") };
    let (_, bound) = BOUNDS[bound.parse::<usize>().expect("the place of a bound")];
    written(&play(Path::new(store), Path::new(forms), bound, pool == "saturated", Scale::of(size == "small")))
}

fn in_a_process(store: &Path, forms: &Path, bound: usize, saturated: bool, scale: Scale) -> Played {
    let exe = std::env::current_exe().expect("the harness knows its own path");
    let (pool, size) = (if saturated { "saturated" } else { "idle" }, if scale == Scale::SMALL { "small" } else { "stated" });
    let output = Process::new(exe).arg("--load-bound-case").arg(bound.to_string()).arg(pool).arg(store).arg(forms).arg(size).output().expect("a case starts");
    let printed = String::from_utf8_lossy(&output.stdout);
    let played = if output.status.success() { parsed(&printed) } else { Err(format!("it ended with {}", output.status)) };
    played.unwrap_or_else(|error| panic!("the case at the bound {} with the pool {pool} gave nothing to read: {error}\n{}", BOUNDS[bound].0, String::from_utf8_lossy(&output.stderr)))
}

// ---------------------------------------------------------------------------
// The family
// ---------------------------------------------------------------------------

/// One large piece of work, measured alone: no load runs, and no import.
#[derive(Clone, Debug)]
pub struct Alone {
    pub work: &'static str,
    pub content: &'static str,
    pub size: usize,
    pub duration: Duration,
}

impl Alone {
    /// Whether the piece shows the bound of one millisecond: one that
    /// measures under two milliseconds on the machine of the run does not.
    pub fn shows_the_bound(&self) -> bool {
        self.duration >= SHOWS_THE_BOUND_FROM
    }
}

/// The duration, measured alone, of each large piece of work: the
/// verification and the validation of `B.1`, the verification and the
/// validation of `F.1` and the building of its form. The median of five runs.
fn alone(store: &Path, scale: Scale) -> Vec<Alone> {
    let cache = Cache::on_disk(store).expect("the loose store is at hand");
    let revision = cache.revision().expect("the revision of the family is in place");
    let part = |asset: AssetId| cache.object(revision.domain[&asset].hash).expect("the part is in the store");
    let (large, formed) = (part(B), part(F));
    let timed = |run: &dyn Fn()| {
        let mut durations: Vec<Duration> = (0..5)
            .map(|_| {
                let started = Instant::now();
                run();
                started.elapsed()
            })
            .collect();
        median(&mut durations)
    };
    let piece = |work: &'static str, content: &'static str, size: usize, duration: Duration| Alone { work, content, size, duration };
    let verification = |bytes: &[u8]| {
        black_box(content_hash(black_box(bytes)));
    };
    let validation = |bytes: &[u8]| {
        black_box(Part::decode(black_box(bytes)).is_ok());
    };
    let form = |bytes: &[u8]| {
        black_box(grind(black_box(bytes)));
    };
    vec![
        piece("verification", "B.1", scale.large, timed(&|| verification(&large))),
        piece("validation", "B.1", scale.large, timed(&|| validation(&large))),
        piece("verification", "F.1", scale.formed, timed(&|| verification(&formed))),
        piece("validation", "F.1", scale.formed, timed(&|| validation(&formed))),
        piece("form", "F.1", scale.formed, timed(&|| form(&formed))),
    ]
}

/// One case of one repetition.
#[derive(Clone, Debug)]
pub struct Case {
    pub repetition: usize,
    /// The name of the bound, and the bound.
    pub bound: &'static str,
    pub limit: Duration,
    pub saturated: bool,
    pub played: Played,
}

impl Case {
    pub fn name(&self) -> String {
        format!("load-bound/{}-{}", self.bound, if self.saturated { "saturated" } else { "idle" })
    }
}

/// What the family gave over its repetitions.
pub struct LoadBoundRow {
    pub scale: Scale,
    pub alone: Vec<Alone>,
    pub cases: Vec<Case>,
}

impl LoadBoundRow {
    fn case(&self, repetition: usize, limit: Duration, saturated: bool) -> Option<&Case> {
        self.cases.iter().find(|case| case.repetition == repetition && case.limit == limit && case.saturated == saturated)
    }

    /// For one bound and one repetition, the wall time with the pool
    /// saturated over the wall time with the pool idle.
    pub fn ratio(&self, repetition: usize, limit: Duration) -> Option<f64> {
        let (idle, saturated) = (self.case(repetition, limit, false)?, self.case(repetition, limit, true)?);
        (idle.played.wall > Duration::ZERO).then(|| saturated.played.wall.as_secs_f64() / idle.played.wall.as_secs_f64())
    }

    fn repetitions(&self) -> usize {
        self.cases.iter().map(|case| case.repetition + 1).max().unwrap_or(0)
    }

    /// The first half of the refutation: at one millisecond, a read delayed
    /// by more than one millisecond behind a computation.
    pub fn a_read_delayed_behind_a_computation(&self) -> bool {
        self.cases.iter().filter(|case| case.limit == BOUND_OF_THE_CLAIM).any(|case| case.played.largest_delay() > BOUND_OF_THE_CLAIM)
    }

    /// The second half: at one millisecond, a gated world that waits more
    /// than twice the time it waits with the pool idle.
    pub fn a_world_that_waits_twice_as_long(&self) -> bool {
        (0..self.repetitions()).filter_map(|repetition| self.ratio(repetition, BOUND_OF_THE_CLAIM)).any(|ratio| ratio > 2.0)
    }

    pub fn as_stated(&self) -> bool {
        self.cases.iter().all(|case| case.played.as_stated)
    }

    /// Whether the row holds on what was measured.
    pub fn holds(&self) -> bool {
        !self.a_read_delayed_behind_a_computation() && !self.a_world_that_waits_twice_as_long() && self.as_stated()
    }
}

/// Runs the family: the content once, the large pieces of work alone, then
/// `repetitions` times the six cases, each a process of its own where
/// `processes` is set and inside this process otherwise.
pub fn run(repetitions: usize, scale: Scale, scratch: &Path, processes: bool) -> LoadBoundRow {
    let _ = fs::remove_dir_all(scratch);
    let store = content(scratch, scale);
    let alone = alone(&store, scale);
    let mut cases = Vec::new();
    for repetition in 0..repetitions {
        for (place, (bound, limit)) in BOUNDS.into_iter().enumerate() {
            for saturated in [false, true] {
                let forms = scratch.join(format!("forms-{repetition}-{place}-{saturated}"));
                let played = if processes { in_a_process(&store, &forms, place, saturated, scale) } else { play(&store, &forms, limit, saturated, scale) };
                cases.push(Case { repetition, bound, limit, saturated, played });
            }
        }
    }
    LoadBoundRow { scale, alone, cases }
}

fn microseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

/// Writes what the family gave: the facts, the summary and the full log.
pub fn report(row: &LoadBoundRow, repetitions: usize, small: bool) -> (Json, String, String) {
    let (mut out, mut summary, mut log) = (Json::new(), String::new(), String::new());
    out.object_start();
    head(&mut out, "load-bound", repetitions, small);
    out.string("the_executor", "two file threads behind the store port, which serve its requests in the order received");
    out.string("the_background_pool", "four worker threads that run actions in the order submitted");
    out.string("how_a_piece_of_work_is_placed", "by its estimate, its size divided by the rate the instance measured when it started: above the bound it is one action of the pool, and otherwise it runs on the file thread that read the artifact, before that thread takes its next request");
    out.u64("bytes_of_the_root", row.scale.root as u64);
    out.u64("bytes_of_the_large_part", row.scale.large as u64);
    out.u64("bytes_of_the_part_whose_form_is_built_at_load", row.scale.formed as u64);
    out.u64("small_assets_outside_the_closure", row.scale.small_assets as u64);
    out.u64("read_request_period_ns", nanoseconds(PROBE_PERIOD));
    out.bool("at_one_millisecond_a_read_delayed_by_more_than_one_millisecond_behind_a_computation", row.a_read_delayed_behind_a_computation());
    out.bool("at_one_millisecond_a_gated_world_that_waits_more_than_twice_as_long", row.a_world_that_waits_twice_as_long());
    out.bool("every_world_as_stated", row.as_stated());
    out.bool("holds", row.holds());
    let _ = writeln!(log, "== the bound above which work at load leaves the executor");

    out.key("large_pieces_of_work_measured_alone");
    out.array_start();
    for piece in &row.alone {
        out.object_start();
        out.string("work", piece.work);
        out.string("content", piece.content);
        out.u64("bytes", piece.size as u64);
        out.u64("median_ns", nanoseconds(piece.duration));
        out.bool("under_two_milliseconds_and_so_not_showing_the_bound", !piece.shows_the_bound());
        out.object_end();
        let shows = if piece.shows_the_bound() { "" } else { ": under two milliseconds, so the bound of one millisecond is not shown by it" };
        let _ = writeln!(log, "  alone, the {} of {} ({} bytes): {:.3} ms{shows}", piece.work, piece.content, piece.size, milliseconds(piece.duration));
    }
    out.array_end();

    out.key("cases");
    out.array_start();
    for case in &row.cases {
        let played = &case.played;
        out.object_start();
        out.u64("repetition", case.repetition as u64 + 1);
        out.string("case", &case.name());
        out.u64("bound_ns", nanoseconds(case.limit));
        out.bool("pool_saturated", case.saturated);
        out.u64("wall_ns", nanoseconds(played.wall));
        out.u64("read_requests", played.probes.len() as u64);
        out.u64("largest_delay_behind_a_computation_ns", nanoseconds(played.largest_delay()));
        out.u64("delay_behind_a_computation_at_the_99th_percentile_ns", nanoseconds(played.delay_at_the_99th()));
        out.u64("largest_wait_ns", nanoseconds(played.largest_wait()));
        out.u64("wait_at_the_99th_percentile_ns", nanoseconds(played.wait_at_the_99th()));
        out.decimal("verification_bytes_per_ms", played.rates[0]);
        out.decimal("validation_bytes_per_ms", played.rates[1]);
        out.decimal("form_bytes_per_ms", played.rates[2]);
        out.u64("passes_of_one_import_action", played.import_passes as u64);
        out.u64("import_actions_submitted", played.imports as u64);
        out.string("fewest_actions_queued_while_the_world_waited", &played.fewest_queued.map_or_else(|| "none: the pool is idle".to_string(), |fewest| fewest.to_string()));
        out.bool("a_step_right_after_tick_1_ran_no_tick", played.gated);
        out.bool("as_stated", played.as_stated);
        out.key("pieces_of_work");
        out.array_start();
        for piece in &played.pieces {
            out.object_start();
            out.string("work", piece.work);
            out.string("content", piece.content);
            out.u64("bytes", piece.size as u64);
            out.u64("estimate_ns", nanoseconds(piece.estimate));
            out.u64("measured_ns", nanoseconds(piece.duration));
            out.string("ran", if piece.action { "as an action of the pool" } else { "on a file thread" });
            out.u64(if piece.action { "worker" } else { "file_thread" }, piece.on as u64);
            out.object_end();
        }
        out.array_end();
        out.object_end();
        let _ = writeln!(
            log,
            "  repetition {} {:<45} the world waited {:>9.3} ms; {} read requests: delay behind a computation largest {:>8.1} us, 99th percentile {:>8.1} us; whole wait largest {:>8.1} us, 99th percentile {:>8.1} us; \
             a step right after tick 1 ran no tick: {}; import actions submitted {}, fewest queued {}; as stated: {}",
            case.repetition + 1,
            case.name(),
            milliseconds(played.wall),
            played.probes.len(),
            microseconds(played.largest_delay()),
            microseconds(played.delay_at_the_99th()),
            microseconds(played.largest_wait()),
            microseconds(played.wait_at_the_99th()),
            played.gated,
            played.imports,
            played.fewest_queued.map_or_else(|| "none".to_string(), |fewest| fewest.to_string()),
            played.as_stated
        );
        for piece in &played.pieces {
            let place = if piece.action { format!("as an action, on the worker {}", piece.on) } else { format!("on the file thread {}", piece.on) };
            let _ = writeln!(
                log,
                "      the {:<12} of {} ({:>9} bytes): estimate {:>10.4} ms, measured {:>10.4} ms, {place}",
                piece.work,
                piece.content,
                piece.size,
                milliseconds(piece.estimate),
                milliseconds(piece.duration)
            );
        }
    }
    out.array_end();

    out.key("wall_time_saturated_over_idle");
    out.array_start();
    for (bound, limit) in BOUNDS {
        for repetition in 0..row.repetitions() {
            let Some(ratio) = row.ratio(repetition, limit) else { continue };
            out.object_start();
            out.u64("repetition", repetition as u64 + 1);
            out.string("bound", bound);
            write_ratio(&mut out, "ratio", ratio);
            out.object_end();
        }
    }
    out.array_end();
    out.object_end();

    let _ = writeln!(summary, "the bound above which work at load leaves the executor, {}", conditions(repetitions, small));
    let _ = writeln!(summary, "  every repetition of a case a process of its own; the pool runs its actions in the order submitted");
    for piece in &row.alone {
        let shows = if piece.shows_the_bound() { "" } else { " (under two milliseconds: the bound of one millisecond is not shown by it)" };
        let _ = writeln!(summary, "  alone, the {} of {}: {:.3} ms{shows}", piece.work, piece.content, milliseconds(piece.duration));
    }
    for (bound, limit) in BOUNDS {
        let of = |saturated: bool| row.cases.iter().filter(move |case| case.limit == limit && case.saturated == saturated);
        let walls = |saturated: bool| of(saturated).map(|case| format!("{:.2}", milliseconds(case.played.wall))).collect::<Vec<_>>().join(", ");
        let delay = largest(row.cases.iter().filter(|case| case.limit == limit).map(|case| case.played.largest_delay()));
        let wait = largest(row.cases.iter().filter(|case| case.limit == limit).map(|case| case.played.largest_wait()));
        let ratios: Vec<String> = (0..row.repetitions()).filter_map(|repetition| row.ratio(repetition, limit)).map(|ratio| format!("{ratio:.3}")).collect();
        let placed = of(false).next().map_or_else(String::new, |case| {
            let on = |action: bool| case.played.pieces.iter().filter(|piece| piece.action == action).map(|piece| format!("{} of {}", piece.work, piece.content)).collect::<Vec<_>>().join(", ");
            format!("; as actions [{}], on a file thread [{}]", on(true), on(false))
        });
        let _ = writeln!(
            summary,
            "  {bound}: the world waited, per repetition, idle [{}] ms and saturated [{}] ms, ratios [{}]; the largest delay of a read behind a computation {:.1} us, the largest whole wait {:.1} us{placed}",
            walls(false),
            walls(true),
            ratios.join(", "),
            microseconds(delay),
            microseconds(wait)
        );
    }
    let _ = writeln!(
        summary,
        "  at one millisecond: a read delayed by more than one millisecond behind a computation: {}; a gated world that waits more than twice the time it waits with the pool idle: {}; every world as stated: {}",
        row.a_read_delayed_behind_a_computation(),
        row.a_world_that_waits_twice_as_long(),
        row.as_stated()
    );
    (out, summary, log)
}

/// The mode `--load-bound [repetitions] [small]`.
pub fn mode(arguments: &[String]) {
    let (repetitions, small) = arguments_or_exit("--load-bound", arguments);
    let scratch = std::env::temp_dir().join(format!("asset-table-and-loader-load-bound-{}", std::process::id()));
    let row = run(repetitions, Scale::of(small), &scratch, true);
    let _ = fs::remove_dir_all(&scratch);
    let (facts, summary, log) = report(&row, repetitions, small);
    finish("load-bound", facts, &summary, &log, &format!("holds: {}", row.holds()));
}

#[cfg(test)]
mod tests {
    use asset_loader::Work;

    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("asset-table-and-loader-load-bound-test-{tag}-{}", std::process::id()))
    }

    /// One case at the small size, under a bound that leaves no piece of
    /// work on the executor and under one that leaves every piece there:
    /// the world is as stated either way, each artifact of the closure is
    /// verified and validated once and the form of `F` built once, and each
    /// piece ran where the bound placed it. No figure is judged.
    #[test]
    fn a_case_runs_at_its_small_size_under_a_bound_that_moves_every_piece_and_under_one_that_moves_none() {
        let scratch = scratch("case");
        let _ = fs::remove_dir_all(&scratch);
        let store = content(&scratch, Scale::SMALL);
        let stated = |played: &Played| {
            assert!(played.as_stated, "{played:?}");
            assert!(played.wall > Duration::ZERO);
            let of = |work: &str| played.pieces.iter().filter(|piece| piece.work == work).map(|piece| piece.content).collect::<Vec<_>>();
            let mut verified = of("verification");
            verified.sort_unstable();
            assert_eq!(verified, ["B.1", "F.1", "R.1"]);
            assert_eq!(of("validation").len(), 3);
            assert_eq!(of("form"), ["F.1"], "the form is built for the one part whose type has it");
            assert!(played.rates.iter().all(|rate| *rate > 0.0) && played.import_passes >= 1);
        };
        let moved = play(&store, &scratch.join("forms-moved"), Duration::ZERO, true, Scale::SMALL);
        stated(&moved);
        assert!(moved.pieces.iter().all(|piece| piece.action && piece.on < POOL_WORKERS), "{:?}", moved.pieces);
        assert!(moved.imports > 0 && moved.fewest_queued.is_some(), "with the pool saturated, import actions are submitted");
        assert!(moved.probes.iter().all(|probe| probe.behind_a_computation == Duration::ZERO), "no file thread runs a computation");
        let left = play(&store, &scratch.join("forms-left"), Duration::from_secs(3_600), false, Scale::SMALL);
        stated(&left);
        assert!(left.pieces.iter().all(|piece| !piece.action && piece.on < FILE_THREADS), "{:?}", left.pieces);
        assert_eq!((left.imports, left.fewest_queued), (0, None), "with the pool idle none is submitted");
        assert!(left.probes.iter().all(|probe| probe.behind_a_computation <= probe.wait));
        // What a case prints reads back.
        assert_eq!(parsed(&written(&left)), Ok(left));
        assert!(parsed("wall 12\n").is_err());
        let _ = fs::remove_dir_all(&scratch);
    }

    /// The family at its small size, one repetition, the six cases inside
    /// the test process: every world is as stated, each bound has its two
    /// cases and its ratio, and the report is written. No figure is judged.
    #[test]
    fn the_mode_runs_at_its_small_size() {
        let scratch = scratch("mode");
        let row = run(1, Scale::SMALL, &scratch, false);
        let _ = fs::remove_dir_all(&scratch);
        assert!(row.as_stated());
        assert_eq!(row.cases.len(), 6);
        let names: Vec<String> = row.cases.iter().map(Case::name).collect();
        assert_eq!(names[0], "load-bound/tenth-of-a-millisecond-idle");
        assert_eq!(names[5], "load-bound/ten-milliseconds-saturated");
        assert!(BOUNDS.iter().all(|(_, limit)| row.ratio(0, *limit).is_some()));
        assert_eq!(row.alone.len(), 5);
        let (facts, summary, log) = report(&row, 1, true);
        assert!(facts.finish().contains("load-bound"));
        assert!(summary.contains("an indication and no measurement"), "{summary}");
        assert!(log.contains("load-bound/one-millisecond-saturated"), "{log}");
    }

    /// The moments during which every file thread computes, and the part of
    /// a wait that stands inside them.
    #[test]
    fn a_delay_behind_a_computation_is_the_part_of_a_wait_during_which_every_file_thread_computes() {
        let origin = Instant::now();
        let at = |micros: u64| origin + Duration::from_micros(micros);
        let ran = |thread: usize, from: u64, to: u64| Ran { work: Work::Verification, hash: 0, size: 1, estimate: Duration::ZERO, place: Place::FileThread(thread), started: at(from), ended: at(to) };
        let pool = Ran { place: Place::Action(0), ..ran(0, 0, 10_000) };
        // The first file thread computes from 100 to 500 and from 900 to
        // 1 000, the second from 300 to 950: both from 300 to 500 and from
        // 900 to 950. An action of the pool is no computation of the executor.
        let together = every_file_thread_computing(&[ran(0, 100, 500), ran(1, 300, 950), ran(0, 900, 1_000), pool]);
        assert_eq!(together, vec![(at(300), at(500)), (at(900), at(950))]);
        assert_eq!(inside((at(0), at(2_000)), &together), Duration::from_micros(250));
        assert_eq!(inside((at(400), at(920)), &together), Duration::from_micros(120));
        assert_eq!(inside((at(500), at(900)), &together), Duration::ZERO);
        assert!(every_file_thread_computing(&[ran(0, 100, 500)]).is_empty(), "one file thread that computes leaves the other to its reads");
        // The 99th percentile of a hundred durations is the 99th.
        let hundred = (1..=100_u64).map(Duration::from_micros);
        assert_eq!(percentile_99(hundred), Duration::from_micros(99));
        assert_eq!(percentile_99([7_u64].into_iter().map(Duration::from_micros)), Duration::from_micros(7));
        assert_eq!(percentile_99(std::iter::empty()), Duration::ZERO);
    }
}
