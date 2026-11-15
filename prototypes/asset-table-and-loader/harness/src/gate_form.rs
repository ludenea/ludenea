//! The cost of the gate in its incremental form: a magnitude, with no
//! oracle and no control. Throwaway, never product code.
//!
//! The family `gate-form` of the sheet of the stand-in stage, on the world
//! and the table of the family `gate-cost`. Three measurements, each set
//! against the median duration of one tick of that world:
//!
//! - one evaluation of the gate on the large table, in the three cases of
//!   `gate-cost` and its two sizes of pending record;
//! - one evaluation on a restored table that awaits an answer for each of
//!   its available entries, read in a process of its own, with nothing, one
//!   closure, half and every answer pending: the verdict alone, then the
//!   whole answer, which also names the assets awaited;
//! - the cost the form moves to every change of a want or of an
//!   availability: two hundred ticks of a running world that is never gated
//!   and whose wants and availabilities change at every tick, run with the
//!   form and in a variant of the build that keeps neither the set nor the
//!   count and whose gate the host does not ask.
//!
//! The first two are read against a tenth of a tick. The third has no bound
//! and is reported.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command as Process;
use std::time::{Duration, Instant};

use prototype_core::table::AssetType::Parameters;
use prototype_core::table::{Arrival, AssetId, Availability, Command, Policy, Record, Rules, Table};
use prototype_core::world::{History, Links, Mode, Restore, Wait, World, WorldConfig};
use reference_workload::Json;

use crate::large_table::{
    Built, Contents, FirstContents, HOST, Identifiers, LAST_TIMED_TICK, LEAVES_OF_A_ROOT, Length, POOL, Plan, Scale, TOOL, built, median_tick, record,
};
use crate::magnitude::{arguments_or_exit, conditions, finish, head, median, nanoseconds, ratio, write_ratio};

/// The bound of the claim: a tenth of a tick.
pub const BOUND: f64 = 0.1;
/// The payload of a part of the family `gate-cost`, in bytes.
const PAYLOAD: Length = Length::Payload(16);
/// The ticks of the third measurement: the records change the wants and the
/// availabilities from tick 103 to tick 302, and each whole tick is timed
/// from tick 123 on.
const FIRST_CHANGING_TICK: u32 = LAST_TIMED_TICK + 1;
const CHANGING_TICKS: u32 = 200;
const FIRST_TIMED_CHANGING_TICK: u32 = FIRST_CHANGING_TICK + 20;
const LAST_CHANGING_TICK: u32 = FIRST_CHANGING_TICK + CHANGING_TICKS - 1;
/// The four cases of the second measurement, by what the pending record of
/// the restored world's first tick holds.
const RESTORED_CASES: [&str; 4] = ["restore-nothing-pending", "restore-one-closure-pending", "restore-half-pending", "restore-all-pending"];
/// How the build keeps what its gate reads, beside the form the world names.
const HOW_THE_SET_AND_THE_COUNT_ARE_KEPT: &str = "the set of the required assets the table holds unanswered is an ordered set of the table, brought up to date at each want applied, released or ended, each want the core adds, changes or ends, each arrival applied, each rejection that fails an entry and each fall; \
     the count of the entries a restore still awaits is the length of the set of the awaited entries, from which an entry is taken when its answer is applied or when it leaves the table; neither is part of the canonical bytes; \
     the verdict reads the emptiness of the set, that length and the pending arrivals and releases, and the awaited entries are walked only to name them";

/// The variant of the build the third measurement runs beside the form: it
/// keeps neither the set nor the count, and the host does not ask its gate.
fn without_the_form() -> Rules {
    Rules { required_set_not_kept: true, gate_not_asked: true, ..Rules::default() }
}

/// The contents of the family: those of `gate-cost`, and those the third
/// measurement adds.
struct Catalog {
    scale: Scale,
    identifiers: Identifiers,
    contents: Contents,
    first: FirstContents,
    /// The one content of the asset the table does not hold.
    of_the_asset_outside: u64,
    /// The second content of the first root: the same four leaves.
    second_of_the_first_root: u64,
    /// The second content of each root of the first half: its four leaves
    /// and the first leaf of the next root.
    second_with_a_leaf_more: Vec<u64>,
    /// A content of the first root that lists the asset the table does not
    /// hold: what tells whether a gate read a pending record.
    of_the_first_root_listing_the_asset_outside: u64,
    /// The one content of each asset the third measurement adds.
    of_the_added: Vec<u64>,
    /// The second content of each leaf the third measurement replaces, the
    /// first of them at the rank `first_replaced_leaf`.
    second_of_the_leaves: Vec<u64>,
}

impl Catalog {
    fn new(scale: Scale) -> Catalog {
        let roots = scale.roots;
        let identifiers = Identifiers::drawn(roots);
        let mut contents = Contents::default();
        let first = FirstContents::of(&mut contents, &identifiers, roots, PAYLOAD);
        let second_of_the_first_root = contents.resident(41.0, &identifiers.listed(0, 0), PAYLOAD, 100_000);
        let second_with_a_leaf_more = (0..roots / 2).map(|root| contents.resident(42.0, &identifiers.listed(root, 1), PAYLOAD, 200_000 + root as u32)).collect();
        let of_the_asset_outside = contents.resident(43.0, &[], PAYLOAD, 300_000);
        let mut listed = identifiers.listed(0, 0);
        listed.push((identifiers.gate, Parameters));
        let of_the_first_root_listing_the_asset_outside = contents.resident(45.0, &listed, PAYLOAD, 700_000);
        let of_the_added = (0..roots).map(|rank| contents.resident(44.0, &[], PAYLOAD, 500_000 + rank as u32)).collect();
        let unit = roots / 20;
        let second_of_the_leaves =
            (4 * unit..40 * unit).map(|leaf| contents.resident(24.5 + (leaf % POOL) as f32, &[], PAYLOAD, 600_000 + leaf as u32)).collect();
        Catalog {
            scale,
            identifiers,
            contents,
            first,
            of_the_asset_outside,
            second_of_the_first_root,
            second_with_a_leaf_more,
            of_the_first_root_listing_the_asset_outside,
            of_the_added,
            second_of_the_leaves,
        }
    }

    fn roots(&self) -> usize {
        self.scale.roots
    }

    /// A hundred where the table is the stated one: what the counts of the
    /// third measurement are multiples of.
    fn unit(&self) -> usize {
        self.roots() / 20
    }

    fn want(asset: AssetId, policy: Policy) -> Command {
        Command::Want { asset, asset_type: Parameters, owner: HOST, policy }
    }

    fn release(asset: AssetId) -> Command {
        Command::Release { asset, owner: HOST }
    }

    /// The small pending record: the release of the want of the root at the
    /// middle of the table, and the arrival of the first root at a second
    /// content that lists the same four leaves.
    fn small(&self) -> (Vec<Command>, Vec<Arrival>) {
        let arrival = self.contents.arrival(self.identifiers.roots[0], self.second_of_the_first_root);
        (vec![Catalog::release(self.identifiers.roots[self.roots() / 2])], vec![arrival])
    }

    /// The large pending record. On the stated table: 50 releases by
    /// `source:host` of its wants of the roots from the middle of the table
    /// on, 50 optional wants of `source:tool` on the leaves of those roots,
    /// and the arrivals of the first thousand roots, each at a second
    /// content that lists its four leaves and the first leaf of the next
    /// root, which the table holds available.
    fn large(&self) -> (Vec<Command>, Vec<Arrival>) {
        let half = self.roots() / 2;
        let commands = half / 20;
        let releases = (half..half + commands).map(|root| Catalog::release(self.identifiers.roots[root]));
        let first_leaf = half * LEAVES_OF_A_ROOT;
        let wants = (first_leaf..first_leaf + commands).map(|leaf| Command::Want { asset: self.identifiers.leaves[leaf], asset_type: Parameters, owner: TOOL, policy: Policy::Optional });
        let arrivals = self.second_with_a_leaf_more.iter().enumerate().map(|(root, hash)| self.contents.arrival(self.identifiers.roots[root], *hash)).collect();
        (releases.chain(wants).collect(), arrivals)
    }

    /// A pending record that closes a gate which reads it: the arrival of
    /// the first root at a content that lists an asset the table does not
    /// hold.
    fn closes_a_gate_that_reads_it(&self, tick: u32) -> Record {
        record(tick, Vec::new(), vec![self.contents.arrival(self.identifiers.roots[0], self.of_the_first_root_listing_the_asset_outside)])
    }

    /// The record of the tick `103 + j` of the third measurement, in the
    /// order the sheet states its elements. With `u` a hundred on the stated
    /// table: the required wants of `u` added assets; the releases of the
    /// `u` added assets wanted ten ticks before; the releases of `u / 5`
    /// roots of the second half; the required wants of the roots released
    /// the tick before; a change of policy of the wants of the first `u`
    /// roots, optional where `j` is even and required where it is odd; the
    /// arrivals of the added assets wanted by this record; and the arrivals
    /// of `u` leaves, each at its second content where the table holds it at
    /// its first, and at its first where the table holds it at its second.
    fn changing(&self, j: usize) -> Record {
        let (unit, identifiers) = (self.unit(), &self.identifiers);
        let added = |of: usize| unit * (of % 20)..unit * (of % 20) + unit;
        let released = |of: usize| {
            let (per_tick, half) = (unit / 5, self.roots() / 2);
            half + per_tick * (of % 50)..half + per_tick * (of % 50) + per_tick
        };
        let mut commands = Vec::new();
        commands.extend(added(j).map(|rank| Catalog::want(identifiers.added[rank], Policy::Required)));
        if j >= 10 {
            commands.extend(added(j - 10).map(|rank| Catalog::release(identifiers.added[rank])));
        }
        commands.extend(released(j).map(|root| Catalog::release(identifiers.roots[root])));
        if j >= 1 {
            commands.extend(released(j - 1).map(|root| Catalog::want(identifiers.roots[root], Policy::Required)));
        }
        let policy = if j.is_multiple_of(2) { Policy::Optional } else { Policy::Required };
        commands.extend((0..unit).map(|root| Catalog::want(identifiers.roots[root], policy)));
        let mut arrivals: Vec<Arrival> = added(j).map(|rank| self.contents.arrival(identifiers.added[rank], self.of_the_added[rank])).collect();
        let first_replaced_leaf = 4 * unit;
        let first = first_replaced_leaf + unit * (j % 36);
        let to_the_second = (j / 36).is_multiple_of(2);
        arrivals.extend((first..first + unit).map(|leaf| {
            let hash = if to_the_second { self.second_of_the_leaves[leaf - first_replaced_leaf] } else { self.first.leaves[leaf] };
            self.contents.arrival(identifiers.leaves[leaf], hash)
        }));
        record(FIRST_CHANGING_TICK + j as u32, commands, arrivals)
    }

    /// The assets whose entry a record of the third measurement changes:
    /// each asset a command or an arrival names and, where it is a root, its
    /// four leaves, on which the want of the core follows the root's.
    fn changed_by(&self, record: &Record) -> Vec<AssetId> {
        let roots = &self.identifiers.roots[..self.roots()];
        let mut changed = Vec::new();
        let mut name = |asset: AssetId| {
            changed.push(asset);
            if let Some(root) = roots.iter().position(|other| *other == asset) {
                changed.extend(self.identifiers.listed(root, 0).into_iter().map(|(leaf, _)| leaf));
            }
        };
        for command in &record.commands {
            if let Command::Want { asset, .. } | Command::Release { asset, .. } = command {
                name(*asset);
            }
        }
        for arrival in &record.arrivals {
            name(arrival.asset);
        }
        changed
    }

    fn plan(&self, workers: usize, rules: Rules, script: Vec<Record>) -> Plan<'_> {
        Plan { scale: self.scale, roots: self.roots(), workers, rules, budget: None, script, identifiers: &self.identifiers, contents: &self.contents, first: &self.first }
    }
}

/// One figure of the first measurement: a case, a size of pending record
/// and a worker count.
#[derive(Clone, Debug)]
pub struct Evaluation {
    pub case: &'static str,
    pub size: &'static str,
    pub workers: usize,
    /// The elements of the pending record.
    pub pending: usize,
    /// The median of the evaluations of the gate, none of which applies the record.
    pub evaluation: Duration,
    /// The median duration of one tick of the same world.
    pub tick: Duration,
    /// Whether the verdict is the one the case states: open, or closed with
    /// the asset awaited named.
    pub verdict_as_stated: bool,
}

/// One figure of the second measurement: a case and a worker count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    pub case: String,
    pub workers: usize,
    /// The elements of the pending record: its marked answers.
    pub pending: usize,
    /// The entries still awaited once the pending record is counted.
    pub awaited: usize,
    pub open: bool,
    /// The median of the verdict alone, open or closed, with that count.
    pub verdict: Duration,
    /// The median of the whole answer, which also names the assets awaited.
    pub whole: Duration,
    /// Where the gate is closed: the median of a step without waiting as
    /// the host makes it, the pending record assembled and copied by it.
    pub step: Option<Duration>,
    /// Whether the verdict, the count and the names are those the case states.
    pub as_stated: bool,
}

/// What the reader of a save gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reader {
    pub cases: Vec<Restored>,
    /// The median time to derive, from the restored table, the set the gate
    /// reads and the entries it awaits: no evaluation.
    pub derivation: Duration,
    /// Whether the restored world awaited every available entry, assembled
    /// each pending record as stated, ran its first tick once every answer
    /// was pending, and continues the saved run.
    pub as_stated: bool,
}

/// One figure of the third measurement: a worker count.
#[derive(Clone, Debug)]
pub struct Changes {
    pub workers: usize,
    /// The median duration of a whole tick from tick 123 to tick 302, with
    /// the form and without it.
    pub with_the_form: Duration,
    pub without_the_form: Duration,
    /// Of those ticks, the median duration of the input fixed point alone,
    /// where every change of a want and of an availability is made, with
    /// the form and without it: beside what the sheet asks for.
    pub fixed_point_with_the_form: Duration,
    pub fixed_point_without_the_form: Duration,
    /// The median duration of the ticks 3 to 102 of the run with the form,
    /// each with an empty record and an open gate.
    pub tick: Duration,
    /// The mean count, per tick from tick 123 to tick 302, of the changes of
    /// a want and of an availability the table followed, the wants the core
    /// adds and ends included.
    pub changes_per_tick: f64,
    /// Beside what the sheet asks for: the median time the table takes, as
    /// tick 302 leaves it, to bring its kept set up to date again for each
    /// asset the last record changes, with the form and without it, and how
    /// many assets that is.
    pub following_with_the_form: Duration,
    pub following_without_the_form: Duration,
    pub followed: usize,
    /// Whether the two runs have the same state hash at every tick.
    pub state_hashes_equal: bool,
    /// Whether both runs ran every tick, were never gated, applied the
    /// records as stated and followed the same count of changes.
    pub as_stated: bool,
}

impl Changes {
    /// The difference of the two medians, in nanoseconds: negative where
    /// the run without the form took longer.
    pub fn difference(&self) -> i64 {
        self.with_the_form.as_nanos() as i64 - self.without_the_form.as_nanos() as i64
    }

    /// The same difference, of the input fixed points alone.
    pub fn difference_of_the_fixed_points(&self) -> i64 {
        self.fixed_point_with_the_form.as_nanos() as i64 - self.fixed_point_without_the_form.as_nanos() as i64
    }

    /// What bringing the kept set up to date costs for one asset, in
    /// nanoseconds, read from the assets followed again: with the form less
    /// without it, over their count.
    pub fn following_per_change(&self) -> f64 {
        (self.following_with_the_form.as_nanos() as f64 - self.following_without_the_form.as_nanos() as f64) / self.followed as f64
    }
}

/// What the family measured.
#[derive(Default)]
pub struct GateForm {
    /// Per repetition: the figures of each measurement.
    pub evaluations: Vec<Vec<Evaluation>>,
    pub restored: Vec<Vec<(Restored, Duration)>>,
    /// Per repetition and worker count: the derivation from the restored table.
    pub derivations: Vec<Vec<(usize, Duration)>>,
    pub changes: Vec<Vec<Changes>>,
    /// The form that evaluated the gate of the measured worlds, in words.
    pub form: &'static str,
    /// Whether, in the case `open`, the gate read the pending record.
    pub open_reads_the_pending_record: bool,
    /// Whether every table, every record and every world was as stated.
    pub as_stated: bool,
}

impl GateForm {
    /// Every ratio of the first two measurements, each with what it is of.
    fn ratios(&self) -> Vec<(f64, String)> {
        let mut ratios = Vec::new();
        for (repetition, cells) in self.evaluations.iter().enumerate() {
            for cell in cells {
                ratios.push((ratio(cell.evaluation, cell.tick), format!("{} {} at {} workers, repetition {}", cell.case, cell.size, cell.workers, repetition + 1)));
            }
        }
        for (repetition, cells) in self.restored.iter().enumerate() {
            for (cell, tick) in cells {
                let of = |what: &str| format!("{} ({what}) at {} workers, repetition {}", cell.case, cell.workers, repetition + 1);
                ratios.push((ratio(cell.verdict, *tick), of("the verdict alone")));
                ratios.push((ratio(cell.whole, *tick), of("the whole answer")));
            }
        }
        ratios
    }

    /// The figure the claim is read on: the largest ratio of one evaluation
    /// to one tick over the first two measurements, and what it is of.
    pub fn largest(&self) -> (f64, String) {
        self.ratios().into_iter().max_by(|a, b| a.0.total_cmp(&b.0)).unwrap_or((0.0, String::new()))
    }

    pub fn every_verdict_as_stated(&self) -> bool {
        self.evaluations.iter().flatten().all(|cell| cell.verdict_as_stated) && self.restored.iter().flatten().all(|(cell, _)| cell.as_stated)
    }

    /// Whether the claim holds on what was measured: every evaluation under
    /// a tenth of a tick, every verdict as stated, every world as stated.
    /// None where no evaluation was measured: the claim is read on the
    /// first two measurements alone.
    pub fn holds(&self) -> Option<bool> {
        let measured = self.evaluations.iter().any(|cells| !cells.is_empty());
        measured.then(|| self.largest().0 < BOUND && self.every_verdict_as_stated() && self.as_stated)
    }
}

/// The evaluations of the gate on one pending record, which none of them
/// applies: their median, and what the last one named as awaited.
fn evaluate(world: &World, pending: &Record, evaluations: usize) -> (Duration, Vec<AssetId>) {
    let mut durations = Vec::with_capacity(evaluations);
    let mut awaited = Vec::new();
    for _ in 0..evaluations {
        let started = Instant::now();
        let (named, _, _) = world.gate_in_two_parts(pending);
        durations.push(started.elapsed());
        awaited = named;
    }
    (median(&mut durations), awaited)
}

/// The first measurement at one worker count: the three cases of the family
/// `gate-cost`, each with its two sizes of pending record. Returns the save
/// of the world after tick 102, which the second measurement reads, and the
/// median tick the family is set against.
fn evaluations_on_the_large_table(catalog: &Catalog, workers: usize, out: &mut GateForm, cells: &mut Vec<Evaluation>) -> (Vec<u8>, Duration) {
    let scale = catalog.scale;
    let Built { mut world, queue, tick, as_stated } = built(catalog.plan(workers, Rules::default(), Vec::new()));
    out.as_stated &= as_stated;
    out.form = world.gate_form();
    let outside = catalog.identifiers.gate;
    let sizes = [("small", catalog.small()), ("large", catalog.large())];
    let mut cell = |case: &'static str, size: &'static str, pending: &Record, evaluation: Duration, verdict_as_stated: bool| {
        cells.push(Evaluation { case, size, workers, pending: pending.commands.len() + pending.arrivals.len(), evaluation, tick, verdict_as_stated });
    };
    // Open: every required asset is answered, and the world is not gated.
    for (size, (commands, arrivals)) in &sizes {
        let pending = record(LAST_TIMED_TICK + 1, commands.clone(), arrivals.clone());
        let (evaluation, awaited) = evaluate(&world, &pending, scale.evaluations);
        cell("open", size, &pending, evaluation, awaited.is_empty());
    }
    // Whether that gate read its pending record: a record that closes a
    // gate which reads it leaves this one open where it does not.
    out.open_reads_the_pending_record |= !world.gate(&catalog.closes_a_gate_that_reads_it(LAST_TIMED_TICK + 1)).is_empty();
    // The world is saved after tick 102, for the second measurement.
    let save = world.save();
    // Closed: the record of tick 103 wants the one asset nothing answers.
    world.post(Catalog::want(outside, Policy::Required));
    out.as_stated &= world.step(1, Wait::No).expect("the world does not fail").ticks_ran == 1;
    for (size, (commands, arrivals)) in &sizes {
        let pending = record(LAST_TIMED_TICK + 2, commands.clone(), arrivals.clone());
        let (evaluation, awaited) = evaluate(&world, &pending, scale.evaluations);
        cell("closed", size, &pending, evaluation, awaited == [outside]);
    }
    // Opens: the pending record holds the arrival of that asset as well.
    let mut last = None;
    for (size, (commands, arrivals)) in &sizes {
        let mut arrivals = arrivals.clone();
        arrivals.push(catalog.contents.arrival(outside, catalog.of_the_asset_outside));
        let pending = record(LAST_TIMED_TICK + 2, commands.clone(), arrivals);
        let (evaluation, awaited) = evaluate(&world, &pending, scale.evaluations);
        cell("opens", size, &pending, evaluation, awaited.is_empty());
        last = Some(pending);
    }
    // The last record, built by hand for its evaluations, is the one the
    // world assembles from what is posted, and the world then runs.
    let pending = last.expect("two sizes");
    for command in &pending.commands {
        world.post(*command);
    }
    queue.post(&pending.arrivals);
    out.as_stated &= world.pending_record() == pending;
    out.as_stated &= world.step(1, Wait::No).expect("the world does not fail").ticks_ran == 1;
    (save, tick)
}

/// The second measurement: the save of the world after tick 102, read under
/// the content at hand, which is the saved world's, naming no content set,
/// with a scripted availability source. The restored world has run no tick
/// and awaits an answer for each of its available entries; every answer of
/// a pending record is marked and at the recorded hash.
pub fn read(save: &[u8], workers: usize, scale: Scale) -> Reader {
    let catalog = Catalog::new(scale);
    let identifiers = &catalog.identifiers;
    let links = Links::new(catalog.contents.residents.clone());
    let queue = links.queue.clone();
    let config = WorldConfig::new(scale.size, workers, Mode::Identifiers, identifiers.leaves[..POOL].to_vec());
    let request = Restore::ContentAtHand { dropped: &[], drop_in_silence: false, types_from: None };
    let mut world = World::restore(config, save, Vec::new(), links, request).expect("a restore under the content at hand is not refused");
    let held = world.restore_query();
    let entries = catalog.roots() * (1 + LEAVES_OF_A_ROOT);
    let mut reader = Reader { as_stated: held.len() == entries && world.tick() == LAST_TIMED_TICK, ..Reader::default() };
    reader.derivation = derivation(&world, scale.evaluations);

    let answer = |asset: &AssetId| {
        let (_, hash) = held[held.binary_search_by_key(asset, |(held, _)| *held).expect("an entry the restore awaits")];
        catalog.contents.arrival(*asset, hash).marked()
    };
    // What each case adds to the pending record of the case before it: the
    // four records are nested, so that one world reads them in turn.
    let half = catalog.roots() / 2;
    let closure = |roots: std::ops::Range<usize>| -> Vec<AssetId> {
        let leaves = roots.start * LEAVES_OF_A_ROOT..roots.end * LEAVES_OF_A_ROOT;
        identifiers.roots[roots].iter().chain(&identifiers.leaves[leaves]).copied().collect()
    };
    let added: [Vec<AssetId>; 4] = [Vec::new(), closure(0..1), closure(1..half), closure(half..catalog.roots())];
    let mut pending: Vec<Arrival> = Vec::new();
    for (case, added) in RESTORED_CASES.into_iter().zip(added) {
        let answers: Vec<Arrival> = added.iter().map(answer).collect();
        queue.post(&answers);
        pending.extend(answers);
        let stated = record(LAST_TIMED_TICK + 1, Vec::new(), pending.clone());
        reader.as_stated &= world.pending_record() == stated;
        let awaited_as_stated = entries - stated.arrivals.len();
        // The verdict alone, open or closed, with the count still awaited.
        let mut durations = Vec::with_capacity(scale.evaluations);
        let mut verdict = (false, 0);
        for _ in 0..scale.evaluations {
            let started = Instant::now();
            let given = world.gate_verdict(&stated);
            durations.push(started.elapsed());
            verdict = given;
        }
        let verdict_alone = median(&mut durations);
        // The whole answer, which also names the assets awaited.
        let (whole, named) = evaluate(&world, &stated, scale.evaluations);
        let (open, awaited) = verdict;
        let mut as_stated = open == (awaited_as_stated == 0) && awaited == awaited_as_stated && named.len() == awaited_as_stated;
        as_stated &= named.iter().all(|asset| !stated.arrivals.iter().any(|arrival| arrival.asset == *asset));
        // Where the gate is closed, a step without waiting runs no tick and
        // applies nothing: it is timed as the host makes it.
        let step = (!open).then(|| {
            let mut durations = Vec::with_capacity(scale.evaluations);
            for _ in 0..scale.evaluations {
                let started = Instant::now();
                let result = world.step(1, Wait::No).expect("the world does not fail");
                durations.push(started.elapsed());
                as_stated &= result.gated && result.ticks_ran == 0 && result.unanswered.len() == awaited_as_stated;
            }
            median(&mut durations)
        });
        reader.cases.push(Restored { case: case.to_string(), workers, pending: stated.arrivals.len(), awaited, open, verdict: verdict_alone, whole, step, as_stated });
    }
    // Every answer is pending: the world runs its first tick, and since
    // every answer is at the recorded hash it continues the saved run.
    let result = world.step(1, Wait::No).expect("the world does not fail");
    reader.as_stated &= result.ticks_ran == 1 && *world.history() == History::Continues;
    reader
}

/// The time it takes to derive, from a restored table, what the gate reads:
/// the entries a restore awaits and the set of the required assets the table
/// holds unanswered, with the two counts the fixed points read. It is made
/// on a copy of the restored table, as a restore makes it, and its median is
/// kept: it is no evaluation.
fn derivation(world: &World, evaluations: usize) -> Duration {
    let mut table = Table::decode(&world.typed_table_bytes(), Rules::default()).expect("the bytes of a table decode");
    let mut durations = Vec::with_capacity(evaluations);
    for _ in 0..evaluations {
        let started = Instant::now();
        table.awaiting = table.entries.iter().filter(|(_, entry)| matches!(entry.availability, Availability::Available(_))).map(|(id, _)| *id).collect();
        table.refresh();
        durations.push(started.elapsed());
    }
    median(&mut durations)
}

/// What the reader prints as a process of its own, a line per fact, and
/// what the harness reads back.
fn written(reader: &Reader) -> String {
    let mut out = format!("reader\t{}\t{}\n", reader.derivation.as_nanos(), u8::from(reader.as_stated));
    for case in &reader.cases {
        let step = case.step.map_or_else(|| "-".to_string(), |step| step.as_nanos().to_string());
        let _ = writeln!(
            out,
            "case\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{step}\t{}",
            case.case,
            case.workers,
            case.pending,
            case.awaited,
            u8::from(case.open),
            case.verdict.as_nanos(),
            case.whole.as_nanos(),
            u8::from(case.as_stated)
        );
    }
    out
}

fn parsed(text: &str) -> Result<Reader, String> {
    let nanos = |field: &str| field.parse::<u64>().map(Duration::from_nanos).map_err(|error| format!("`{field}` is no duration: {error}"));
    let count = |field: &str| field.parse::<usize>().map_err(|error| format!("`{field}` is no count: {error}"));
    let mut reader = None;
    let mut cases = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["reader", derivation, as_stated] => reader = Some((nanos(derivation)?, *as_stated == "1")),
            ["case", case, workers, pending, awaited, open, verdict, whole, step, as_stated] => cases.push(Restored {
                case: (*case).to_string(),
                workers: count(workers)?,
                pending: count(pending)?,
                awaited: count(awaited)?,
                open: *open == "1",
                verdict: nanos(verdict)?,
                whole: nanos(whole)?,
                step: if *step == "-" { None } else { Some(nanos(step)?) },
                as_stated: *as_stated == "1",
            }),
            _ => return Err(format!("a line the reader does not print: {line}")),
        }
    }
    let (derivation, as_stated) = reader.ok_or("the reader printed no line of its own")?;
    Ok(Reader { cases, derivation, as_stated })
}

/// The reader of a save as a process of its own:
/// `--gate-form-reader <save> <workers> <stated|small>`.
pub fn child(arguments: &[String]) -> String {
    let [save, workers, size] = arguments else { panic!("the reader is given a save, a worker count and a size") };
    let bytes = std::fs::read(save).unwrap_or_else(|error| panic!("the save {save} cannot be read: {error}"));
    let workers = workers.parse().expect("a worker count");
    written(&read(&bytes, workers, Scale::of(size == "small")))
}

/// Runs the reader as a process of its own and reads what it prints.
fn read_in_a_process(scratch: &Path, save: &[u8], workers: usize, small: bool) -> Reader {
    std::fs::create_dir_all(scratch).expect("a scratch directory is created");
    let path = scratch.join(format!("after-tick-{LAST_TIMED_TICK}-{workers}-workers.save"));
    std::fs::write(&path, save).expect("the save is written");
    let exe = std::env::current_exe().expect("the harness knows its own path");
    let size = if small { "small" } else { "stated" };
    let output = Process::new(exe).arg("--gate-form-reader").arg(&path).arg(workers.to_string()).arg(size).output().expect("the reader starts");
    let _ = std::fs::remove_file(&path);
    let printed = String::from_utf8_lossy(&output.stdout);
    let reader = if output.status.success() { parsed(&printed) } else { Err(format!("it ended with {}", output.status)) };
    reader.unwrap_or_else(|error| panic!("the reader of the save at {workers} workers gave nothing to read: {error}\n{}", String::from_utf8_lossy(&output.stderr)))
}

/// One run of the third measurement: the world of the family from tick 103
/// to tick 302 under the rules given, the records of those ticks scripted.
struct Run {
    median: Duration,
    /// The median duration of the input fixed point of the timed ticks.
    fixed_point: Duration,
    /// The median duration of the ticks 3 to 102 of the same world.
    tick: Duration,
    /// The changes the table followed from tick 123 to tick 302.
    changes: u64,
    /// The median time the table takes to bring its kept set up to date
    /// again for the assets the last record changes, and how many they are.
    following: Duration,
    followed: usize,
    state: Vec<u64>,
    as_stated: bool,
}

fn changing(catalog: &Catalog, workers: usize, rules: Rules) -> Run {
    let script: Vec<Record> = (0..CHANGING_TICKS as usize).map(|j| catalog.changing(j)).collect();
    let Built { mut world, tick, as_stated, .. } = built(catalog.plan(workers, rules, script.clone()));
    // The ticks before the timed ones, then the timed ones in one step, so
    // that each of them but the first is timed inside a step.
    let before = FIRST_TIMED_CHANGING_TICK - FIRST_CHANGING_TICK;
    let first = world.step(before, Wait::No).expect("the world does not fail");
    let followed = world.changes_followed();
    let timed = world.step(CHANGING_TICKS - before, Wait::No).expect("the world does not fail");
    let changes = world.changes_followed() - followed;
    let ran = first.ticks_ran == before && timed.ticks_ran == CHANGING_TICKS - before && world.tick() == LAST_CHANGING_TICK;
    let never_gated = world.gates().is_empty();
    let records_as_stated = world.log().iter().filter(|record| record.tick >= FIRST_CHANGING_TICK).eq(script.iter());
    let timed = FIRST_TIMED_CHANGING_TICK..=LAST_CHANGING_TICK;
    let mut fixed_points: Vec<Duration> = world.input_fixed_point_durations().iter().filter(|(tick, _)| timed.contains(tick)).map(|(_, duration)| *duration).collect();
    let ran = ran && fixed_points.len() == timed.count();
    // Beside the two medians: what the form does at a change, alone, on the
    // table as the last tick leaves it.
    let changed = catalog.changed_by(script.last().expect("two hundred records"));
    let mut followings: Vec<Duration> = (0..catalog.scale.evaluations).map(|_| world.follow_again(&changed)).collect();
    Run {
        median: if ran { median_tick(&world, FIRST_TIMED_CHANGING_TICK, LAST_CHANGING_TICK) } else { Duration::ZERO },
        fixed_point: if ran { median(&mut fixed_points) } else { Duration::ZERO },
        tick,
        changes,
        following: median(&mut followings),
        followed: changed.len(),
        state: world.state_hashes(),
        as_stated: as_stated && ran && never_gated && records_as_stated,
    }
}

/// The third measurement at one worker count: the same two hundred ticks
/// run twice, with the form and in the variant of the build without it.
fn cost_moved_to_the_changes(catalog: &Catalog, workers: usize) -> Changes {
    let with = changing(catalog, workers, Rules::default());
    let without = changing(catalog, workers, without_the_form());
    let timed = f64::from(LAST_CHANGING_TICK - FIRST_TIMED_CHANGING_TICK + 1);
    Changes {
        workers,
        with_the_form: with.median,
        without_the_form: without.median,
        fixed_point_with_the_form: with.fixed_point,
        fixed_point_without_the_form: without.fixed_point,
        tick: with.tick,
        changes_per_tick: with.changes as f64 / timed,
        following_with_the_form: with.following,
        following_without_the_form: without.following,
        followed: with.followed,
        state_hashes_equal: with.state == without.state,
        as_stated: with.as_stated && without.as_stated && with.changes == without.changes && with.changes > 0,
    }
}

/// Which measurements of the family a run makes: the two that are read
/// against the bound, the one that is reported with no bound, or all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Which {
    /// The first two measurements: one evaluation on the large table and on
    /// a restored table.
    pub evaluations: bool,
    /// The third: the cost the form moves to the changes.
    pub changes: bool,
}

impl Which {
    pub const ALL: Which = Which { evaluations: true, changes: true };
}

/// Measures the family: `repetitions` times, at each worker count. With a
/// scratch directory, the reader of the save runs as a process of its own,
/// as the sheet states it, and the run says on standard error where it is,
/// between two measurements; without one, the reader runs inside this
/// process and nothing is said, which is how a self-check runs it.
pub fn run(repetitions: usize, worker_counts: &[usize], scale: Scale, small: bool, which: Which, scratch: Option<&Path>) -> GateForm {
    let catalog = Catalog::new(scale);
    let mut out = GateForm { as_stated: true, ..GateForm::default() };
    for repetition in 1..=repetitions {
        let (mut evaluations, mut restored, mut derivations, mut changes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for &workers in worker_counts {
            let says = |what: &str| {
                if scratch.is_some() {
                    eprintln!("repetition {repetition} of {repetitions}, {workers} workers: {what}");
                }
            };
            if which.evaluations {
                says("one evaluation on the large table, then on the restored table");
                let (save, tick) = evaluations_on_the_large_table(&catalog, workers, &mut out, &mut evaluations);
                let reader = match scratch {
                    Some(scratch) => read_in_a_process(scratch, &save, workers, small),
                    None => read(&save, workers, scale),
                };
                out.as_stated &= reader.as_stated && reader.cases.len() == RESTORED_CASES.len();
                derivations.push((workers, reader.derivation));
                restored.extend(reader.cases.into_iter().map(|case| (case, tick)));
            }
            if which.changes {
                says("two hundred ticks of the changing world, with the form and then without it");
                let moved = cost_moved_to_the_changes(&catalog, workers);
                out.as_stated &= moved.as_stated && moved.state_hashes_equal;
                changes.push(moved);
            }
        }
        out.evaluations.push(evaluations);
        out.restored.push(restored);
        out.derivations.push(derivations);
        out.changes.push(changes);
    }
    out
}

/// Writes what the family measured: the facts, the summary and the full log.
pub fn report(cost: &GateForm, repetitions: usize, small: bool) -> (Json, String, String) {
    let (mut out, mut summary, mut log) = (Json::new(), String::new(), String::new());
    let (largest, largest_of) = cost.largest();
    // The claim is read on the first two measurements alone.
    let measured = cost.holds().is_some();
    let read = if cost.open_reads_the_pending_record { "it reads the pending record" } else { "it reads no pending record: the world is not gated, which its state alone tells" };
    out.object_start();
    head(&mut out, "gate-form", repetitions, small);
    out.string("the_form_that_evaluates_the_gate", cost.form);
    out.string("how_the_set_and_the_count_are_kept", HOW_THE_SET_AND_THE_COUNT_ARE_KEPT);
    out.bool("in_the_case_open_the_gate_reads_the_pending_record", cost.open_reads_the_pending_record);
    write_ratio(&mut out, "bound", BOUND);
    out.bool("the_first_two_measurements_were_run", measured);
    if measured {
        write_ratio(&mut out, "largest_ratio_of_one_evaluation_to_one_tick", largest);
        out.string("largest_ratio_is_of", &largest_of);
        out.bool("under_the_bound", largest < BOUND);
    }
    out.bool("every_verdict_as_stated", cost.every_verdict_as_stated());
    out.bool("every_table_record_and_world_as_stated", cost.as_stated);
    let _ = writeln!(log, "== the incremental form of the gate: {}", cost.form);
    let _ = writeln!(log, "   {HOW_THE_SET_AND_THE_COUNT_ARE_KEPT}");

    let _ = writeln!(log, "\n-- first measurement: one evaluation on the large table (in the case open, {read})");
    out.key("one_evaluation_on_the_large_table");
    out.array_start();
    for (repetition, cells) in cost.evaluations.iter().enumerate() {
        for cell in cells {
            let to_a_tick = ratio(cell.evaluation, cell.tick);
            out.object_start();
            out.u64("repetition", repetition as u64 + 1);
            out.string("case", &format!("gate-form/{}", cell.case));
            out.string("pending_record", cell.size);
            out.u64("pending_elements", cell.pending as u64);
            out.u64("workers", cell.workers as u64);
            out.u64("median_evaluation_ns", nanoseconds(cell.evaluation));
            out.u64("median_tick_ns", nanoseconds(cell.tick));
            write_ratio(&mut out, "ratio", to_a_tick);
            out.bool("verdict_as_stated", cell.verdict_as_stated);
            out.object_end();
            let _ = writeln!(
                log,
                "  repetition {} gate-form/{:<6} {:<5} ({:>4} pending elements) {} workers: evaluation {:>9} ns, tick {:>9} ns, ratio {:.6}, verdict as stated {}",
                repetition + 1,
                cell.case,
                cell.size,
                cell.pending,
                cell.workers,
                cell.evaluation.as_nanos(),
                cell.tick.as_nanos(),
                to_a_tick,
                cell.verdict_as_stated
            );
        }
    }
    out.array_end();

    let _ = writeln!(log, "\n-- second measurement: one evaluation on a restored table that awaits its answers, read in a process of its own");
    out.key("one_evaluation_on_a_restored_table");
    out.array_start();
    for (repetition, cells) in cost.restored.iter().enumerate() {
        for (cell, tick) in cells {
            let (of_the_verdict, of_the_whole) = (ratio(cell.verdict, *tick), ratio(cell.whole, *tick));
            out.object_start();
            out.u64("repetition", repetition as u64 + 1);
            out.string("case", &format!("gate-form/{}", cell.case));
            out.u64("workers", cell.workers as u64);
            out.u64("pending_elements", cell.pending as u64);
            out.u64("entries_still_awaited", cell.awaited as u64);
            out.string("verdict", if cell.open { "open" } else { "closed" });
            out.u64("median_verdict_alone_ns", nanoseconds(cell.verdict));
            out.u64("median_whole_answer_ns", nanoseconds(cell.whole));
            out.u64("median_tick_ns", nanoseconds(*tick));
            write_ratio(&mut out, "ratio_of_the_verdict_alone", of_the_verdict);
            write_ratio(&mut out, "ratio_of_the_whole_answer", of_the_whole);
            if let Some(step) = cell.step {
                out.u64("median_step_without_waiting_as_the_host_makes_it_ns", nanoseconds(step));
                write_ratio(&mut out, "ratio_of_the_step_as_the_host_makes_it", ratio(step, *tick));
            }
            out.bool("as_stated", cell.as_stated);
            out.object_end();
            let step = cell.step.map_or_else(|| "the step runs the tick".to_string(), |step| format!("the step as the host makes it {:>9} ns, ratio {:.6}", step.as_nanos(), ratio(step, *tick)));
            let _ = writeln!(
                log,
                "  repetition {} gate-form/{:<27} {} workers: {:>5} pending elements, {:>5} entries still awaited, {:<6}: the verdict alone {:>9} ns, ratio {:.6}; the whole answer {:>9} ns, ratio {:.6}; {step}; tick {:>9} ns; as stated {}",
                repetition + 1,
                cell.case,
                cell.workers,
                cell.pending,
                cell.awaited,
                if cell.open { "open" } else { "closed" },
                cell.verdict.as_nanos(),
                of_the_verdict,
                cell.whole.as_nanos(),
                of_the_whole,
                tick.as_nanos(),
                cell.as_stated
            );
        }
    }
    out.array_end();
    out.key("derivation_of_the_set_and_of_the_count_from_the_restored_table");
    out.array_start();
    for (repetition, derivations) in cost.derivations.iter().enumerate() {
        for (workers, derivation) in derivations {
            out.object_start();
            out.u64("repetition", repetition as u64 + 1);
            out.u64("workers", *workers as u64);
            out.u64("median_derivation_ns", nanoseconds(*derivation));
            out.object_end();
            let _ = writeln!(log, "  repetition {} reader at {workers} workers: the set and the count derived from the restored table in {} ns, which is no evaluation", repetition + 1, derivation.as_nanos());
        }
    }
    out.array_end();

    let _ = writeln!(log, "\n-- third measurement: the cost the form moves to every change of a want or of an availability (ticks {FIRST_TIMED_CHANGING_TICK} to {LAST_CHANGING_TICK})");
    out.key("cost_moved_to_the_changes");
    out.array_start();
    for (repetition, cells) in cost.changes.iter().enumerate() {
        for cell in cells {
            let share = cell.difference() as f64 / cell.tick.as_nanos() as f64;
            let per_change = cell.difference() as f64 / cell.changes_per_tick;
            let (following, following_a_tick) = (cell.following_per_change(), cell.following_per_change() * cell.changes_per_tick);
            out.object_start();
            out.u64("repetition", repetition as u64 + 1);
            out.u64("workers", cell.workers as u64);
            out.u64("median_tick_with_the_form_ns", nanoseconds(cell.with_the_form));
            out.u64("median_tick_without_the_form_ns", nanoseconds(cell.without_the_form));
            out.i64("difference_ns", cell.difference());
            out.u64("median_tick_of_the_ticks_3_to_102_ns", nanoseconds(cell.tick));
            write_ratio(&mut out, "difference_as_a_share_of_that_tick", share);
            out.decimal("changes_of_a_want_and_of_an_availability_per_tick", cell.changes_per_tick);
            out.decimal("difference_per_change_ns", per_change);
            out.bool("state_hashes_equal_at_every_tick", cell.state_hashes_equal);
            out.bool("as_stated", cell.as_stated);
            // Beside what the sheet asks for.
            out.u64("median_input_fixed_point_with_the_form_ns", nanoseconds(cell.fixed_point_with_the_form));
            out.u64("median_input_fixed_point_without_the_form_ns", nanoseconds(cell.fixed_point_without_the_form));
            out.i64("difference_of_the_input_fixed_points_ns", cell.difference_of_the_fixed_points());
            out.u64("assets_followed_again", cell.followed as u64);
            out.u64("median_following_again_with_the_form_ns", nanoseconds(cell.following_with_the_form));
            out.u64("median_following_again_without_the_form_ns", nanoseconds(cell.following_without_the_form));
            out.decimal("following_again_per_asset_ns", following);
            out.decimal("following_again_times_the_changes_of_a_tick_ns", following_a_tick);
            write_ratio(&mut out, "that_as_a_share_of_the_tick_of_the_ticks_3_to_102", following_a_tick / cell.tick.as_nanos() as f64);
            out.object_end();
            let _ = writeln!(
                log,
                "  repetition {} {} workers: tick with the form {:>9} ns, without it {:>9} ns, difference {:>8} ns, {:.4} of the tick of the ticks 3 to 102 ({} ns); \
                 {:.1} changes per tick, {:.2} ns per change; state hashes equal at every tick {}; as stated {}",
                repetition + 1,
                cell.workers,
                cell.with_the_form.as_nanos(),
                cell.without_the_form.as_nanos(),
                cell.difference(),
                share,
                cell.tick.as_nanos(),
                cell.changes_per_tick,
                per_change,
                cell.state_hashes_equal,
                cell.as_stated
            );
            let _ = writeln!(
                log,
                "    beside it: of those ticks the input fixed point alone {:>9} ns with the form and {:>9} ns without it, difference {:>8} ns; \
                 the kept set brought up to date again for the {} assets the last record changes in {} ns with the form and {} ns without it, {:.2} ns an asset, \
                 {:.0} ns for the changes of one tick, {:.4} of the tick of the ticks 3 to 102",
                cell.fixed_point_with_the_form.as_nanos(),
                cell.fixed_point_without_the_form.as_nanos(),
                cell.difference_of_the_fixed_points(),
                cell.followed,
                cell.following_with_the_form.as_nanos(),
                cell.following_without_the_form.as_nanos(),
                following,
                following_a_tick,
                following_a_tick / cell.tick.as_nanos() as f64
            );
        }
    }
    out.array_end();
    out.object_end();

    let _ = writeln!(summary, "the incremental form of the gate, {}", conditions(repetitions, small));
    if measured {
        let _ = writeln!(summary, "the gate is evaluated by {}", cost.form);
        let _ = writeln!(summary, "in the case open, {read}");
        let _ = writeln!(summary, "the largest ratio of one evaluation to one tick, over the first two measurements: {largest:.4} ({largest_of}); under a tenth of a tick: {}", largest < BOUND);
    } else {
        let _ = writeln!(summary, "the first two measurements were not run: the claim is read on them, and nothing is judged here");
    }
    // Per case of the restored table, over every repetition and worker
    // count: the pending elements and the largest of its two ratios, so that
    // a cost that follows the pending record is told from one that follows
    // the table.
    for case in RESTORED_CASES {
        let cells: Vec<&(Restored, Duration)> = cost.restored.iter().flatten().filter(|(cell, _)| cell.case == case).collect();
        let Some((first, _)) = cells.first() else { continue };
        let largest_of_the = |of: fn(&Restored) -> Duration| cells.iter().map(|(cell, tick)| ratio(of(cell), *tick)).fold(0.0, f64::max);
        let _ = writeln!(
            summary,
            "  gate-form/{case}: {} pending elements, {} entries still awaited; largest ratio of the verdict alone {:.4}, of the whole answer {:.4}",
            first.pending,
            first.awaited,
            largest_of_the(|cell| cell.verdict),
            largest_of_the(|cell| cell.whole)
        );
    }
    for size in ["small", "large"] {
        let Some(open) = cost.evaluations.iter().flatten().find(|cell| cell.size == size && cell.case == "open") else { continue };
        let largest = cost.evaluations.iter().flatten().filter(|cell| cell.size == size).map(|cell| ratio(cell.evaluation, cell.tick)).fold(0.0, f64::max);
        let _ = writeln!(summary, "  on the large table, the {size} pending record ({} elements; one more in the case opens): largest ratio {largest:.4}", open.pending);
    }
    for cells in &cost.changes {
        for cell in cells {
            let _ = writeln!(
                summary,
                "the cost moved to the changes at {} workers: the tick with the form {} ns, without it {} ns, difference {} ns ({:.4} of a tick with an empty record), {:.1} changes a tick, {:.2} ns a change; \
                 no bound is stated; state hashes equal {}; beside it, the kept set brought up to date again: {:.2} ns an asset, {:.4} of a tick with an empty record for the changes of one tick",
                cell.workers,
                cell.with_the_form.as_nanos(),
                cell.without_the_form.as_nanos(),
                cell.difference(),
                cell.difference() as f64 / cell.tick.as_nanos() as f64,
                cell.changes_per_tick,
                cell.difference() as f64 / cell.changes_per_tick,
                cell.state_hashes_equal,
                cell.following_per_change(),
                cell.following_per_change() * cell.changes_per_tick / cell.tick.as_nanos() as f64
            );
        }
    }
    let _ = writeln!(summary, "every verdict as stated: {}; every table, record and world as stated: {}", cost.every_verdict_as_stated(), cost.as_stated);
    (out, summary, log)
}

/// The mode `--gate-form [repetitions] [small] [evaluations|changes]`. With
/// `evaluations`, the first two measurements alone, which the claim is read
/// on; with `changes`, the third alone, which has no bound and takes most of
/// the time; with neither, all three.
pub fn mode(arguments: &[String]) {
    let only = |word: &str| arguments.iter().any(|argument| argument == word);
    let which = match (only("evaluations"), only("changes")) {
        (true, false) => Which { evaluations: true, changes: false },
        (false, true) => Which { evaluations: false, changes: true },
        _ => Which::ALL,
    };
    let others: Vec<String> = arguments.iter().filter(|argument| !["evaluations", "changes"].contains(&argument.as_str())).cloned().collect();
    let (repetitions, small) = arguments_or_exit("--gate-form", &others);
    let scratch = std::env::temp_dir().join(format!("asset-table-and-loader-gate-form-{}", std::process::id()));
    let cost = run(repetitions, &[1, 4, 8], Scale::of(small), small, which, Some(&scratch));
    let _ = std::fs::remove_dir_all(&scratch);
    let (facts, summary, log) = report(&cost, repetitions, small);
    let closing = match cost.holds() {
        Some(holds) => format!("holds: {holds}"),
        None => "holds: not judged, the first two measurements were not run".to_string(),
    };
    finish("gate-form", facts, &summary, &log, &closing);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first two measurements at their small size, the reader of the
    /// save inside the test process: every verdict is the one its case
    /// states, a world that is not gated reads no pending record, and the
    /// restored world awaits every entry and counts its pending answers.
    /// It judges no magnitude.
    #[test]
    fn the_evaluations_run_at_their_small_size() {
        let scale = Scale::SMALL;
        let cost = run(1, &[1, 4], scale, true, Which { evaluations: true, changes: false }, None);
        assert!(cost.as_stated && cost.every_verdict_as_stated());
        assert!(cost.holds().is_some(), "the claim is read where the evaluations were run");
        assert!(!cost.open_reads_the_pending_record, "a world that is not gated steps without reading the record of its next tick");
        assert!(cost.form.contains("kept set"), "the form that evaluated is the incremental one: {}", cost.form);
        assert_eq!(cost.evaluations[0].len(), 2 * 3 * 2, "three cases, two sizes of pending record, two worker counts");
        let entries = scale.roots * (1 + LEAVES_OF_A_ROOT);
        let stated = [(0, entries, false), (5, entries - 5, false), (entries / 2, entries / 2, false), (entries, 0, true)];
        for (cell, _) in &cost.restored[0] {
            let at = RESTORED_CASES.iter().position(|case| *case == cell.case).expect("a case of the sheet");
            assert_eq!((cell.pending, cell.awaited, cell.open), stated[at], "{}", cell.case);
            assert_eq!(cell.step.is_some(), !cell.open, "a step is timed where it runs no tick");
        }
        assert_eq!(cost.restored[0].len(), 2 * RESTORED_CASES.len());
        assert!(cost.changes[0].is_empty());
        let (facts, summary, log) = report(&cost, 1, true);
        let facts = facts.finish();
        for key in ["one_evaluation_on_the_large_table", "one_evaluation_on_a_restored_table", "cost_moved_to_the_changes", "largest_ratio_of_one_evaluation_to_one_tick"] {
            assert!(facts.contains(key), "{key}");
        }
        assert!(summary.contains("an indication and no measurement") && log.contains("second measurement"));
    }

    /// The third measurement at its small size: the world is never gated,
    /// every record is applied as stated, the table follows the same count
    /// of changes with the form and without it, and the two runs have the
    /// same state hash at every tick. Run alone it judges nothing: the claim
    /// is read on the first two measurements.
    #[test]
    fn the_cost_moved_to_the_changes_runs_at_its_small_size() {
        let cost = run(1, &[4], Scale::SMALL, true, Which { evaluations: false, changes: true }, None);
        assert!(cost.as_stated && cost.holds().is_none());
        let [cell] = &cost.changes[0][..] else { panic!("one figure per worker count") };
        assert!(cell.state_hashes_equal && cell.as_stated);
        assert!(cell.changes_per_tick > 0.0 && cell.with_the_form > Duration::ZERO && cell.without_the_form > Duration::ZERO);
        assert!(cell.followed > 0 && cell.fixed_point_with_the_form > Duration::ZERO && cell.tick > Duration::ZERO);
        let (_, summary, log) = report(&cost, 1, true);
        assert!(summary.contains("nothing is judged here") && log.contains("third measurement"));
    }

    /// What the reader prints as a process of its own is what the harness
    /// reads back, and a line it does not print is refused.
    #[test]
    fn what_the_reader_prints_is_read_back() {
        let case = |name: &str, open: bool, step: Option<u64>| Restored {
            case: name.to_string(),
            workers: 4,
            pending: 5,
            awaited: 9_995,
            open,
            verdict: Duration::from_nanos(120),
            whole: Duration::from_nanos(4_500),
            step: step.map(Duration::from_nanos),
            as_stated: true,
        };
        let reader = Reader { cases: vec![case("restore-one-closure-pending", false, Some(9_000)), case("restore-all-pending", true, None)], derivation: Duration::from_nanos(77), as_stated: true };
        assert_eq!(parsed(&written(&reader)), Ok(reader));
        assert!(parsed("case\tcut short\n").is_err());
        assert!(parsed("").is_err(), "a reader that printed nothing gave nothing to read");
    }

    /// The records of the third measurement are those the sheet states: at
    /// every boundary every required asset is answered, the wants and the
    /// availabilities change at every tick, and from the eleventh record on
    /// each record holds the same count of elements.
    #[test]
    fn the_records_of_the_changing_world_are_those_the_sheet_states() {
        let catalog = Catalog::new(Scale::SMALL);
        let unit = catalog.unit();
        let (first, later, back) = (catalog.changing(0), catalog.changing(25), catalog.changing(36));
        assert_eq!(first.tick, FIRST_CHANGING_TICK);
        assert_eq!(first.commands.len(), unit + unit / 5 + unit, "no release of an added asset and no want of a released root yet");
        assert_eq!(later.commands.len(), unit + unit + unit / 5 + unit / 5 + unit);
        assert_eq!((first.arrivals.len(), later.arrivals.len()), (2 * unit, 2 * unit));
        // The leaves a record replaces come back to their first content
        // thirty-six ticks later.
        let leaf = catalog.identifiers.leaves[4 * unit];
        let at = |record: &Record| record.arrivals.iter().find(|arrival| arrival.asset == leaf).map(|arrival| arrival.outcome);
        assert_eq!(at(&back), Some(prototype_core::table::Outcome::Hash(catalog.first.leaves[4 * unit])));
        assert_ne!(at(&first), at(&back));
        // An added asset is wanted with its arrival, released ten ticks
        // later and wanted again twenty ticks later.
        let added = catalog.identifiers.added[0];
        let names = |record: &Record, wanted: bool| {
            record.commands.iter().any(|command| match command {
                Command::Want { asset, .. } => wanted && *asset == added,
                Command::Release { asset, .. } => !wanted && *asset == added,
                _ => false,
            })
        };
        assert!(names(&first, true) && first.arrivals.iter().any(|arrival| arrival.asset == added));
        assert!(names(&catalog.changing(10), false) && names(&catalog.changing(20), true));
    }
}
