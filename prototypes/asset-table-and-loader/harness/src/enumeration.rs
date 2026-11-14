//! An enumeration of small records. Throwaway, never product code.
//!
//! Every record of at most four elements over two assets and two owners (a
//! want, a release, a despawn, an arrival at the same hash, at a new hash,
//! failed) is applied to the table alone from each of five starting tables
//! of the first asset (absent, wanted and unanswered, available under one
//! want, available with no want, failed), and followed by one empty tick.
//! Each is checked against invariants the design text states, and not
//! against an oracle. It reaches the orders that scenarios written by hand
//! do not.
//!
//! Every want states the asset type of the asset it names. The elements of
//! the amended rules add what the text gained since the first stage: a want
//! that states the second type, made by a third maker; a failure for the
//! reason that the manifest holds the asset under another type; and a sixth
//! starting table, failed for that reason.

use std::collections::BTreeMap;
use std::sync::Arc;

use prototype_core::table::{
    Applied, Arrival, AssetId, AssetType, Availability, Command, Event, Maker, Outcome, Owner, Part, Policy, REASON_ANOTHER_TYPE, REASON_INVALID_PART,
    Record, RepulsionTable, Resident, Residents, Rules, Table, part_bytes,
};
use reference_workload::fnv1a;

const X: AssetId = 0x1000;
const Y: AssetId = 0x2000;
const ENTITY: Owner = Owner::Entity { slot: 1, generation: 0 };
const MAKER: Owner = Owner::World(Maker::Source(3));
/// The maker of the wants that state the second type.
const OTHER_MAKER: Owner = Owner::World(Maker::Source(5));

/// Which elements the records are made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elements {
    /// The elements of the first stage, every want stating the first type.
    FirstStage,
    /// With the elements of the amended rules.
    Amended,
}

struct Parts(BTreeMap<u64, Arc<Resident>>);

impl Residents for Parts {
    fn get(&self, hash: u64) -> Option<Arc<Resident>> {
        self.0.get(&hash).cloned()
    }
}

fn parts() -> (Parts, [[u64; 2]; 2]) {
    let mut map = BTreeMap::new();
    let mut hashes = [[0; 2]; 2];
    for (asset, row) in hashes.iter_mut().enumerate() {
        for (version, hash) in row.iter_mut().enumerate() {
            let bytes = part_bytes(RepulsionTable::SPECIFICATION, &[], 8, (asset * 2 + version) as u32);
            *hash = fnv1a(&bytes);
            let part = Part::decode(&bytes).expect("a valid part");
            map.insert(*hash, Arc::new(Resident { bytes, part }));
        }
    }
    (Parts(map), hashes)
}

/// What the enumeration counted.
#[derive(Clone, Debug, Default)]
pub struct Enumeration {
    pub records: u64,
    pub applications: u64,
    /// An asset left the table although no element of the record ended a want of it.
    pub fell_while_wanted: u64,
    /// An entry stayed without a want past one tick boundary.
    pub stayed_without_a_want: u64,
    /// The canonical bytes depended on the order in which wants that were
    /// all applied were made.
    pub order_of_making: u64,
    /// A table read back from its canonical bytes was not byte-identical.
    pub round_trip: u64,
    /// An entry changed its type although it was not failed for its type.
    pub type_changed: u64,
    /// An entry that was failed, on which the record added a want and which
    /// no arrival of the record failed, is still failed.
    pub failed_under_a_new_want: u64,
    /// The events are not one per asset that became available, whose hash
    /// changed, or that failed.
    pub events: u64,
    /// A want left the domain for an asset the table held available, or for
    /// an asset no want names at the tick's end.
    pub left: u64,
    /// The applications in which an entry that had no want was wanted and
    /// released again inside one record: the case the text's rule on the
    /// fall states since the first stage.
    pub wanted_and_released_in_one_record: u64,
    /// The applications of a record made of wants alone in which a want is
    /// refused for its type in one order of making or in the other: the
    /// first want of an asset decides its type, so the order decides.
    pub order_decides_the_type: u64,
    pub first_violation: String,
}

impl Enumeration {
    pub fn violations(&self) -> u64 {
        self.fell_while_wanted
            + self.stayed_without_a_want
            + self.order_of_making
            + self.round_trip
            + self.type_changed
            + self.failed_under_a_new_want
            + self.events
            + self.left
    }

    pub fn holds(&self) -> bool {
        self.violations() == 0
    }
}

fn want(asset: AssetId, asset_type: AssetType, owner: Owner, policy: Policy) -> Command {
    Command::Want { asset, asset_type, owner, policy }
}

fn starting_tables(parts: &Parts, hashes: &[[u64; 2]; 2], elements: Elements) -> Vec<(&'static str, Table)> {
    let alive = |_: u32, _: u32| true;
    let wanted = || {
        let mut table = Table::with_rules(Rules::default());
        table.want(X, AssetType::Parameters, ENTITY, Policy::Required);
        table.refresh();
        table
    };
    let mut available = wanted();
    available.apply(&Record::arrivals(0, vec![Arrival::at(X, hashes[0][0])]), parts, &alive);
    let mut orphan = available.clone();
    orphan.apply(&Record::commands(0, vec![Command::Release { asset: X, owner: ENTITY }]), parts, &alive);
    let failed_for = |code: u32| {
        let mut failed = wanted();
        failed.apply(&Record::arrivals(0, vec![Arrival::failed(X, code)]), parts, &alive);
        failed
    };
    let mut tables = vec![
        ("absent", Table::with_rules(Rules::default())),
        ("wanted and unanswered", wanted()),
        ("available under one want", available),
        ("available with no want", orphan),
        ("failed", failed_for(REASON_INVALID_PART)),
    ];
    if elements == Elements::Amended {
        tables.push(("failed for its type", failed_for(REASON_ANOTHER_TYPE)));
    }
    tables
}

/// Whether an element of the record ends a want of the asset.
fn ends_a_want(record: &Record, asset: AssetId) -> bool {
    record.commands.iter().any(|command| match command {
        Command::Release { asset: released, .. } => *released == asset,
        Command::Despawn { .. } => true,
        _ => false,
    })
}

fn availability(table: &Table, asset: AssetId) -> Option<Availability> {
    table.entries.get(&asset).map(|entry| entry.availability)
}

/// Whether the events of a record are one per asset that became available,
/// whose hash changed, or that failed, each true of the table it left.
fn events_hold(before: &Table, after: &Table, applied: &Applied) -> bool {
    let of = |event: &Event| match event {
        Event::Arrived { asset, .. } | Event::HashChanged { asset, .. } | Event::Failed { asset, .. } => Some(*asset),
        Event::NewHistory(_) => None,
    };
    for (index, event) in applied.events.iter().enumerate() {
        let Some(asset) = of(event) else { return false };
        if applied.events[..index].iter().any(|other| of(other) == Some(asset)) {
            return false;
        }
        let (held, now) = (availability(before, asset), availability(after, asset));
        let stated = match event {
            Event::Arrived { hash, .. } => held != Some(Availability::Available(*hash)) && !matches!(held, Some(Availability::Available(_))) && now == Some(Availability::Available(*hash)),
            Event::HashChanged { from, to, .. } => from != to && held == Some(Availability::Available(*from)) && now == Some(Availability::Available(*to)),
            Event::Failed { code, .. } => now == Some(Availability::Failed(*code)),
            Event::NewHistory(_) => false,
        };
        if !stated {
            return false;
        }
    }
    // An asset that became available, changed its hash or failed has its event.
    [X, Y].into_iter().all(|asset| {
        let (held, now) = (availability(before, asset), availability(after, asset));
        let changed = matches!(now, Some(Availability::Available(_) | Availability::Failed(_))) && held != now;
        !changed || applied.events.iter().any(|event| of(event) == Some(asset))
    })
}

pub fn run(elements: Elements) -> Enumeration {
    let (parts, hashes) = parts();
    let amended = elements == Elements::Amended;
    let mut commands = Vec::new();
    for asset in [X, Y] {
        commands.push(want(asset, AssetType::Parameters, ENTITY, Policy::Required));
        commands.push(want(asset, AssetType::Parameters, MAKER, Policy::Optional));
        commands.push(Command::Release { asset, owner: ENTITY });
        commands.push(Command::Release { asset, owner: MAKER });
        if amended {
            commands.push(want(asset, AssetType::Second, OTHER_MAKER, Policy::Optional));
        }
    }
    commands.push(Command::Despawn { slot: 1, generation: 0 });
    let arrivals_of = |asset: AssetId, index: usize| {
        let mut arrivals = vec![None, Some(Arrival::at(asset, hashes[index][0])), Some(Arrival::at(asset, hashes[index][1])), Some(Arrival::failed(asset, REASON_INVALID_PART))];
        if amended {
            arrivals.push(Some(Arrival::failed(asset, REASON_ANOTHER_TYPE)));
        }
        arrivals
    };
    let mut batches: Vec<Vec<Arrival>> = Vec::new();
    for x in arrivals_of(X, 0) {
        for y in arrivals_of(Y, 1) {
            batches.push(x.into_iter().chain(y).collect());
        }
    }
    let mut sequences: Vec<Vec<Command>> = vec![Vec::new()];
    let mut last: Vec<Vec<Command>> = vec![Vec::new()];
    for _ in 0..4 {
        last = last.iter().flat_map(|sequence| commands.iter().map(move |command| sequence.iter().copied().chain([*command]).collect())).collect();
        sequences.extend(last.iter().cloned());
    }
    let starts = starting_tables(&parts, &hashes, elements);
    let alive = |_: u32, _: u32| true;
    let mut result = Enumeration::default();
    for sequence in &sequences {
        for batch in batches.iter().filter(|batch| sequence.len() + batch.len() <= 4) {
            result.records += 1;
            let record = Record { tick: 1, commands: sequence.clone(), arrivals: batch.clone() };
            for (name, start) in &starts {
                result.applications += 1;
                let mut table = start.clone();
                let before = table.clone();
                let applied = table.apply(&record, &parts, &alive);
                let after_first = table.clone();
                table.apply(&Record { tick: 2, ..Record::default() }, &parts, &alive);
                let mut violate = |counter: &mut u64, what: &str| {
                    *counter += 1;
                    if result.first_violation.is_empty() {
                        result.first_violation = format!("{what}: from the table \"{name}\", the record {record:?}");
                    }
                };
                // No asset falls while a want names it.
                for asset in [X, Y] {
                    let wanted_before = before.entries.get(&asset).is_some_and(|entry| !entry.wants.is_empty());
                    if wanted_before && !ends_a_want(&record, asset) && (!after_first.entries.contains_key(&asset) || !table.entries.contains_key(&asset)) {
                        violate(&mut result.fell_while_wanted, "an asset fell while a want named it");
                    }
                }
                // No entry stays without a want past one tick boundary.
                let stays = after_first.entries.iter().any(|(id, entry)| entry.wants.is_empty() && table.entries.contains_key(id));
                if stays || table.entries.values().any(|entry| entry.wants.is_empty()) {
                    violate(&mut result.stayed_without_a_want, "an entry stayed without a want past one tick boundary");
                }
                // The canonical bytes do not depend on the order of making,
                // among wants that are all applied: where two wants state two
                // types, the first made decides, which is the rule.
                if sequence.len() > 1 && sequence.iter().all(|command| matches!(command, Command::Want { .. })) {
                    let mut reversed = start.clone();
                    let commands: Vec<Command> = sequence.iter().rev().copied().collect();
                    let other = reversed.apply(&Record { tick: 1, commands, arrivals: batch.clone() }, &parts, &alive);
                    if !applied.refusals.is_empty() || !other.refusals.is_empty() {
                        result.order_decides_the_type += 1;
                    } else if reversed.canonical_bytes() != after_first.canonical_bytes() {
                        violate(&mut result.order_of_making, "the canonical bytes depend on the order in which wants were made");
                    }
                }
                // A table saved and restored is byte-identical.
                for saved in [&after_first, &table] {
                    let bytes = saved.canonical_bytes();
                    if Table::decode(&bytes, Rules::default()).map(|read| read.canonical_bytes()) != Ok(bytes) {
                        violate(&mut result.round_trip, "a table read back from its canonical bytes is not byte-identical");
                    }
                }
                for asset in [X, Y] {
                    let (Some(held), Some(now)) = (before.entries.get(&asset), after_first.entries.get(&asset)) else { continue };
                    // An entry holds the type its first want stated, but
                    // where it was failed for its type.
                    if held.asset_type != now.asset_type && held.availability != Availability::Failed(REASON_ANOTHER_TYPE) {
                        violate(&mut result.type_changed, "an entry changed its type although it was not failed for its type");
                    }
                    // A want added on a failed entry returns it to unavailable.
                    let added = now.wants.iter().any(|(owner, _)| !held.wants.iter().any(|(other, _)| other == owner));
                    let failed_again = record.arrivals.iter().any(|arrival| arrival.asset == asset && matches!(arrival.outcome, Outcome::Failure(_)));
                    if matches!(held.availability, Availability::Failed(_)) && added && !failed_again && matches!(now.availability, Availability::Failed(_)) {
                        violate(&mut result.failed_under_a_new_want, "an entry stayed failed under a want the record added");
                    }
                }
                // One event per asset that became available, whose hash
                // changed, or that failed; none for an arrival that leaves
                // an entry as it was.
                if !events_hold(&before, &after_first, &applied) {
                    violate(&mut result.events, "the events are not one per asset that became available, changed or failed");
                }
                // A want leaves the domain only where the asset it names is
                // unanswered or failed, and stands at the tick's end.
                for (asset, asset_type) in &applied.left {
                    let held_available = matches!(availability(&before, *asset), Some(Availability::Available(_)));
                    let stands = after_first.entries.get(asset).is_some_and(|entry| !entry.wants.is_empty() && entry.asset_type == *asset_type);
                    if held_available || !stands {
                        violate(&mut result.left, "a want left the domain for an asset in force, or for an asset no want names");
                    }
                }
                // The case of the fall the text states since the first
                // stage: an entry with no want, wanted and released again
                // inside the record.
                for asset in [X, Y] {
                    let orphan_before = before.entries.get(&asset).is_some_and(|entry| entry.wants.is_empty());
                    let orphan_after = after_first.entries.get(&asset).is_some_and(|entry| entry.wants.is_empty());
                    if orphan_before && orphan_after && after_first.entries[&asset].availability != Availability::Unavailable {
                        result.wanted_and_released_in_one_record += 1;
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_small_record_keeps_the_invariants() {
        let result = run(Elements::FirstStage);
        assert!(result.holds(), "{}", result.first_violation);
        assert_eq!((result.records, result.applications), (13_120, 65_600));
        assert_eq!(result.order_decides_the_type, 0, "every want states the first type");
    }

    #[test]
    fn every_small_record_of_the_amended_elements_keeps_the_invariants() {
        let result = run(Elements::Amended);
        assert!(result.holds(), "{}", result.first_violation);
        assert_eq!((result.records, result.applications), (29_945, 179_670));
        assert!(result.order_decides_the_type > 0, "two wants of two types: the first made decides");
    }
}
