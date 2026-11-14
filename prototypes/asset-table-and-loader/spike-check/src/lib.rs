//! The scenarios of the asset table spike, copied and ported: the first check
//! of the prototype of the asset table and of the loader. Throwaway, never
//! product code.
//!
//! These are the asset table spike's own scenarios, with their oracles, run
//! again on the prototype's table and world in place of the spike's. Each is
//! kept as the spike wrote it, changed only where the prototype's interface
//! differs: a want of the world's own entry names what made it, an instance
//! wants its asset under the world's own entry, the world hands its wants to
//! the availability source through an extraction at a tick's end and a query
//! at its creation, and the parts are read in the prototype's resident store.
//! The spike's own loader and catalog stand outside the domain as they did.
//! No check is changed to fit the prototype: where a rule of the prototype
//! differs from the spike's, the check that states the spike's rule fails,
//! and that failure is its result.
//!
//! The reference workload reads its repulsion parameters through asset
//! handles: every entity names an asset by a 128-bit identifier, and a
//! world's asset table says which assets are wanted, by whom, and at which
//! content hash each is available. The scenarios check, instead of assuming:
//!
//! - wants and their owners: an asset stays available while one owner is
//!   left and falls at the input fixed point of the tick after the last want
//!   ended, with nothing in the input record; a batch wants its dependencies;
//! - the gate during a run, evaluated on the pending input record;
//! - a revision applied as one batch, and two revisions merged before a drain;
//! - the table under the state hash;
//! - a save read by another process, as a new run and to continue its history;
//! - one resident copy of a canonical part for sixteen worlds of one process.
//!
//! The spike's measurement of what a handle costs on a hot pass is not run
//! again, and nothing that only it used is kept.
//!
//! The crate is a library: the binary that hosts it declares the counting
//! allocator as its global allocator, calls [`run`], and routes the argument
//! `--spike-reader` to [`child`].

mod loader;
mod oracle;

use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use asset_loader::ResidentStore;
use prototype_core::table::{
    Arrival, AssetId, Availability, Command, Event, Maker, Outcome, Owner, Policy, REASON_HASH_MISMATCH, Record, RepulsionTable, Resident,
    Residents, decode_log, encode_log, reason_text,
};
use prototype_core::world::{Extraction, Links, Mode, Refusal, Restore, StepResult, Wait, World, WorldConfig};
use reference_workload::{Json, Parameters, Size, first_divergent_tick, fnv1a, run_hash, run_sequential};

use loader::{Catalog, Delay, LEAVES, Loader, Request, Source};
use oracle::{Switch, first_kill_ticks, oracle, switch};

// ---------------------------------------------------------------------------
// The allocation counters
// ---------------------------------------------------------------------------

/// The system allocator with two counters: every allocation, and the live
/// blocks of one tracked size (the large asset's canonical part), with their peak.
///
/// A library cannot declare a global allocator for the binary it is linked
/// into: the binary that hosts these scenarios declares it,
/// `#[global_allocator] static ALLOCATOR: spike_check::Counting = spike_check::Counting;`.
/// Without it the counters stay at zero and the scenario of the one resident
/// copy does not hold.
pub struct Counting;

/// Every allocation of the process, counted.
pub static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
/// The size whose live blocks are counted; no size while it is the maximum.
pub static TRACKED_SIZE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The live blocks of the tracked size.
pub static TRACKED_LIVE: AtomicIsize = AtomicIsize::new(0);
/// The most blocks of the tracked size that were live at once.
pub static TRACKED_PEAK: AtomicIsize = AtomicIsize::new(0);

impl Counting {
    fn entered(size: usize) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        if size == TRACKED_SIZE.load(Ordering::Relaxed) {
            let live = TRACKED_LIVE.fetch_add(1, Ordering::Relaxed) + 1;
            TRACKED_PEAK.fetch_max(live, Ordering::Relaxed);
        }
    }

    fn left(size: usize) {
        if size == TRACKED_SIZE.load(Ordering::Relaxed) {
            TRACKED_LIVE.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

// SAFETY: every call is forwarded to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Counting::entered(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Counting::entered(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Counting::left(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Counting::left(layout.size());
        Counting::entered(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// The library's own tests run under the counting allocator.
#[cfg(test)]
#[global_allocator]
static GLOBAL: Counting = Counting;

// ---------------------------------------------------------------------------
// Settings and small helpers
// ---------------------------------------------------------------------------

/// The name of the scratch directory of the save scenario.
const SCRATCH: &str = "asset-table-spike-check";
/// The first argument of a reader of a save that runs as a process of its
/// own: the hosting binary routes what follows it to [`child`].
const READER_ARGUMENT: &str = "--spike-reader";
/// A wait no scenario is expected to exhaust.
const LONG: Wait = Wait::Until(Duration::from_secs(120));
const SPECIFICATION: Option<RepulsionTable> = Some(RepulsionTable::SPECIFICATION);
/// The one contributor of the composition: it makes every want of a
/// configuration.
const CONTRIBUTOR: u64 = 1;
/// The one source of commands: it makes every want a command makes.
const SOURCE: u64 = 1;
/// A want of the world's own entry made by the configuration.
const COMPOSITION: Owner = Owner::World(Maker::Contributor(CONTRIBUTOR));
/// A want of the world's own entry made by a command.
const COMMANDED: Owner = Owner::World(Maker::Source(SOURCE));

/// The sizes, tick counts, worker counts and delays the scenarios run with.
#[derive(Clone)]
pub struct Settings {
    size: Size,
    ticks: u32,
    worker_counts: Vec<usize>,
    /// The loader's delays of the gate scenario, beside the clock's range.
    clock_delay_ms: (u64, u64),
    explicit_delays_ms: Vec<u64>,
    /// The deadline of the step that waits at a gate nothing answers.
    deadline: Duration,
    /// The payloads of a revision land over this long.
    revision_spread: Duration,
    /// The host's pause between two batches of ten ticks while a revision lands.
    batch_pause: Duration,
    shared_worlds: usize,
    shared_ticks: u32,
    big_payload: usize,
    /// The self-checks read the save inside their own process.
    children_in_process: bool,
}

impl Settings {
    /// The measured settings: the default size over the specification's tick count.
    pub fn measured() -> Settings {
        Settings {
            size: Size::Default,
            ticks: reference_workload::DEFAULT_TICK_COUNT,
            worker_counts: vec![1, 4, 8],
            clock_delay_ms: (20, 150),
            explicit_delays_ms: vec![0, 100],
            deadline: Duration::from_millis(30),
            revision_spread: Duration::from_millis(200),
            batch_pause: Duration::from_millis(3),
            shared_worlds: 16,
            shared_ticks: 32,
            big_payload: 1 << 20,
            children_in_process: false,
        }
    }

    /// An unoptimized build measures the small size over fewer ticks, as the
    /// spikes before this one do; the report says which settings ran.
    pub fn unoptimized() -> Settings {
        Settings { size: Size::Small, ticks: 200, revision_spread: Duration::from_millis(60), ..Settings::measured() }
    }

    /// The tick at a share of the run, in thousandths.
    fn at(&self, per_mille: u32) -> u32 {
        (self.ticks * per_mille / 1_000).max(1)
    }

    fn parameters(&self) -> Parameters {
        Parameters::new(self.size, self.ticks, false)
    }

    fn world(&self, workers: usize, catalog: &Catalog, wants: &[AssetId]) -> WorldConfig {
        let wants: Vec<(AssetId, Policy)> = wants.iter().map(|id| (*id, Policy::Required)).collect();
        WorldConfig::new(self.size, workers, Mode::Identifiers, catalog.leaf.to_vec()).wanting(CONTRIBUTOR, &wants)
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn divergence(ours: &[u64], theirs: &[u64]) -> i64 {
    first_divergent_tick(ours, theirs).map_or(-1, |t| t as i64)
}

/// Where a world's wants leave the domain for the spike's loader: the wanted
/// assets that became unanswered at a tick are what the loader receives, as
/// the spike's world sent them.
fn extraction_to(requests: &Sender<Request>) -> Arc<dyn Fn(Extraction) + Send + Sync> {
    let requests = requests.clone();
    Arc::new(move |extraction: Extraction| {
        if !extraction.unanswered.is_empty() {
            let _ = requests.send(Request::Wanted(extraction.unanswered));
        }
    })
}

/// What the spike's world sent its loader at its creation, where no tick has
/// ended: the wanted assets its table holds as unanswered, read by a query.
fn hand_over_the_wants_at_creation(world: &World, requests: &Sender<Request>) {
    let unanswered = world.unanswered();
    if !unanswered.is_empty() {
        let _ = requests.send(Request::Wanted(unanswered));
    }
}

/// Steps a world up to a tick; false where it stopped before, at a gate or on a failure.
fn step_to(world: &mut World, tick: u32) -> bool {
    if tick <= world.tick() {
        return tick == world.tick();
    }
    world.step(tick - world.tick(), LONG).is_ok_and(|result| !result.gated) && world.tick() == tick
}

fn names(catalog: &Catalog, assets: &[AssetId]) -> String {
    assets.iter().map(|id| catalog.name(*id)).collect::<Vec<_>>().join(", ")
}

fn ticks_text(ticks: &[u32]) -> String {
    ticks.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

fn record_ticks(log: &[Record]) -> Vec<u32> {
    log.iter().map(|record| record.tick).collect()
}

fn record_at(log: &[Record], tick: u32) -> Option<&Record> {
    log.iter().find(|record| record.tick == tick)
}

/// Every part a scripted scenario names, made resident: a script stands for
/// a loader that has already done its work.
fn preload(store: &ResidentStore, catalog: &Catalog) {
    let mut parts = vec![
        catalog.group_a_part(),
        catalog.group_b_part(),
        catalog.root_part(catalog.group_a),
        catalog.root_part(catalog.group_b),
        Catalog::extra_part(1),
        Catalog::extra_part(2),
    ];
    for version in 1..=4 {
        parts.extend((0..LEAVES).map(|leaf| Catalog::leaf_part(version, leaf)));
    }
    for bytes in parts {
        store.insert(fnv1a(&bytes), bytes).expect("a valid part");
    }
}

/// The arrivals that make every leaf available at version 1 through the two
/// roots and leaf 4 on its own: twelve assets.
fn everything(catalog: &Catalog) -> Vec<Arrival> {
    let mut arrivals = catalog.closure_a();
    arrivals.push(Arrival::at(catalog.leaf[4], Catalog::leaf_hash(1, 4)));
    arrivals.extend(catalog.closure_b());
    arrivals
}

fn all_wants(catalog: &Catalog) -> [AssetId; 3] {
    [catalog.root_a, catalog.leaf[4], catalog.root_b]
}

// ---------------------------------------------------------------------------
// Wants
// ---------------------------------------------------------------------------

/// Wants and their owners, scripted: an asset two entities want, which the
/// workload's own structural changes then despawn one after the other; a root
/// wanted by the world, answered with its closure by one batch, then
/// released; an arrival for an asset no want names; an asset wanted again
/// after it fell. The oracle is given the ticks the rules say.
fn wants(out: &mut Json, settings: &Settings, catalog: &Catalog) -> bool {
    let parameters = settings.parameters();
    let store = Arc::new(ResidentStore::default());
    preload(&store, catalog);
    let t = |per_mille: u32| settings.at(per_mille);
    let first = first_kill_ticks(&parameters);
    let pick = |low: u32, high: u32| first.iter().position(|tick| tick.is_some_and(|tick| tick >= low && tick < high));
    let (Some(first_slot), Some(second_slot)) = (pick(t(300), t(500)), pick(t(600), t(800))) else {
        panic!("the workload despawns an entity in each of the two windows");
    };
    let (first_kill, second_kill) = (first[first_slot].expect("picked"), first[second_slot].expect("picked"));
    // The two owners in the order the table holds the wants of two entities in: by slot.
    let mut owner_slots = [first_slot as u32, second_slot as u32];
    owner_slots.sort_unstable();
    let owners = owner_slots.map(|slot| Owner::Entity { slot, generation: 0 });
    let leaf4 = catalog.leaf[4];
    let want = |owner: Owner| Command::Want { asset: leaf4, owner, policy: Policy::Optional };
    let script = vec![
        Record::arrivals(1, catalog.closure_a()),
        Record::arrivals(t(50), vec![Arrival::at(catalog.extra, fnv1a(&Catalog::extra_part(1)))]),
        Record::commands(t(100), vec![want(owners[0]), want(owners[1])]),
        Record::arrivals(t(110), vec![Arrival::at(leaf4, Catalog::leaf_hash(1, 4))]),
        Record::commands(t(150), vec![Command::Want { asset: catalog.root_b, owner: COMMANDED, policy: Policy::Optional }]),
        Record::arrivals(t(160), catalog.closure_b()),
        Record::commands(t(250), vec![Command::Release { asset: catalog.root_b, owner: COMMANDED }]),
        Record::commands(t(900), vec![want(COMMANDED)]),
        Record::arrivals(t(905), vec![Arrival::at(leaf4, Catalog::leaf_hash(2, 4))]),
    ];
    let mut expected_ticks = record_ticks(&script);
    expected_ticks.dedup();
    // What the rules say each leaf reads, and from which tick.
    let mut schedule: Vec<Switch> = (0..4).map(|leaf| switch(1, leaf, SPECIFICATION)).collect();
    schedule.push(switch(t(110), 4, SPECIFICATION));
    schedule.push(switch(second_kill + 1, 4, None));
    schedule.push(switch(t(905), 4, Some(Catalog::leaf_table(2, 4))));
    for leaf in 5..8 {
        schedule.push(switch(t(160), leaf, SPECIFICATION));
        schedule.push(switch(t(250) + 1, leaf, None));
    }
    let oracle_hashes = oracle(&parameters, LEAVES, &schedule);
    let available = |version: u32, leaf: usize| Availability::Available(Catalog::leaf_hash(version, leaf));

    let mut holds = true;
    let mut first_state_hashes: Option<Vec<u64>> = None;
    let mut first_log: Option<Vec<u8>> = None;
    let mut table_at_a_fifth = (0_u64, 0_usize);
    out.key("wants");
    out.object_start();
    out.u64("first_owner_slot", first_slot as u64);
    out.u64("first_owner_despawned_at", u64::from(first_kill));
    out.u64("second_owner_slot", second_slot as u64);
    out.u64("second_owner_despawned_at", u64::from(second_kill));
    out.key("runs");
    out.array_start();
    for (index, &workers) in settings.worker_counts.iter().enumerate() {
        let config = settings.world(workers, catalog, &[catalog.root_a]);
        let mut world = World::new(config.clone(), script.clone(), Links::new(store.clone()));
        let mut stepped = step_to(&mut world, t(50));
        let unnamed_arrival_changes_nothing =
            world.entry(catalog.extra).is_none() && world.table_hash(t(50)) == world.table_hash(t(50) - 1) && world.unnamed_arrivals() == 1;
        stepped &= step_to(&mut world, t(110));
        let wanted_by_two_owners = world.entry(leaf4) == Some((available(1, 4), owners.iter().map(|owner| (*owner, Policy::Optional)).collect()));
        stepped &= step_to(&mut world, t(160));
        let optional_from = |parent: AssetId| vec![(Owner::Asset(parent), Policy::Optional)];
        let dependencies_wanted_and_answered_by_the_same_batch = world.entry(catalog.group_b).is_some_and(|(_, wants)| wants == optional_from(catalog.root_b))
            && (5..8).all(|leaf| world.entry(catalog.leaf[leaf]) == Some((available(1, leaf), optional_from(catalog.group_b))))
            && record_at(world.log(), t(160)).is_some_and(|record| record.arrivals.len() == 5 && record.commands.is_empty());
        stepped &= step_to(&mut world, t(200));
        if index == 0 {
            table_at_a_fifth = (world.table_hash(t(200)), world.table_bytes().len());
        }
        stepped &= step_to(&mut world, t(250));
        let available_during_the_tick_of_the_release = world.entry(catalog.root_b).is_some_and(|(availability, wants)| wants.is_empty() && availability != Availability::Unavailable);
        stepped &= step_to(&mut world, t(250) + 1);
        let released_closure_fell_together =
            [catalog.root_b, catalog.group_b, catalog.leaf[5], catalog.leaf[6], catalog.leaf[7]].iter().all(|id| world.entry(*id).is_none());
        stepped &= step_to(&mut world, first_kill);
        let stays_available_with_one_owner_left = world.entry(leaf4).is_some_and(|(availability, wants)| availability == available(1, 4) && wants.len() == 1);
        stepped &= step_to(&mut world, second_kill);
        let still_available_at_the_tick_its_last_want_ended = world.entry(leaf4) == Some((available(1, 4), Vec::new()));
        stepped &= step_to(&mut world, second_kill + 1);
        let fell_at_the_fixed_point_of_the_tick_after = world.entry(leaf4).is_none();
        stepped &= step_to(&mut world, settings.ticks);
        let wanted_again_and_answered_by_a_transition = world.entry(leaf4) == Some((available(2, 4), vec![(COMMANDED, Policy::Optional)]));
        let ticks = record_ticks(world.log());
        let falls_absent_from_the_input_record = ticks == expected_ticks && !ticks.contains(&(second_kill + 1)) && !ticks.contains(&(t(250) + 1));

        let components = world.component_hashes();
        let states = world.state_hashes();
        let log_bytes = encode_log(world.log());
        // The recorded log, read back from its bytes, replayed at another worker count.
        let replay_workers = settings.worker_counts[(index + 1) % settings.worker_counts.len()];
        let mut replay = World::new(WorldConfig { workers: replay_workers, ..config }, decode_log(&log_bytes).expect("the log decodes"), Links::new(store.clone()));
        let replay_matches = step_to(&mut replay, settings.ticks) && replay.state_hashes() == states;
        let same_at_every_worker_count = first_state_hashes.get_or_insert_with(|| states.clone()) == &states && first_log.get_or_insert_with(|| log_bytes.clone()) == &log_bytes;
        let matches_oracle = components == oracle_hashes;
        let checks = [
            ("unnamed_arrival_changes_nothing", unnamed_arrival_changes_nothing),
            ("wanted_by_two_owners", wanted_by_two_owners),
            ("dependencies_wanted_and_answered_by_the_same_batch", dependencies_wanted_and_answered_by_the_same_batch),
            ("available_during_the_tick_of_the_release", available_during_the_tick_of_the_release),
            ("released_closure_fell_together", released_closure_fell_together),
            ("stays_available_with_one_owner_left", stays_available_with_one_owner_left),
            ("still_available_at_the_tick_its_last_want_ended", still_available_at_the_tick_its_last_want_ended),
            ("fell_at_the_fixed_point_of_the_tick_after", fell_at_the_fixed_point_of_the_tick_after),
            ("wanted_again_and_answered_by_a_transition", wanted_again_and_answered_by_a_transition),
            ("falls_absent_from_the_input_record", falls_absent_from_the_input_record),
            ("matches_oracle", matches_oracle),
            ("replay_matches", replay_matches),
            ("same_at_every_worker_count", same_at_every_worker_count),
            ("every_step_ran", stepped),
        ];
        out.object_start();
        out.u64("workers", workers as u64);
        out.hex("state_run_hash", run_hash(&states));
        out.hex("component_run_hash", run_hash(&components));
        out.i64("first_divergent_tick_from_oracle", divergence(&components, &oracle_hashes));
        out.u64("replay_workers", replay_workers as u64);
        out.string("record_ticks", &ticks_text(&ticks));
        out.u64("log_bytes", log_bytes.len() as u64);
        out.u64("refused_commands", u64::from(world.refused_commands()));
        for (name, value) in checks {
            out.bool(name, value);
            holds &= value;
        }
        out.object_end();
    }
    out.array_end();
    out.hex("table_hash_after_a_fifth_of_the_run", table_at_a_fifth.0);
    out.u64("table_bytes_after_a_fifth_of_the_run", table_at_a_fifth.1 as u64);
    out.bool("holds", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// The gate during a run
// ---------------------------------------------------------------------------

fn write_step(out: &mut Json, key: &str, catalog: &Catalog, result: &StepResult) {
    out.key(key);
    out.object_start();
    out.u64("ticks_ran", u64::from(result.ticks_ran));
    out.bool("gated", result.gated);
    out.string("unanswered", &names(catalog, &result.unanswered));
    let failed: Vec<String> = result.failed_required.iter().map(|(id, code)| format!("{}: {}", catalog.name(*id), reason_text(*code))).collect();
    out.string("failed_required", &failed.join("; "));
    out.object_end();
}

/// The gate during a run, with a live loader and a real delay: a world whose
/// composition requires root A and leaf 4; an instance made from root B,
/// required, at a fifth of the run; an instance made from the corrupt root at
/// two fifths; a required want nothing answers at three fifths, which the
/// host then releases, with another command, while the world stands gated.
fn gate_run(out: &mut Json, settings: &Settings, catalog: &Catalog, oracle_hashes: &[u64], workers: usize, delay: Delay) -> (u64, bool) {
    let t = |per_mille: u32| settings.at(per_mille);
    let store = Arc::new(ResidentStore::default());
    let mut links = Links::new(store.clone());
    let loader = Loader::spawn(catalog.manifest([Some(1); LEAVES]), store.clone(), links.queue.clone(), delay);
    links.extract = Some(extraction_to(&loader.requests));
    let instantiate = |asset: AssetId| Command::Instantiate { asset, policy: Policy::Required, reload: false };
    let script = vec![
        Record::commands(t(200), vec![instantiate(catalog.root_b)]),
        Record::commands(t(400), vec![instantiate(catalog.root_c)]),
        Record::commands(t(600), vec![Command::Want { asset: catalog.root_d, owner: COMMANDED, policy: Policy::Required }]),
    ];
    let mut world = World::new(settings.world(workers, catalog, &[catalog.root_a, catalog.leaf[4]]), script, links);
    hand_over_the_wants_at_creation(&world, &loader.requests);
    let failed = StepResult { gated: true, ..StepResult::default() };
    let first = world.step(1, Wait::No).unwrap_or(failed.clone());
    let step_while_loading = match (first.gated, first.ticks_ran) {
        (true, 0) => "refused",
        (false, 1) => "gate already open",
        _ => "not refused",
    };
    let through_two_gates = world.step(t(600) - world.tick(), LONG).unwrap_or(failed.clone());
    let without_waiting = world.step(1, Wait::No).unwrap_or(failed.clone());
    let started = Instant::now();
    let with_a_deadline = world.step(1, Wait::Until(settings.deadline)).unwrap_or(failed.clone());
    let deadline_waited = started.elapsed();
    // A release names what made the want it ends: the configuration wanted leaf 4, a command the unanswered root.
    world.post(Command::Release { asset: catalog.leaf[4], owner: COMPOSITION });
    world.post(Command::Release { asset: catalog.root_d, owner: COMMANDED });
    let rest = world.step(settings.ticks - world.tick(), LONG).unwrap_or(failed);
    let report = loader.finish();

    let log = world.log();
    let gate_before = |tick: u32| world.gates().iter().find(|gate| gate.before_tick == tick);
    // With no delay the answer may stand in the pending record before the
    // boundary is reached: the gate then never closes, and nothing is asked of it.
    let instant = matches!(delay, Delay::Fixed(duration) if duration.is_zero());
    let closed_for = |tick: u32, asset: AssetId| match gate_before(tick) {
        Some(gate) => gate.unanswered == [asset] && gate.waited.is_some(),
        None => instant,
    };
    let a_closure_of_three_levels_answered_by_one_batch = closed_for(t(200) + 1, catalog.root_b)
        && record_at(log, t(200) + 1).is_some_and(|record| record.arrivals.len() == 5 && record.commands.is_empty())
        && world.instances().first().is_some_and(|instance| instance.instantiations == 1);
    let failure = Event::Failed { asset: catalog.root_c, code: REASON_HASH_MISMATCH };
    let a_failure_opens_the_gate_and_is_named = closed_for(t(400) + 1, catalog.root_c)
        && through_two_gates.failed_required == [(catalog.root_c, REASON_HASH_MISMATCH)]
        && record_at(log, t(400) + 1).is_some_and(|record| record.arrivals.len() == 1 && record.commands.is_empty())
        && world.events().contains(&(t(400) + 1, failure))
        && world.entry(catalog.root_c).is_some_and(|(availability, _)| availability == Availability::Failed(REASON_HASH_MISMATCH))
        && world.instances().get(1).is_some_and(|instance| instance.failed)
        && !through_two_gates.gated;
    let stands_at = |result: &StepResult| result.gated && result.ticks_ran == 0 && result.unanswered == [catalog.root_d];
    let a_gated_step_answers_with_no_tick = stands_at(&without_waiting) && stands_at(&with_a_deadline) && deadline_waited >= settings.deadline;
    let a_release_opens_the_gate_and_the_record_is_applied_once = !rest.gated
        && world.tick() == settings.ticks
        && record_at(log, t(600) + 1).is_some_and(|record| record.commands.len() == 2 && record.arrivals.is_empty())
        && world.entry(catalog.root_d).is_none();
    let ticks = record_ticks(log);
    let one_record_per_tick = ticks == [1, t(200), t(200) + 1, t(400), t(400) + 1, t(600), t(600) + 1]
        && record_at(log, 1).is_some_and(|record| record.arrivals.len() == 7);
    let components = world.component_hashes();
    let states = world.state_hashes();
    let matches_oracle = components == oracle_hashes;
    let checks = [
        ("a_closure_of_three_levels_answered_by_one_batch", a_closure_of_three_levels_answered_by_one_batch),
        ("a_failure_opens_the_gate_and_is_named", a_failure_opens_the_gate_and_is_named),
        ("a_gated_step_answers_with_no_tick", a_gated_step_answers_with_no_tick),
        ("a_release_opens_the_gate_and_the_record_is_applied_once", a_release_opens_the_gate_and_the_record_is_applied_once),
        ("one_record_per_tick", one_record_per_tick),
        ("matches_oracle", matches_oracle),
    ];
    let mut holds = step_while_loading != "not refused";
    out.object_start();
    out.u64("workers", workers as u64);
    out.string("delay", &delay.name());
    out.string("loader_delays_ms", &report.delays_ms.iter().map(|delay| format!("{delay:.1}")).collect::<Vec<_>>().join(", "));
    out.u64("loader_batches", u64::from(report.batches));
    out.u64("artifacts_read", report.reads as u64);
    out.string("step_while_loading", step_while_loading);
    out.key("gates");
    out.array_start();
    for gate in world.gates() {
        out.object_start();
        out.u64("before_tick", u64::from(gate.before_tick));
        out.string("unanswered", &names(catalog, &gate.unanswered));
        out.decimal("waited_ms", gate.waited.map_or(-1.0, ms));
        out.object_end();
    }
    out.array_end();
    write_step(out, "step_through_two_gates", catalog, &through_two_gates);
    write_step(out, "step_without_waiting", catalog, &without_waiting);
    write_step(out, "step_with_a_deadline", catalog, &with_a_deadline);
    out.decimal("deadline_waited_ms", ms(deadline_waited));
    out.string("record_ticks", &ticks_text(&ticks));
    out.hex("state_run_hash", run_hash(&states));
    out.hex("component_run_hash", run_hash(&components));
    out.i64("first_divergent_tick_from_oracle", divergence(&components, oracle_hashes));
    for (name, value) in checks {
        out.bool(name, value);
        holds &= value;
    }
    out.object_end();
    (run_hash(&states), holds)
}

fn gate(out: &mut Json, settings: &Settings, catalog: &Catalog) -> bool {
    let t = |per_mille: u32| settings.at(per_mille);
    let mut schedule: Vec<Switch> = (0..5).map(|leaf| switch(1, leaf, SPECIFICATION)).collect();
    schedule.extend((5..8).map(|leaf| switch(t(200) + 1, leaf, SPECIFICATION)));
    schedule.push(switch(t(600) + 2, 4, None));
    let oracle_hashes = oracle(&settings.parameters(), LEAVES, &schedule);
    let mut delays = vec![Delay::Clock(settings.clock_delay_ms.0, settings.clock_delay_ms.1)];
    delays.extend(settings.explicit_delays_ms.iter().map(|&delay| Delay::Fixed(Duration::from_millis(delay))));
    let mut holds = true;
    let mut run_hashes = Vec::new();
    out.key("gate");
    out.object_start();
    out.key("runs");
    out.array_start();
    for &workers in &settings.worker_counts {
        for &delay in &delays {
            let (state_run_hash, run_holds) = gate_run(out, settings, catalog, &oracle_hashes, workers, delay);
            run_hashes.push(state_run_hash);
            holds &= run_holds;
        }
    }
    out.array_end();
    run_hashes.sort_unstable();
    run_hashes.dedup();
    holds &= run_hashes.len() == 1;
    out.u64("distinct_state_run_hashes", run_hashes.len() as u64);
    out.bool("holds", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// A revision as one batch
// ---------------------------------------------------------------------------

/// A revision of eight leaves whose payloads land one by one in a random
/// order while the world steps; then two revisions posted before one drain.
fn revision(out: &mut Json, settings: &Settings, catalog: &Catalog) -> bool {
    let parameters = settings.parameters();
    let t = |per_mille: u32| settings.at(per_mille);
    let workers = if settings.worker_counts.contains(&4) { 4 } else { *settings.worker_counts.last().expect("a worker count") };
    let store = Arc::new(ResidentStore::default());
    let mut links = Links::new(store.clone());
    let queue = links.queue.clone();
    let loader = Loader::spawn(catalog.manifest([Some(1); LEAVES]), store.clone(), queue.clone(), Delay::Fixed(Duration::ZERO));
    links.extract = Some(extraction_to(&loader.requests));
    let config = settings.world(workers, catalog, &all_wants(catalog));
    let mut world = World::new(config.clone(), Vec::new(), links);
    hand_over_the_wants_at_creation(&world, &loader.requests);
    let mut stepped = step_to(&mut world, t(100));
    let parts = (0..LEAVES).map(|leaf| (catalog.leaf[leaf], Catalog::leaf_part(2, leaf))).collect();
    let _ = loader.requests.send(Request::Revision { parts, spread: settings.revision_spread });
    // The host steps on while the payloads land, and notes where the world stood.
    let mut tick_at_the_first_payload = None;
    while world.tick() < t(790) && !loader.posted.load(Ordering::SeqCst) {
        let batch_end = (world.tick() + 10).min(t(790));
        stepped &= step_to(&mut world, batch_end);
        if tick_at_the_first_payload.is_none() && loader.landed.load(Ordering::SeqCst) > 0 {
            tick_at_the_first_payload = Some(world.tick());
        }
        thread::sleep(settings.batch_pause);
    }
    while !loader.posted.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(1));
    }
    stepped &= step_to(&mut world, t(800));
    // Two revisions posted before one drain: every leaf at version 3, then the leaves 0 to 3 at version 4.
    let leaves_at = |version: u32, leaves: std::ops::Range<usize>| -> Vec<Arrival> { leaves.map(|leaf| Arrival::at(catalog.leaf[leaf], Catalog::leaf_hash(version, leaf))).collect() };
    preload(&store, catalog);
    queue.post(&leaves_at(3, 0..LEAVES));
    queue.post(&leaves_at(4, 0..4));
    stepped &= step_to(&mut world, settings.ticks);
    let report = loader.finish();

    let log = world.log();
    let second_version = |record: &&Record| record.arrivals.iter().any(|arrival| (0..LEAVES).any(|leaf| arrival.outcome == Outcome::Hash(Catalog::leaf_hash(2, leaf))));
    let revision_records: Vec<&Record> = log.iter().filter(second_version).collect();
    let applied_at = revision_records.first().map_or(0, |record| record.tick);
    let applied_at_one_fixed_point = revision_records.len() == 1 && revision_records[0].arrivals.len() == LEAVES;
    let in_identifier_order = revision_records.first().is_some_and(|record| record.arrivals.windows(2).all(|pair| pair[0].asset < pair[1].asset));
    let merged_at = t(800) + 1;
    let expected_merge: Vec<Arrival> = {
        let mut arrivals = leaves_at(4, 0..4);
        arrivals.extend(leaves_at(3, 4..LEAVES));
        arrivals.sort_by_key(|arrival| arrival.asset);
        arrivals
    };
    let two_revisions_merged_each_asset_at_its_later_hash = record_at(log, merged_at).is_some_and(|record| record.arrivals == expected_merge);

    let mut schedule: Vec<Switch> = (0..LEAVES).map(|leaf| switch(1, leaf, SPECIFICATION)).collect();
    schedule.extend((0..LEAVES).map(|leaf| switch(applied_at, leaf, Some(Catalog::leaf_table(2, leaf)))));
    schedule.extend((0..LEAVES).map(|leaf| switch(merged_at, leaf, Some(Catalog::leaf_table(if leaf < 4 { 4 } else { 3 }, leaf)))));
    let oracle_hashes = oracle(&parameters, LEAVES, &schedule);
    let components = world.component_hashes();
    let states = world.state_hashes();
    let matches_oracle = components == oracle_hashes;
    // A world that applied a part of the batch: the first payloads to land, and no other.
    let landing: Vec<usize> = report.landing.iter().filter_map(|(id, _)| catalog.leaf.iter().position(|leaf| leaf == id)).collect();
    let until = Parameters::new(settings.size, applied_at, false);
    let live_at_the_batch = components.get(applied_at as usize).copied().unwrap_or(0);
    let mut partial_hashes = Vec::new();
    for count in 1..LEAVES {
        let mut partial: Vec<Switch> = (0..LEAVES).map(|leaf| switch(1, leaf, SPECIFICATION)).collect();
        partial.extend(landing.iter().take(count).map(|&leaf| switch(applied_at, leaf, Some(Catalog::leaf_table(2, leaf)))));
        partial_hashes.push(oracle(&until, LEAVES, &partial).last().copied().unwrap_or(0));
    }
    let no_tick_equals_a_partial_application = applied_at > 0 && landing.len() == LEAVES && partial_hashes.iter().all(|hash| *hash != live_at_the_batch);

    let log_bytes = encode_log(log);
    let mut replays_match = true;
    for &replay_workers in &settings.worker_counts {
        let mut replay = World::new(WorldConfig { workers: replay_workers, ..config.clone() }, decode_log(&log_bytes).expect("the log decodes"), Links::new(store.clone()));
        replays_match &= step_to(&mut replay, settings.ticks) && replay.state_hashes() == states;
    }
    let checks = [
        ("applied_at_one_fixed_point", applied_at_one_fixed_point),
        ("in_identifier_order", in_identifier_order),
        ("no_tick_equals_a_partial_application", no_tick_equals_a_partial_application),
        ("two_revisions_merged_each_asset_at_its_later_hash", two_revisions_merged_each_asset_at_its_later_hash),
        ("matches_oracle", matches_oracle),
        ("replays_match_at_every_worker_count", replays_match),
        ("every_step_ran", stepped),
    ];
    let mut holds = true;
    out.key("revision");
    out.object_start();
    out.u64("workers", workers as u64);
    out.decimal("spread_ms", ms(settings.revision_spread));
    out.string("payloads_landed_at_ms", &report.landing.iter().map(|(_, at)| format!("{at:.1}")).collect::<Vec<_>>().join(", "));
    out.string("landing_order", &names(catalog, &report.landing.iter().map(|(id, _)| *id).collect::<Vec<_>>()));
    out.string("identifier_order", &names(catalog, &revision_records.first().map_or_else(Vec::new, |record| record.arrivals.iter().map(|arrival| arrival.asset).collect())));
    out.i64("tick_at_the_first_payload", tick_at_the_first_payload.map_or(-1, i64::from));
    out.u64("applied_at_tick", u64::from(applied_at));
    out.u64("merged_at_tick", u64::from(merged_at));
    out.u64("partial_applications_compared", partial_hashes.len() as u64);
    out.string("record_ticks", &ticks_text(&record_ticks(log)));
    out.u64("log_bytes", log_bytes.len() as u64);
    out.hex("state_run_hash", run_hash(&states));
    out.i64("first_divergent_tick_from_oracle", divergence(&components, &oracle_hashes));
    for (name, value) in checks {
        out.bool(name, value);
        holds &= value;
    }
    out.bool("holds", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// The table under the hash
// ---------------------------------------------------------------------------

/// Two worlds equal in every component and different in the hash of one
/// asset no entity names.
fn table_under_the_hash(out: &mut Json, settings: &Settings, catalog: &Catalog) -> bool {
    let store = Arc::new(ResidentStore::default());
    preload(&store, catalog);
    let ticks = 10.min(settings.ticks);
    let run = |version: u32| {
        let config = WorldConfig::new(Size::Small, 1, Mode::Identifiers, catalog.leaf.to_vec()).wanting(CONTRIBUTOR, &[(catalog.extra, Policy::Optional)]);
        let script = vec![Record::arrivals(1, vec![Arrival::at(catalog.extra, fnv1a(&Catalog::extra_part(version)))])];
        let mut world = World::new(config, script, Links::new(store.clone()));
        let stepped = step_to(&mut world, ticks);
        (world.component_hashes(), world.state_hashes(), world.table_hash(ticks), world.table_bytes().len(), stepped)
    };
    let (one, other) = (run(1), run(2));
    let components_equal = one.0 == other.0 && one.4 && other.4;
    let state_hashes_differ_from_the_arrival = one.1[0] == other.1[0] && (1..=ticks as usize).all(|tick| one.1[tick] != other.1[tick]);
    let holds = components_equal && state_hashes_differ_from_the_arrival;
    out.key("table_under_the_hash");
    out.object_start();
    out.u64("ticks", u64::from(ticks));
    out.bool("components_equal_at_every_tick", components_equal);
    out.bool("state_hashes_differ_from_the_arrival", state_hashes_differ_from_the_arrival);
    out.hex("table_hash_of_the_one", one.2);
    out.hex("table_hash_of_the_other", other.2);
    out.u64("table_bytes", one.3 as u64);
    out.bool("holds", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// A save read by another process
// ---------------------------------------------------------------------------

const CHILD_MODES: [&str; 6] = ["new run, same hashes", "new run, one changed hash", "new run, one failure", "continued, confirmed", "continued, one changed hash", "continued, one part missing"];

fn write_hashes(path: &Path, hashes: &[u64]) {
    let bytes: Vec<u8> = hashes.iter().flat_map(|hash| hash.to_le_bytes()).collect();
    fs::write(path, bytes).expect("the scratch directory is writable");
}

fn read_hashes(path: &Path) -> Vec<u64> {
    let bytes = fs::read(path).expect("the parent wrote the expected hashes");
    bytes.chunks_exact(8).map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight bytes"))).collect()
}

fn part_path(directory: &Path, hash: u64) -> PathBuf {
    directory.join(format!("{hash:016x}.part"))
}

/// The world a save is taken from, and what another process is handed: all
/// leaves required through the two roots and leaf 4, an instance made from
/// leaf 4 with its reload setting on, leaf 4 reloaded at three tenths of the
/// run (the instance is instantiated again), the save at half, leaf 6
/// reloaded at seven tenths.
fn save(out: &mut Json, settings: &Settings, catalog: &Catalog) -> bool {
    let parameters = settings.parameters();
    let t = |per_mille: u32| settings.at(per_mille);
    let workers = if settings.worker_counts.contains(&4) { 4 } else { *settings.worker_counts.last().expect("a worker count") };
    let store = Arc::new(ResidentStore::default());
    preload(&store, catalog);
    let instantiate = Command::Instantiate { asset: catalog.leaf[4], policy: Policy::Required, reload: true };
    let mut first = Record::arrivals(1, everything(catalog));
    first.commands.push(instantiate);
    let script = vec![
        first,
        Record::arrivals(t(300), vec![Arrival::at(catalog.leaf[4], Catalog::leaf_hash(2, 4))]),
        Record::arrivals(t(700), vec![Arrival::at(catalog.leaf[6], Catalog::leaf_hash(2, 6))]),
    ];
    let mut world = World::new(settings.world(workers, catalog, &all_wants(catalog)), script, Links::new(store.clone()));
    let saved_at = t(500);
    let mut stepped = step_to(&mut world, saved_at);
    let saved = world.save();
    let table_hash_at_the_save = world.table_hash(saved_at);
    stepped &= step_to(&mut world, settings.ticks);

    // The oracles: the run as it went, and the two runs a new process may make of the save.
    let base = |leaf4_after: Option<Option<RepulsionTable>>| {
        let mut schedule: Vec<Switch> = (0..LEAVES).map(|leaf| switch(1, leaf, SPECIFICATION)).collect();
        schedule.push(switch(t(300), 4, Some(Catalog::leaf_table(2, 4))));
        if let Some(table) = leaf4_after {
            schedule.push(switch(saved_at + 1, 4, table));
        }
        schedule.push(switch(t(700), 6, Some(Catalog::leaf_table(2, 6))));
        oracle(&parameters, LEAVES, &schedule)
    };
    let components = world.component_hashes();
    let states = world.state_hashes();
    let matches_oracle = components == base(None);
    let a_reload_instantiates_again = world.instances().first().is_some_and(|instance| instance.instantiations == 2 && !instance.failed)
        && world.events().iter().any(|(tick, event)| *tick == t(300) && matches!(event, Event::HashChanged { .. }));

    let directory = env::temp_dir().join(format!("{SCRATCH}-{}-{}", std::process::id(), loader::clock_nanos()));
    let (full, missing) = (directory.join("store"), directory.join("store-missing"));
    fs::create_dir_all(&full).expect("the scratch directory is writable");
    fs::create_dir_all(&missing).expect("the scratch directory is writable");
    for hash in store.hashes() {
        let resident = store.get(hash).expect("listed");
        fs::write(part_path(&full, hash), &resident.bytes).expect("the scratch directory is writable");
        if hash != Catalog::leaf_hash(2, 4) {
            fs::write(part_path(&missing, hash), &resident.bytes).expect("the scratch directory is writable");
        }
    }
    let later: Vec<Record> = world.log().iter().filter(|record| record.tick > saved_at).cloned().collect();
    fs::write(directory.join("save.bin"), &saved).expect("the scratch directory is writable");
    fs::write(directory.join("record.bin"), encode_log(&later)).expect("the scratch directory is writable");
    let from_the_save = saved_at as usize..;
    write_hashes(&directory.join("expected-state.bin"), &states[from_the_save.clone()]);
    write_hashes(&directory.join("expected-changed.bin"), &base(Some(Some(Catalog::leaf_table(3, 4))))[from_the_save.clone()]);
    write_hashes(&directory.join("expected-failed.bin"), &base(Some(None))[from_the_save]);

    let mut holds = matches_oracle && a_reload_instantiates_again && stepped;
    out.key("save");
    out.object_start();
    out.u64("workers", workers as u64);
    out.u64("saved_at_tick", u64::from(saved_at));
    out.u64("save_bytes", saved.len() as u64);
    out.hex("table_hash_at_the_save", table_hash_at_the_save);
    out.hex("state_run_hash", run_hash(&states));
    out.bool("matches_oracle", matches_oracle);
    out.bool("a_reload_instantiates_again", a_reload_instantiates_again);
    out.u64("process", u64::from(std::process::id()));
    out.key("readers");
    out.array_start();
    for (index, mode) in CHILD_MODES.iter().enumerate() {
        let arguments = [index.to_string(), directory.display().to_string(), settings.size.name().to_string(), settings.ticks.to_string(), workers.to_string(), std::process::id().to_string()];
        let report = if settings.children_in_process {
            child(&arguments)
        } else {
            let run = env::current_exe().and_then(|exe| Process::new(exe).arg(READER_ARGUMENT).args(&arguments).output());
            match run {
                Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).trim().to_string(),
                Ok(output) => child_error(mode, &String::from_utf8_lossy(&output.stderr)),
                Err(error) => child_error(mode, &error.to_string()),
            }
        };
        holds &= report.contains("\"holds\":true");
        out.array_value(&report);
    }
    out.array_end();
    out.bool("holds", holds);
    out.object_end();
    let _ = fs::remove_dir_all(&directory);
    holds
}

fn child_error(mode: &str, error: &str) -> String {
    let mut out = Json::new();
    out.object_start();
    out.string("mode", mode);
    out.string("error", error.trim());
    out.bool("holds", false);
    out.object_end();
    out.finish()
}

trait ArrayValue {
    fn array_value(&mut self, json: &str);
}

impl ArrayValue for Json {
    /// An element already encoded as JSON, inside an array.
    fn array_value(&mut self, json: &str) {
        self.object_start();
        self.raw("report", json);
        self.object_end();
    }
}

/// The reader of a save, in a process of its own: a new run on its answers,
/// or the continuation of the saved history.
///
/// The save scenario starts the hosting binary again with `--spike-reader`
/// as its first argument; the binary hands the arguments that follow it to
/// this function and prints what it returns, the reader's report, on its
/// standard output. The arguments are the reader's mode, the scratch
/// directory, the size, the tick count, the worker count and the parent's
/// process identifier.
pub fn child(arguments: &[String]) -> String {
    let mode: usize = arguments[0].parse().expect("a mode");
    let directory = PathBuf::from(&arguments[1]);
    let size = *Size::ALL.iter().find(|size| size.name() == arguments[2]).expect("a size");
    let ticks: u32 = arguments[3].parse().expect("a tick count");
    let workers: usize = arguments[4].parse().expect("a worker count");
    let parent: u32 = arguments[5].parse().expect("a process identifier");
    let catalog = Catalog::new();
    let saved = fs::read(directory.join("save.bin")).expect("the parent wrote the save");
    let recorded = fs::read(directory.join("record.bin")).expect("the parent wrote the record");
    let later = decode_log(&recorded).expect("the record decodes");
    let store = Arc::new(ResidentStore::default());
    let wants: Vec<(AssetId, Policy)> = all_wants(&catalog).iter().map(|id| (*id, Policy::Required)).collect();
    let mut config = WorldConfig::new(size, workers, Mode::Identifiers, catalog.leaf.to_vec()).wanting(CONTRIBUTOR, &wants);
    // The spike measured a failed answer to a restore as an event that fails
    // the instance made from the asset. The prototype reads the design text
    // as emitting no event on such an answer, and keeps what the spike
    // measured as a reading of its rules: the readers of this check run
    // under that reading, so that the spike's checks stand as it wrote them.
    config.rules.marked_failure_emits = true;
    let leaf4 = catalog.leaf[4];
    // What this process's content set says of leaf 4: the saved hash, another one, or a corrupt artifact.
    let mut versions = [Some(1); LEAVES];
    versions[4] = match mode {
        1 | 4 => Some(3),
        2 => None,
        _ => Some(2),
    };
    let manifest = catalog.manifest(versions);
    let parts = directory.join(if mode == 5 { "store-missing" } else { "store" });
    let read_part = |hash: u64| -> Result<Arc<Resident>, String> {
        if let Some(resident) = store.get(hash) {
            return Ok(resident);
        }
        let bytes = fs::read(part_path(&parts, hash)).map_err(|_| "the part cannot be had".to_string())?;
        store.insert(hash, bytes).map_err(|code| reason_text(code).to_string())
    };
    // The parts the later records name are had before any step, or the read is refused.
    let later_parts: Result<(), String> = later.iter().flat_map(|record| &record.arrivals).try_for_each(|arrival| match arrival.outcome {
        Outcome::Hash(hash) => read_part(hash).map(|_| ()),
        Outcome::Failure(_) => Ok(()),
    });

    let mut out = Json::new();
    out.object_start();
    out.string("mode", CHILD_MODES[mode]);
    out.u64("process", u64::from(std::process::id()));
    out.bool("in_another_process", std::process::id() != parent);
    let holds;
    if mode < 3 {
        // A save loaded as a new run: gated on its answers.
        let mut links = Links::new(store.clone());
        let loader = Loader::spawn(manifest, store.clone(), links.queue.clone(), Delay::Fixed(Duration::from_millis(20)));
        links.extract = Some(extraction_to(&loader.requests));
        let requests = loader.requests.clone();
        // The content at hand names no asset type, and the composition is the saved one: no entry is dropped.
        let content_at_hand = Restore::ContentAtHand { type_of: &|_| None, dropped: &[], drop_in_silence: false };
        let mut world = World::restore(config, &saved, later, links, content_at_hand).expect("a new run is never refused");
        let saved_at = world.tick();
        let query = world.restore_query();
        let awaited = query.len();
        let first = world.step(1, Wait::No).unwrap_or_default();
        let first_tick_waits_for_every_answer = first.gated && first.ticks_ran == 0 && first.unanswered.len() == awaited;
        let _ = requests.send(Request::Restore(query));
        let result = world.step(ticks - saved_at, LONG).unwrap_or_default();
        loader.finish();
        let answers = world.log().first().filter(|record| record.tick == saved_at + 1);
        let answers_in_the_first_record = answers.is_some_and(|record| record.arrivals.len() == awaited && record.arrivals.iter().all(|arrival| arrival.marked));
        let at_the_answers = |event: &&(u32, Event)| event.0 == saved_at + 1;
        // A reload shows as an arrival or as a change of hash. The event that
        // records a restore that started a new history, which the spike did
        // not have, is neither.
        let reload = |event: &Event| matches!(event, Event::Arrived { .. } | Event::HashChanged { .. });
        let reloads_on_the_answers = world.events().iter().filter(at_the_answers).filter(|(_, event)| reload(event)).count();
        let failures_on_the_answers = world.events().iter().filter(at_the_answers).filter(|(_, event)| matches!(event, Event::Failed { .. })).count();
        let instance = world.instances().first().copied();
        let instantiations = instance.map_or(0, |instance| instance.instantiations);
        let instance_failed = instance.is_some_and(|instance| instance.failed);
        let (expected, ours) = match mode {
            0 => (read_hashes(&directory.join("expected-state.bin")), world.state_hashes()),
            1 => (read_hashes(&directory.join("expected-changed.bin")), world.component_hashes()),
            _ => (read_hashes(&directory.join("expected-failed.bin")), world.component_hashes()),
        };
        let matches_expected = ours == expected && world.tick() == ticks;
        let held = world.entry(leaf4).map(|(availability, _)| availability);
        let leaf4_as_answered = match mode {
            0 => held == Some(Availability::Available(Catalog::leaf_hash(2, 4))),
            1 => held == Some(Availability::Available(Catalog::leaf_hash(3, 4))),
            _ => matches!(held, Some(Availability::Failed(_))) && result.failed_required.iter().any(|(id, _)| *id == leaf4),
        };
        holds = later_parts.is_ok()
            && first_tick_waits_for_every_answer
            && answers_in_the_first_record
            && reloads_on_the_answers == 0
            && failures_on_the_answers == usize::from(mode == 2)
            && instantiations == 2
            && instance_failed == (mode == 2)
            && leaf4_as_answered
            && matches_expected
            && !result.gated;
        out.string("compared", if mode == 0 { "state hashes with the uninterrupted run" } else { "component hashes with the oracle that changes leaf 4 at the first tick" });
        out.u64("assets_awaited", awaited as u64);
        out.bool("first_tick_waits_for_every_answer", first_tick_waits_for_every_answer);
        out.bool("answers_in_the_first_record", answers_in_the_first_record);
        out.u64("reload_events_on_the_answers", reloads_on_the_answers as u64);
        out.u64("failure_events_on_the_answers", failures_on_the_answers as u64);
        out.u64("instantiations_of_the_instance", u64::from(instantiations));
        out.bool("instance_failed", instance_failed);
        out.bool("leaf_4_as_answered", leaf4_as_answered);
        write_step(&mut out, "step", &catalog, &result);
        out.bool("matches_expected", matches_expected);
        out.i64("first_divergent_tick", divergence(&ours, &expected));
        out.hex("run_hash", run_hash(&ours));
    } else {
        // A snapshot read to continue its history: confirmed at the same hashes, or refused.
        let confirm = |asset: AssetId, hash: u64| -> Result<Arc<Resident>, String> {
            match manifest.sources.get(&asset) {
                Some(Source::Artifact { hash: named, .. }) if *named == hash => read_part(hash),
                Some(Source::Artifact { hash: named, .. }) => Err(format!("the content set names another hash, {named:#018x}")),
                _ => Err("the content set does not name the asset".to_string()),
            }
        };
        let restored = later_parts.map_err(|reason| Refusal { asset: 0, hash: 0, reason }).and_then(|()| World::restore(config, &saved, later, Links::new(store.clone()), Restore::Continue(&confirm)));
        match restored {
            Err(refusal) => {
                let names_the_hash = refusal.asset == leaf4 && refusal.hash == Catalog::leaf_hash(2, 4);
                holds = mode != 3 && names_the_hash;
                out.bool("refused", true);
                out.string("refused_for", &catalog.name(refusal.asset));
                out.hex("refused_at_hash", refusal.hash);
                out.string("reason", &refusal.reason);
                out.bool("names_the_recorded_hash", names_the_hash);
            }
            Ok(mut world) => {
                let saved_at = world.tick();
                let result = world.step(ticks - saved_at, Wait::No).unwrap_or_default();
                let expected = read_hashes(&directory.join("expected-state.bin"));
                let ours = world.state_hashes();
                let matches_expected = ours == expected && world.tick() == ticks && !result.gated;
                let record_unchanged = encode_log(world.log()) == recorded;
                let confirmations_in_the_record = world.log().iter().flat_map(|record| &record.arrivals).filter(|arrival| arrival.marked).count();
                holds = mode == 3 && matches_expected && record_unchanged && confirmations_in_the_record == 0;
                out.bool("refused", false);
                out.bool("matches_expected", matches_expected);
                out.i64("first_divergent_tick", divergence(&ours, &expected));
                out.bool("record_unchanged", record_unchanged);
                out.u64("confirmations_in_the_record", confirmations_in_the_record as u64);
                out.hex("run_hash", run_hash(&ours));
            }
        }
    }
    out.bool("holds", holds);
    out.object_end();
    out.finish()
}

// ---------------------------------------------------------------------------
// One resident copy
// ---------------------------------------------------------------------------

/// Sixteen worlds of one process want one large asset; each gets it at a
/// tick of its own. The live blocks of the size of its canonical part are
/// counted by the allocator.
fn one_resident_copy(out: &mut Json, settings: &Settings, catalog: &Catalog) -> bool {
    let store = Arc::new(ResidentStore::default());
    let size = Catalog::big_len(settings.big_payload);
    TRACKED_LIVE.store(0, Ordering::Relaxed);
    TRACKED_PEAK.store(0, Ordering::Relaxed);
    TRACKED_SIZE.store(size, Ordering::Relaxed);
    // The manifest names the hash; the artifact is read on a miss of the resident store only.
    let hash = fnv1a(&Catalog::big_part(settings.big_payload));
    TRACKED_PEAK.store(0, Ordering::Relaxed);
    let mut reads = 0_u64;
    let mut worlds = Vec::new();
    for index in 0..settings.shared_worlds {
        if store.get(hash).is_none() {
            reads += 1;
            store.insert(hash, Catalog::big_part(settings.big_payload)).expect("a valid part");
        }
        let config = WorldConfig::new(Size::Small, 1, Mode::Identifiers, vec![catalog.big]).wanting(CONTRIBUTOR, &[(catalog.big, Policy::Optional)]);
        let script = vec![Record::arrivals(index as u32 + 1, vec![Arrival::at(catalog.big, hash)])];
        worlds.push(World::new(config, script, Links::new(store.clone())));
    }
    let half = (settings.shared_worlds / 2) as u32;
    let mut stepped = true;
    for world in &mut worlds {
        stepped &= step_to(world, half);
    }
    let holding = |worlds: &[World]| worlds.iter().filter(|world| world.entry(catalog.big).is_some_and(|(availability, _)| availability == Availability::Available(hash))).count();
    let resident = store.get(hash).expect("resident");
    let (available_midway, live_midway, references_midway) = (holding(&worlds), TRACKED_LIVE.load(Ordering::Relaxed), Arc::strong_count(&resident) - 2);
    for world in &mut worlds {
        stepped &= step_to(world, settings.shared_ticks);
    }
    let (available_at_the_end, live_at_the_end, references_at_the_end) = (holding(&worlds), TRACKED_LIVE.load(Ordering::Relaxed), Arc::strong_count(&resident) - 2);
    let peak = TRACKED_PEAK.load(Ordering::Relaxed);
    // The control: a deliberate second copy is seen by the counter. The copy
    // goes through a black box: with the allocator declared in another crate,
    // an optimized build removes a copy nothing reads, and the control with it.
    let copy = std::hint::black_box(resident.bytes.clone());
    let live_with_a_second_copy = TRACKED_LIVE.load(Ordering::Relaxed);
    drop(copy);
    let mut run_hashes: Vec<u64> = worlds.iter().map(|world| run_hash(&world.state_hashes())).collect();
    run_hashes.sort_unstable();
    run_hashes.dedup();
    TRACKED_SIZE.store(usize::MAX, Ordering::Relaxed);
    let holds = stepped
        && reads == 1
        && live_midway == 1
        && live_at_the_end == 1
        && peak == 1
        && live_with_a_second_copy == 2
        && available_midway == half as usize
        && available_at_the_end == settings.shared_worlds
        && references_midway == half as usize
        && references_at_the_end == settings.shared_worlds
        && run_hashes.len() == settings.shared_worlds;
    out.key("one_resident_copy");
    out.object_start();
    out.u64("worlds", settings.shared_worlds as u64);
    out.u64("part_bytes", size as u64);
    out.u64("reads_of_the_artifact", reads);
    out.u64("worlds_holding_it_available_midway", available_midway as u64);
    out.i64("copies_midway", live_midway as i64);
    out.u64("references_held_by_worlds_midway", references_midway as u64);
    out.u64("worlds_holding_it_available_at_the_end", available_at_the_end as u64);
    out.i64("copies_at_the_end", live_at_the_end as i64);
    out.u64("references_held_by_worlds_at_the_end", references_at_the_end as u64);
    out.i64("peak_copies", peak as i64);
    out.i64("copies_with_a_deliberate_second_one", live_with_a_second_copy as i64);
    out.u64("distinct_run_hashes", run_hashes.len() as u64);
    out.bool("holds", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// The scenarios, run one after the other
// ---------------------------------------------------------------------------

/// Runs the spike's scenarios on the prototype's table and world. The caller
/// has opened an object in `out`: the function writes into it, under
/// `settings`, which settings ran and the spike's comparison of its oracle
/// with the sequential reference, then the facts of each scenario under the
/// scenario's own key, as the spike's report held them. Returns the verdict
/// of each scenario, named by that key, in the order they ran.
pub fn run(out: &mut Json, settings: &Settings) -> Vec<(&'static str, bool)> {
    let catalog = Catalog::new();
    let reference = run_sequential(&settings.parameters());
    out.key("settings");
    out.object_start();
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", settings.size.name());
    out.u64("ticks", u64::from(settings.ticks));
    out.string("worker_counts", &settings.worker_counts.iter().map(usize::to_string).collect::<Vec<_>>().join(", "));
    out.hex("reference_run_hash", reference.run_hash);
    out.decimal("reference_ticks_per_second", reference.ticks_per_second());
    let every_leaf: Vec<Switch> = (0..LEAVES).map(|leaf| switch(1, leaf, SPECIFICATION)).collect();
    out.bool("oracle_with_every_leaf_at_the_specification_equals_reference", oracle(&settings.parameters(), LEAVES, &every_leaf) == reference.tick_hashes);
    out.hex("leaf_content_hash_at_the_specification", Catalog::leaf_hash(1, 0));
    out.object_end();

    vec![
        ("wants", wants(out, settings, &catalog)),
        ("gate", gate(out, settings, &catalog)),
        ("revision", revision(out, settings, &catalog)),
        ("table_under_the_hash", table_under_the_hash(out, settings, &catalog)),
        ("save", save(out, settings, &catalog)),
        ("one_resident_copy", one_resident_copy(out, settings, &catalog)),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    fn small() -> Settings {
        Settings {
            size: Size::Small,
            ticks: 100,
            worker_counts: vec![1, 3],
            clock_delay_ms: (2, 10),
            explicit_delays_ms: vec![0],
            deadline: Duration::from_millis(5),
            revision_spread: Duration::from_millis(20),
            batch_pause: Duration::from_millis(1),
            shared_worlds: 16,
            shared_ticks: 20,
            big_payload: 1 << 16,
            children_in_process: true,
        }
    }

    /// Runs one scenario on the small settings and returns its verdict and its report.
    fn scenario(run: impl FnOnce(&mut Json, &Settings, &Catalog) -> bool) -> (bool, String) {
        let mut out = Json::new();
        out.object_start();
        let holds = run(&mut out, &small(), &Catalog::new());
        out.object_end();
        (holds, out.finish())
    }

    #[test]
    fn wants_hold_on_the_small_size() {
        let (holds, report) = scenario(wants);
        assert!(holds, "{report}");
    }

    #[test]
    fn the_gate_holds_on_the_small_size() {
        let (holds, report) = scenario(gate);
        assert!(holds, "{report}");
    }

    #[test]
    fn a_revision_is_one_batch_on_the_small_size() {
        let (holds, report) = scenario(revision);
        assert!(holds, "{report}");
    }

    #[test]
    fn the_table_is_under_the_hash() {
        let (holds, report) = scenario(table_under_the_hash);
        assert!(holds, "{report}");
    }

    #[test]
    fn a_save_is_read_as_a_new_run_and_to_continue_its_history() {
        let (holds, report) = scenario(save);
        assert!(holds, "{report}");
        assert_eq!(report.matches("\"refused\":true").count(), 2, "{report}");
    }

    #[test]
    fn sixteen_worlds_hold_one_copy() {
        let _alone = counters();
        let (holds, report) = scenario(one_resident_copy);
        assert!(holds, "{report}");
    }

    #[test]
    fn the_settings_place_their_ticks() {
        let settings = small();
        assert_eq!((settings.at(100), settings.at(905), settings.at(1)), (10, 90, 1));
    }

    /// The allocation counters are the process's own: the two tests that run
    /// the scenario of the one resident copy run one at a time.
    static COUNTERS: Mutex<()> = Mutex::new(());

    fn counters() -> MutexGuard<'static, ()> {
        COUNTERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What a hosting binary calls. The verdicts themselves are asserted
    /// above, one scenario per test.
    #[test]
    fn a_run_writes_every_scenario_under_its_own_key_and_names_its_verdict_by_it() {
        let _alone = counters();
        let mut out = Json::new();
        out.object_start();
        let verdicts = run(&mut out, &small());
        out.object_end();
        let report = out.finish();
        let names: Vec<&str> = verdicts.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["wants", "gate", "revision", "table_under_the_hash", "save", "one_resident_copy"]);
        for name in names {
            assert_eq!(report.matches(&format!("\"{name}\":{{")).count(), 1, "{name} in {report}");
        }
        assert!(report.contains("\"settings\":{") && report.contains("\"size\":\"small\",\"ticks\":100,\"worker_counts\":\"1, 3\""), "{report}");
        assert!(report.contains("\"oracle_with_every_leaf_at_the_specification_equals_reference\":true"), "{report}");
    }
}
