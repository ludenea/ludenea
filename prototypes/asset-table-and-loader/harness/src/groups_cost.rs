//! The cost of deriving the groups of a rejection over a world's budget: a
//! magnitude, with no oracle and no control. Throwaway, never product code.
//!
//! The family `groups-cost` of the sheet of the stand-in stage. A world of
//! the reference workload holds the large table of the family `gate-cost`,
//! or its first tenth, every content of 1 000 bytes, under a budget that no
//! record reaches: every group is accepted, and what is timed is how the
//! groups of one record are derived and decided, whatever the decision. The
//! same record, of one arrival or of a thousand, is derived on the two
//! tables: a cost that follows the record leaves the derivation on the large
//! table near the one on the small table, and a cost that follows the table
//! brings it near ten times that, the ratio of the two tables.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use prototype_core::table::AssetType::Parameters;
use prototype_core::table::{Record, Rules};
use prototype_core::world::Wait;
use reference_workload::Json;

use crate::large_table::{Built, Contents, FirstContents, Identifiers, LAST_TIMED_TICK, LEAVES_OF_A_ROOT, Length, POOL, Plan, Scale, built, record};
use crate::magnitude::{arguments_or_exit, conditions, finish, head, median, nanoseconds, ratio, write_ratio};

/// The world's budget for its asset data, which no record of the family reaches.
const BUDGET: u64 = 100_000_000;
/// The size of every first content, and of every second one, in bytes.
const FIRST_SIZE: usize = 1_000;
const SECOND_SIZE: usize = 1_100;
/// The tick whose record is derived, assembled after tick 102.
const TICK_OF_THE_RECORD: u32 = LAST_TIMED_TICK + 1;
/// The small table and the large record are each a tenth of the large table.
const TENTH: usize = 10;
const TABLES: [&str; 2] = ["large", "small"];
const RECORDS: [&str; 2] = ["one", "thousand"];

/// The contents of the family and its two records.
struct Catalog {
    identifiers: Identifiers,
    contents: Contents,
    first: FirstContents,
    /// The second content of each leaf of the small table: 1 100 bytes, and
    /// it lists nothing.
    second_of_the_leaves: Vec<u64>,
    /// The second content of each root of the small table: 1 100 bytes, its
    /// four leaves and one leaf more, the first leaf of the next root and,
    /// for the last root, the first leaf of the table.
    second_of_the_roots: Vec<u64>,
}

impl Catalog {
    fn new(scale: Scale) -> Catalog {
        let identifiers = Identifiers::drawn(0);
        let mut contents = Contents::default();
        let first = FirstContents::of(&mut contents, &identifiers, scale.roots, Length::Part(FIRST_SIZE));
        let roots = scale.roots / TENTH;
        let second_of_the_leaves =
            (0..roots * LEAVES_OF_A_ROOT).map(|leaf| contents.resident(24.5 + (leaf % POOL) as f32, &[], Length::Part(SECOND_SIZE), 400_000 + leaf as u32)).collect();
        let second_of_the_roots = (0..roots)
            .map(|root| {
                let one_more = if root + 1 < roots { (root + 1) * LEAVES_OF_A_ROOT } else { 0 };
                let mut listed = identifiers.listed(root, 0);
                listed.push((identifiers.leaves[one_more], Parameters));
                contents.resident(42.0, &listed, Length::Part(SECOND_SIZE), 200_000 + root as u32)
            })
            .collect();
        Catalog { identifiers, contents, first, second_of_the_leaves, second_of_the_roots }
    }

    /// The record `one`: the arrival of the first leaf at its second content.
    fn one(&self) -> Record {
        record(TICK_OF_THE_RECORD, Vec::new(), vec![self.contents.arrival(self.identifiers.leaves[0], self.second_of_the_leaves[0])])
    }

    /// The record `thousand`: the arrival of every root and of every leaf of
    /// the small table, each at its second content.
    fn thousand(&self) -> Record {
        let of_roots = self.identifiers.roots.iter().zip(&self.second_of_the_roots);
        let of_leaves = self.identifiers.leaves.iter().zip(&self.second_of_the_leaves);
        record(TICK_OF_THE_RECORD, Vec::new(), of_roots.chain(of_leaves).map(|(asset, hash)| self.contents.arrival(*asset, *hash)).collect())
    }

    fn record_named(&self, name: &str) -> Record {
        if name == "one" { self.one() } else { self.thousand() }
    }
}

/// One figure: a table, a record and a worker count.
#[derive(Clone, Debug)]
pub struct Cell {
    pub table: &'static str,
    pub record: &'static str,
    pub workers: usize,
    /// The entries the table holds, and the arrivals the record holds.
    pub entries: usize,
    pub arrivals: usize,
    /// The median of the derivations and decisions of the groups of the
    /// record, which none of them applies.
    pub derivation: Duration,
    /// The median duration of one tick of the same world.
    pub tick: Duration,
    /// The roots whose closure one derivation read.
    pub roots_read: usize,
    /// The arrivals one derivation rejected: none, the budget being out of reach.
    pub rejected: usize,
    /// The duration of the input fixed point of tick 103, which applies the
    /// record, under the budget and in a twin world created with no budget.
    pub fixed_point_under_the_budget: Duration,
    pub fixed_point_without_a_budget: Duration,
    /// Whether the two worlds held the table the family states, applied the
    /// record as stated at tick 103 with no rejection, and left one state.
    pub as_stated: bool,
}

/// What the family measured.
pub struct GroupsCost {
    /// Per repetition, the figures of each table, record and worker count.
    pub repetitions: Vec<Vec<Cell>>,
    /// How the build derives the groups, in words.
    pub form: &'static str,
}

impl GroupsCost {
    pub fn every_world_as_stated(&self) -> bool {
        self.repetitions.iter().flatten().all(|cell| cell.as_stated && cell.rejected == 0)
    }
}

/// The figure the claim is read on, for one record and one worker count of
/// one repetition: the median derivation on the large table over the median
/// derivation on the small one.
pub fn large_over_small(cells: &[Cell], record: &str, workers: usize) -> f64 {
    let of = |table: &str| cells.iter().find(|cell| cell.table == table && cell.record == record && cell.workers == workers).expect("a figure per table").derivation;
    ratio(of("large"), of("small"))
}

/// What the input fixed point of the record's tick gave a world.
struct Applied {
    fixed_point: Duration,
    as_stated: bool,
    state: Option<u64>,
}

/// Posts the record and steps the world one tick: the input fixed point of
/// tick 103 applies it.
fn apply(built: &mut Built, stated: &Record) -> Applied {
    built.queue.post(&stated.arrivals);
    let result = built.world.step(1, Wait::No).expect("the world does not fail");
    let fixed_point = built.world.input_fixed_point_durations().iter().rev().find(|(tick, _)| *tick == stated.tick).map(|(_, duration)| *duration);
    let assembled = built.world.log().last() == Some(stated);
    let as_stated = built.as_stated && result.ticks_ran == 1 && result.rejected.is_empty() && assembled && fixed_point.is_some();
    Applied { fixed_point: fixed_point.unwrap_or_default(), as_stated, state: built.world.state_hashes().last().copied() }
}

/// One figure: a world under the budget, on which the groups of the record
/// are derived and decided a hundred times and the record then applied, and
/// a twin world created with no budget, on which it is applied too.
fn cell(catalog: &Catalog, scale: Scale, table: &'static str, name: &'static str, workers: usize) -> (Cell, &'static str) {
    let roots = if table == "large" { scale.roots } else { scale.roots / TENTH };
    let plan = |budget: Option<u64>| Plan {
        scale,
        roots,
        workers,
        rules: Rules::default(),
        budget,
        script: Vec::new(),
        identifiers: &catalog.identifiers,
        contents: &catalog.contents,
        first: &catalog.first,
    };
    let stated = catalog.record_named(name);
    let mut under = built(plan(Some(BUDGET)));
    let form = under.world.groups_form();
    // The entries with no want are found before the commands of a record
    // are applied: what is timed starts once they are.
    let without_a_want = under.world.entries_without_a_want();
    let mut durations = Vec::with_capacity(scale.evaluations);
    let (mut rejected, mut roots_read) = (0, 0);
    for _ in 0..scale.evaluations {
        let started = Instant::now();
        let decided = under.world.groups_decided(&stated, &without_a_want);
        durations.push(started.elapsed());
        (rejected, roots_read) = decided;
    }
    let derivation = median(&mut durations);
    let entries = under.world.entries().len();
    let tick = under.tick;
    let with = apply(&mut under, &stated);
    let mut twin = built(plan(None));
    let without = apply(&mut twin, &stated);
    let cell = Cell {
        table,
        record: name,
        workers,
        entries,
        arrivals: stated.arrivals.len(),
        derivation,
        tick,
        roots_read,
        rejected,
        fixed_point_under_the_budget: with.fixed_point,
        fixed_point_without_a_budget: without.fixed_point,
        as_stated: with.as_stated && without.as_stated && with.state == without.state,
    };
    (cell, form)
}

/// Measures the family: `repetitions` times, at each worker count.
pub fn run(repetitions: usize, worker_counts: &[usize], scale: Scale) -> GroupsCost {
    let catalog = Catalog::new(scale);
    let mut out = GroupsCost { repetitions: Vec::new(), form: "" };
    for _ in 0..repetitions {
        let mut cells = Vec::new();
        for &workers in worker_counts {
            for table in TABLES {
                for name in RECORDS {
                    let (cell, form) = cell(&catalog, scale, table, name, workers);
                    out.form = form;
                    cells.push(cell);
                }
            }
        }
        out.repetitions.push(cells);
    }
    out
}

/// Writes what the family measured: the facts, the summary and the full log.
pub fn report(cost: &GroupsCost, worker_counts: &[usize], repetitions: usize, small: bool) -> (Json, String, String) {
    let (mut out, mut summary, mut log) = (Json::new(), String::new(), String::new());
    out.object_start();
    head(&mut out, "groups-cost", repetitions, small);
    out.string("how_the_groups_are_derived", cost.form);
    out.u64("budget_bytes", BUDGET);
    out.bool("every_world_as_stated_and_nothing_rejected", cost.every_world_as_stated());
    let _ = writeln!(log, "== the cost of deriving the groups of a rejection over a budget: {}", cost.form);
    out.key("figures");
    out.array_start();
    for (repetition, cells) in cost.repetitions.iter().enumerate() {
        for cell in cells {
            let to_a_tick = ratio(cell.derivation, cell.tick);
            out.object_start();
            out.u64("repetition", repetition as u64 + 1);
            out.string("table", cell.table);
            out.string("record", cell.record);
            out.u64("workers", cell.workers as u64);
            out.u64("entries_of_the_table", cell.entries as u64);
            out.u64("arrivals_of_the_record", cell.arrivals as u64);
            out.u64("median_derivation_ns", nanoseconds(cell.derivation));
            out.u64("median_tick_ns", nanoseconds(cell.tick));
            write_ratio(&mut out, "ratio", to_a_tick);
            out.u64("roots_whose_closure_one_derivation_read", cell.roots_read as u64);
            out.u64("arrivals_rejected", cell.rejected as u64);
            out.u64("input_fixed_point_of_the_record_under_the_budget_ns", nanoseconds(cell.fixed_point_under_the_budget));
            out.u64("input_fixed_point_of_the_record_without_a_budget_ns", nanoseconds(cell.fixed_point_without_a_budget));
            out.bool("as_stated", cell.as_stated);
            out.object_end();
            let _ = writeln!(
                log,
                "  repetition {} table {:<5} ({:>5} entries) record {:<8} ({:>4} arrivals) {} workers: derivation {:>9} ns, tick {:>9} ns, ratio {:.4}, \
                 roots whose closure was read {:>4}, rejected {}, input fixed point of tick {TICK_OF_THE_RECORD} under the budget {:>9} ns and without a budget {:>9} ns, as stated {}",
                repetition + 1,
                cell.table,
                cell.entries,
                cell.record,
                cell.arrivals,
                cell.workers,
                cell.derivation.as_nanos(),
                cell.tick.as_nanos(),
                to_a_tick,
                cell.roots_read,
                cell.rejected,
                cell.fixed_point_under_the_budget.as_nanos(),
                cell.fixed_point_without_a_budget.as_nanos(),
                cell.as_stated
            );
        }
    }
    out.array_end();
    // The figure the claim is read on: per record and per worker count, the
    // derivation on the large table over the derivation on the small one.
    out.key("derivation_on_the_large_table_over_the_small_one");
    out.array_start();
    let _ = writeln!(summary, "the cost of deriving the groups of a rejection over a budget, {}", conditions(repetitions, small));
    let _ = writeln!(summary, "the build derives the groups {}", cost.form);
    let _ = writeln!(summary, "the derivation on the large table over the derivation on the small one, the record being the same (near one: the cost follows the record; near ten: it follows the table):");
    for (repetition, cells) in cost.repetitions.iter().enumerate() {
        let mut figures = Vec::new();
        for name in RECORDS {
            for &workers in worker_counts {
                let figure = large_over_small(cells, name, workers);
                out.object_start();
                out.u64("repetition", repetition as u64 + 1);
                out.string("record", name);
                out.u64("workers", workers as u64);
                write_ratio(&mut out, "large_over_small", figure);
                out.object_end();
                figures.push(format!("{name} at {workers} workers {figure:.2}"));
            }
        }
        let _ = writeln!(summary, "  repetition {}: {}", repetition + 1, figures.join(", "));
    }
    out.array_end();
    out.object_end();
    // The roots read and the largest share of a tick, over every figure.
    let read = |table: &str, name: &str| cost.repetitions.iter().flatten().find(|cell| cell.table == table && cell.record == name).map_or(0, |cell| cell.roots_read);
    let _ = writeln!(
        summary,
        "roots whose closure one derivation read: large table {} for the record of one arrival and {} for the record of a thousand; small table {} and {}",
        read("large", "one"),
        read("large", "thousand"),
        read("small", "one"),
        read("small", "thousand")
    );
    let largest = cost.repetitions.iter().flatten().max_by(|a, b| ratio(a.derivation, a.tick).total_cmp(&ratio(b.derivation, b.tick)));
    if let Some(cell) = largest {
        let _ = writeln!(
            summary,
            "the largest derivation beside a tick: {:.3} of a tick ({} ns, table {}, record {}, {} workers); no bound is stated",
            ratio(cell.derivation, cell.tick),
            cell.derivation.as_nanos(),
            cell.table,
            cell.record,
            cell.workers
        );
    }
    let _ = writeln!(summary, "every world as stated, every group accepted: {}", cost.every_world_as_stated());
    (out, summary, log)
}

/// The mode `--groups-cost [repetitions] [small]`.
pub fn mode(arguments: &[String]) {
    let (repetitions, small) = arguments_or_exit("--groups-cost", arguments);
    let workers = [1, 4, 8];
    let cost = run(repetitions, &workers, Scale::of(small));
    let (facts, summary, log) = report(&cost, &workers, repetitions, small);
    finish("groups-cost", facts, &summary, &log, &format!("every world as stated: {}", cost.every_world_as_stated()));
}

#[cfg(test)]
mod tests {
    use prototype_core::table::Residents as _;

    use super::*;

    /// The mode at its small size: the two tables and the two records are
    /// those the sheet states, a tenth as large, nothing is rejected, the
    /// record applied under the budget leaves the state its twin without a
    /// budget has, and the report holds a figure per table, record and
    /// worker count. It judges no magnitude.
    #[test]
    fn the_mode_runs_at_its_small_size() {
        let scale = Scale::SMALL;
        let cost = run(1, &[1, 4], scale);
        assert!(cost.every_world_as_stated());
        let cells = &cost.repetitions[0];
        assert_eq!(cells.len(), 2 * TABLES.len() * RECORDS.len());
        for cell in cells {
            let roots = if cell.table == "large" { scale.roots } else { scale.roots / TENTH };
            assert_eq!(cell.entries, roots * (1 + LEAVES_OF_A_ROOT), "{} holds its roots and their leaves", cell.table);
            let arrivals = if cell.record == "one" { 1 } else { scale.roots / TENTH * (1 + LEAVES_OF_A_ROOT) };
            assert_eq!(cell.arrivals, arrivals, "the record {} is the same on the two tables", cell.record);
            assert!(cell.roots_read <= roots, "a derivation reads the closure of no more roots than the table holds");
            assert!(cell.derivation > Duration::ZERO && cell.tick > Duration::ZERO);
            assert!(cell.fixed_point_under_the_budget > Duration::ZERO && cell.fixed_point_without_a_budget > Duration::ZERO);
        }
        assert!(!cost.form.is_empty());
        assert!(large_over_small(cells, "one", 1) > 0.0);
        let (facts, summary, log) = report(&cost, &[1, 4], 1, true);
        let facts = facts.finish();
        assert!(facts.contains("\"mode\":\"groups-cost\"") && facts.contains("derivation_on_the_large_table_over_the_small_one"));
        assert!(summary.contains("an indication and no measurement"));
        assert_eq!(log.lines().count(), 1 + cells.len());
    }

    /// The two records are the same, byte for byte, whatever the table: the
    /// last root of the record lists the first leaf of the table as its one
    /// leaf more.
    #[test]
    fn the_records_are_those_the_sheet_states() {
        let catalog = Catalog::new(Scale::SMALL);
        let roots = Scale::SMALL.roots / TENTH;
        let one = catalog.one();
        assert_eq!((one.tick, one.commands.len(), one.arrivals.len()), (TICK_OF_THE_RECORD, 0, 1));
        assert_eq!((one.arrivals[0].asset, one.arrivals[0].size), (catalog.identifiers.leaves[0], SECOND_SIZE as u64));
        let thousand = catalog.thousand();
        assert_eq!(thousand.arrivals.len(), roots * (1 + LEAVES_OF_A_ROOT));
        assert!(thousand.arrivals.iter().all(|arrival| arrival.size == SECOND_SIZE as u64));
        assert!(thousand.arrivals.windows(2).all(|pair| pair[0].asset < pair[1].asset), "the arrivals stand in identifier order");
        let part_of = |root: usize| catalog.contents.residents.get(catalog.second_of_the_roots[root]).expect("a resident part");
        let last = |root: usize| part_of(root).part.dependencies.last().map(|(asset, _)| *asset);
        assert_eq!(part_of(0).part.dependencies.len(), LEAVES_OF_A_ROOT + 1);
        assert_eq!(last(0), Some(catalog.identifiers.leaves[LEAVES_OF_A_ROOT]), "the first leaf of the next root");
        assert_eq!(last(roots - 1), Some(catalog.identifiers.leaves[0]), "for the last root, the first leaf of the table");
    }
}
