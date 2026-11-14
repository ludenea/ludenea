//! The cost of the gate evaluated on a large table: a magnitude, with no
//! oracle and no control. Throwaway, never product code.
//!
//! A world of the reference workload at default size holds a table of ten
//! thousand assets, two thousand roots each wanted as required by a command
//! and eight thousand leaves under the wants the core added. One evaluation
//! of the gate on a pending record, which it does not apply, is timed a
//! hundred times and its median is set against the median duration of one
//! tick of the same world, over a hundred ticks with an empty record and an
//! open gate.

use std::sync::Arc;
use std::time::{Duration, Instant};

use asset_loader::ResidentStore;
use content_store::content_hash;
use prototype_core::table::AssetType::Parameters;
use prototype_core::table::{Arrival, AssetId, Availability, Command, Dependency, Maker, Owner, Policy, Record, RepulsionTable, Rules, part_bytes};
use prototype_core::world::{Links, Mode, Size, Wait, World, WorldConfig};
use reference_workload::SplitMix64;

const ROOTS: usize = 2_000;
const LEAVES_OF_A_ROOT: usize = 4;
const LEAVES: usize = ROOTS * LEAVES_OF_A_ROOT;
/// The entities name the first sixteen leaves, in turn.
const POOL: usize = 16;
const EVALUATIONS: usize = 100;
/// The ticks whose duration is the one a gate's evaluation is set against.
const FIRST_TIMED_TICK: u32 = 3;
const LAST_TIMED_TICK: u32 = 102;
const HOST: Owner = Owner::World(Maker::Source(3));
const TOOL: Owner = Owner::World(Maker::Source(5));

/// The assets of the family, their contents, and the parts made resident.
struct Catalog {
    roots: Vec<AssetId>,
    leaves: Vec<AssetId>,
    /// The asset the table does not hold, which closes the gate.
    gate: AssetId,
    root_hashes: Vec<u64>,
    leaf_hashes: Vec<u64>,
    gate_hash: u64,
    /// The second content of the first root: the same four leaves.
    second_of_the_first_root: u64,
    /// The second content of each of the first thousand roots: its four
    /// leaves and the first leaf of the next root.
    second_with_a_leaf_more: Vec<u64>,
    residents: Arc<ResidentStore>,
}

impl Catalog {
    /// The identifiers are drawn from the fixed stream of the spikes, in no
    /// relation to the names.
    fn new() -> Catalog {
        let mut stream = SplitMix64::new(0x6173_7365_7420_6964);
        let mut draw = || (u128::from(stream.next()) << 64) | u128::from(stream.next() | 1);
        let roots: Vec<AssetId> = (0..ROOTS).map(|_| draw()).collect();
        let leaves: Vec<AssetId> = (0..LEAVES).map(|_| draw()).collect();
        let gate = draw();
        let residents = Arc::new(ResidentStore::default());
        let resident = |strength: f32, dependencies: &[Dependency], salt: u32| {
            let bytes = part_bytes(RepulsionTable { strength, radius: 8.0 }, dependencies, 16, salt);
            let hash = content_hash(&bytes);
            residents.insert(hash, bytes).expect("a part of the family is valid");
            hash
        };
        let listed = |root: usize, more: usize| -> Vec<Dependency> {
            (root * LEAVES_OF_A_ROOT..root * LEAVES_OF_A_ROOT + LEAVES_OF_A_ROOT + more).map(|leaf| (leaves[leaf], Parameters)).collect()
        };
        let leaf_hashes = (0..LEAVES).map(|leaf| resident(24.0 + (leaf % POOL) as f32, &[], leaf as u32)).collect();
        let root_hashes = (0..ROOTS).map(|root| resident(40.0, &listed(root, 0), root as u32)).collect();
        let second_of_the_first_root = resident(41.0, &listed(0, 0), 100_000);
        let second_with_a_leaf_more = (0..ROOTS / 2).map(|root| resident(42.0, &listed(root, 1), 200_000 + root as u32)).collect();
        let gate_hash = resident(43.0, &[], 300_000);
        Catalog { roots, leaves, gate, root_hashes, leaf_hashes, gate_hash, second_of_the_first_root, second_with_a_leaf_more, residents }
    }

    /// The small pending record: one release and one arrival.
    fn small(&self) -> (Vec<Command>, Vec<Arrival>) {
        (vec![Command::Release { asset: self.roots[1_000], owner: HOST }], vec![Arrival::at(self.roots[0], self.second_of_the_first_root)])
    }

    /// The large pending record: fifty releases, fifty optional wants on
    /// leaves the table holds, and a thousand arrivals of roots, each at a
    /// content that lists one leaf more, which the table holds available.
    fn large(&self) -> (Vec<Command>, Vec<Arrival>) {
        let releases = (1_000..1_050).map(|root| Command::Release { asset: self.roots[root], owner: HOST });
        let wants = (4_000..4_050).map(|leaf| Command::Want { asset: self.leaves[leaf], asset_type: Parameters, owner: TOOL, policy: Policy::Optional });
        let arrivals = self.second_with_a_leaf_more.iter().enumerate().map(|(root, hash)| Arrival::at(self.roots[root], *hash)).collect();
        (releases.chain(wants).collect(), arrivals)
    }
}

/// One figure: a case, a size of pending record and a worker count.
#[derive(Clone, Debug)]
pub struct Cell {
    pub case: &'static str,
    pub size: &'static str,
    pub workers: usize,
    /// The median of a hundred evaluations of the gate.
    pub evaluation: Duration,
    /// Of an evaluation, the median of the copy of the table alone: zero
    /// under a form that copies nothing.
    pub copy: Duration,
    /// The median duration of one tick of the same world.
    pub tick: Duration,
    /// Whether the verdict is the one the case states: open, or closed with
    /// the asset awaited named.
    pub verdict_as_stated: bool,
}

impl Cell {
    pub fn ratio(&self) -> f64 {
        self.evaluation.as_secs_f64() / self.tick.as_secs_f64()
    }
}

/// What the family measured.
pub struct GateCost {
    /// Per repetition, the eighteen figures of the build as it stands.
    pub repetitions: Vec<Vec<Cell>>,
    /// Per repetition, the case `open` again under the other reading of the
    /// gate, in which the pending record of a world that is not gated is read.
    pub open_with_the_record_read: Vec<Vec<Cell>>,
    /// Whether every world held ten thousand entries, all available, from
    /// tick 2 on, and whether a record built for an evaluation is the one the
    /// world assembles and then applies.
    pub table_as_stated: bool,
    pub record_as_assembled: bool,
    /// The form that evaluated the gate of the measured worlds, in words.
    pub form: &'static str,
}

impl GateCost {
    /// The figure the claim is read on: the largest of the ratios.
    pub fn largest(&self) -> f64 {
        self.repetitions.iter().flatten().map(Cell::ratio).fold(0.0, f64::max)
    }

    /// The largest ratio of the copy of the table alone to a tick.
    pub fn largest_copy(&self) -> f64 {
        self.repetitions.iter().chain(&self.open_with_the_record_read).flatten().map(|cell| cell.copy.as_secs_f64() / cell.tick.as_secs_f64()).fold(0.0, f64::max)
    }

    pub fn largest_with_the_record_read(&self) -> f64 {
        self.open_with_the_record_read.iter().flatten().map(Cell::ratio).fold(0.0, f64::max)
    }
}

fn median(durations: &mut [Duration]) -> Duration {
    durations.sort_unstable();
    let middle = durations.len() / 2;
    if durations.len().is_multiple_of(2) { (durations[middle - 1] + durations[middle]) / 2 } else { durations[middle] }
}

fn record(tick: u32, (commands, mut arrivals): (Vec<Command>, Vec<Arrival>)) -> Record {
    arrivals.sort_by_key(|arrival| arrival.asset);
    Record { tick, commands, arrivals }
}

/// A hundred evaluations of the gate on one pending record, which none of
/// them applies: their median, the median of the copy of the table inside
/// them, and what the last one named as awaited.
fn evaluate(world: &World, record: &Record) -> (Duration, Duration, Vec<AssetId>) {
    let mut durations = Vec::with_capacity(EVALUATIONS);
    let mut copies = Vec::with_capacity(EVALUATIONS);
    let mut awaited = Vec::new();
    for _ in 0..EVALUATIONS {
        let started = Instant::now();
        let (named, copied, _) = world.gate_in_two_parts(record);
        durations.push(started.elapsed());
        copies.push(copied);
        awaited = named;
    }
    (median(&mut durations), median(&mut copies), awaited)
}

/// A world brought to the boundary after tick 102: the wants of the roots
/// in the record of tick 1, the arrivals of the ten thousand assets in the
/// record of tick 2, then a hundred ticks with an empty record. Returns the
/// world, its queue, the median duration of those hundred ticks, and whether
/// its table is the one the family states.
fn world(catalog: &Catalog, workers: usize, rules: Rules) -> (World, Arc<prototype_core::table::ArrivalQueue>, Duration, bool) {
    let links = Links::new(catalog.residents.clone());
    let queue = links.queue.clone();
    let mut config = WorldConfig::new(Size::Default, workers, Mode::Identifiers, catalog.leaves[..POOL].to_vec());
    config.rules = rules;
    let mut world = World::new(config, Vec::new(), links);
    for root in &catalog.roots {
        world.post(Command::Want { asset: *root, asset_type: Parameters, owner: HOST, policy: Policy::Required });
    }
    let first = world.step(1, Wait::No).expect("the world does not fail");
    assert_eq!(first.ticks_ran, 1, "the tick of the wants runs");
    let arrivals: Vec<Arrival> = catalog
        .roots
        .iter()
        .zip(&catalog.root_hashes)
        .chain(catalog.leaves.iter().zip(&catalog.leaf_hashes))
        .map(|(asset, hash)| Arrival::at(*asset, *hash))
        .collect();
    queue.post(&arrivals);
    let ran = world.step(LAST_TIMED_TICK - 1, Wait::No).expect("the world does not fail");
    assert_eq!(ran.ticks_ran, LAST_TIMED_TICK - 1, "the gate opens on the one batch");
    let mut ticks: Vec<Duration> =
        world.tick_durations().into_iter().filter(|(tick, _)| (FIRST_TIMED_TICK..=LAST_TIMED_TICK).contains(tick)).map(|(_, duration)| duration).collect();
    assert_eq!(ticks.len(), (LAST_TIMED_TICK - FIRST_TIMED_TICK + 1) as usize, "a hundred ticks are timed");
    let entries = world.entries();
    let as_stated = entries.len() == ROOTS + LEAVES && entries.iter().all(|entry| matches!(entry.availability, Availability::Available(_)));
    (world, queue, median(&mut ticks), as_stated)
}

/// The eighteen figures of one repetition are made of three worlds, one per
/// worker count; the case `open` under the other reading takes three more.
fn repetition(catalog: &Catalog, worker_counts: &[usize], out: &mut GateCost) {
    let mut cells = Vec::new();
    let mut read = Vec::new();
    for &workers in worker_counts {
        let (mut measured, queue, tick, as_stated) = world(catalog, workers, Rules::default());
        out.table_as_stated &= as_stated;
        out.form = measured.gate_form();
        let sizes = [("small", catalog.small()), ("large", catalog.large())];
        // Open: every required asset is answered.
        for (size, elements) in &sizes {
            let (evaluation, copy, awaited) = evaluate(&measured, &record(LAST_TIMED_TICK + 1, elements.clone()));
            cells.push(Cell { case: "open", size, workers, evaluation, copy, tick, verdict_as_stated: awaited.is_empty() });
        }
        // Closed: the record of tick 103 wants the one asset nothing answers.
        measured.post(Command::Want { asset: catalog.gate, asset_type: Parameters, owner: HOST, policy: Policy::Required });
        assert_eq!(measured.step(1, Wait::No).expect("the world does not fail").ticks_ran, 1, "the tick of the want runs");
        for (size, elements) in &sizes {
            let (evaluation, copy, awaited) = evaluate(&measured, &record(LAST_TIMED_TICK + 2, elements.clone()));
            cells.push(Cell { case: "closed", size, workers, evaluation, copy, tick, verdict_as_stated: awaited == [catalog.gate] });
        }
        // Opens: the pending record holds the arrival of that asset as well.
        let mut last = None;
        for (size, (commands, arrivals)) in &sizes {
            let mut arrivals = arrivals.clone();
            arrivals.push(Arrival::at(catalog.gate, catalog.gate_hash));
            let pending = record(LAST_TIMED_TICK + 2, (commands.clone(), arrivals));
            let (evaluation, copy, awaited) = evaluate(&measured, &pending);
            cells.push(Cell { case: "opens", size, workers, evaluation, copy, tick, verdict_as_stated: awaited.is_empty() });
            last = Some(pending);
        }
        // The last record, built by hand for its evaluations, is the one the
        // world assembles from what is posted, and the world then runs.
        let pending = last.expect("two sizes");
        for command in &pending.commands {
            measured.post(*command);
        }
        queue.post(&pending.arrivals);
        out.record_as_assembled &= measured.pending_record() == pending;
        out.record_as_assembled &= measured.step(1, Wait::No).expect("the world does not fail").ticks_ran == 1;

        // The other reading: the pending record of a world that is not gated is read.
        let other = Rules { gate_read_on_a_world_that_is_not_gated: true, ..Rules::default() };
        let (measured, _, tick, as_stated) = world(catalog, workers, other);
        out.table_as_stated &= as_stated;
        for (size, elements) in &sizes {
            let (evaluation, copy, awaited) = evaluate(&measured, &record(LAST_TIMED_TICK + 1, elements.clone()));
            read.push(Cell { case: "open", size, workers, evaluation, copy, tick, verdict_as_stated: awaited.is_empty() });
        }
    }
    out.repetitions.push(cells);
    out.open_with_the_record_read.push(read);
}

/// Measures the family: `repetitions` times, at each worker count.
pub fn run(repetitions: usize, worker_counts: &[usize]) -> GateCost {
    let catalog = Catalog::new();
    let mut out = GateCost { repetitions: Vec::new(), open_with_the_record_read: Vec::new(), table_as_stated: true, record_as_assembled: true, form: "" };
    for _ in 0..repetitions {
        repetition(&catalog, worker_counts, &mut out);
    }
    out
}
