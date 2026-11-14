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

use std::collections::BTreeMap;
use std::sync::Arc;

use prototype_core::table::{
    Arrival, AssetId, Availability, Command, Maker, Owner, Part, Policy, REASON_INVALID_PART, Record, RepulsionTable, Resident, Residents, Rules, Table,
    part_bytes,
};
use reference_workload::fnv1a;

const X: AssetId = 0x1000;
const Y: AssetId = 0x2000;
const ENTITY: Owner = Owner::Entity { slot: 1, generation: 0 };
const MAKER: Owner = Owner::World(Maker::Source(3));

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
    /// The canonical bytes depended on the order in which wants were made.
    pub order_of_making: u64,
    /// A table read back from its canonical bytes was not byte-identical.
    pub round_trip: u64,
    /// The applications in which an entry that had no want was wanted and
    /// released again inside one record: the case the text's rule on the
    /// fall reads two ways.
    pub wanted_and_released_in_one_record: u64,
    pub first_violation: String,
}

impl Enumeration {
    pub fn holds(&self) -> bool {
        self.fell_while_wanted + self.stayed_without_a_want + self.order_of_making + self.round_trip == 0
    }
}

fn starting_tables(parts: &Parts, hashes: &[[u64; 2]; 2]) -> Vec<(&'static str, Table)> {
    let alive = |_: u32, _: u32| true;
    let wanted = || {
        let mut table = Table::with_rules(Rules::default());
        table.want(X, ENTITY, Policy::Required);
        table.refresh();
        table
    };
    let mut available = wanted();
    available.apply(&Record::arrivals(0, vec![Arrival::at(X, hashes[0][0])]), parts, &alive);
    let mut orphan = available.clone();
    orphan.apply(&Record::commands(0, vec![Command::Release { asset: X, owner: ENTITY }]), parts, &alive);
    let mut failed = wanted();
    failed.apply(&Record::arrivals(0, vec![Arrival::failed(X, REASON_INVALID_PART)]), parts, &alive);
    vec![
        ("absent", Table::with_rules(Rules::default())),
        ("wanted and unanswered", wanted()),
        ("available under one want", available),
        ("available with no want", orphan),
        ("failed", failed),
    ]
}

/// Whether an element of the record ends a want of the asset.
fn ends_a_want(record: &Record, asset: AssetId) -> bool {
    record.commands.iter().any(|command| match command {
        Command::Release { asset: released, .. } => *released == asset,
        Command::Despawn { .. } => true,
        _ => false,
    })
}

pub fn run() -> Enumeration {
    let (parts, hashes) = parts();
    let mut commands = Vec::new();
    for asset in [X, Y] {
        commands.push(Command::Want { asset, owner: ENTITY, policy: Policy::Required });
        commands.push(Command::Want { asset, owner: MAKER, policy: Policy::Optional });
        commands.push(Command::Release { asset, owner: ENTITY });
        commands.push(Command::Release { asset, owner: MAKER });
    }
    commands.push(Command::Despawn { slot: 1, generation: 0 });
    let arrivals_of = |asset: AssetId, index: usize| {
        [None, Some(Arrival::at(asset, hashes[index][0])), Some(Arrival::at(asset, hashes[index][1])), Some(Arrival::failed(asset, REASON_INVALID_PART))]
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
    let starts = starting_tables(&parts, &hashes);
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
                table.apply(&record, &parts, &alive);
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
                // The canonical bytes do not depend on the order of making.
                if sequence.len() > 1 && sequence.iter().all(|command| matches!(command, Command::Want { .. })) {
                    let mut reversed = start.clone();
                    let commands: Vec<Command> = sequence.iter().rev().copied().collect();
                    reversed.apply(&Record { tick: 1, commands, arrivals: batch.clone() }, &parts, &alive);
                    if reversed.canonical_bytes() != after_first.canonical_bytes() {
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
                // The case the rule on the fall reads two ways: an entry with
                // no want, wanted and released again inside the record.
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
    #[test]
    fn every_small_record_keeps_the_invariants() {
        let result = super::run();
        assert!(result.holds(), "{}", result.first_violation);
        assert_eq!(result.records, 13_120);
    }
}
