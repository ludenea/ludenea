//! The self-checks of what the table offers the rows judged on a magnitude:
//! an evaluation made apart from the fixed point that makes it, which
//! changes nothing and gives what the fixed point gives. They hold no oracle.

use super::*;

const D: AssetId = 0x10;
const A: AssetId = 0x20;
const P: AssetId = 0x30;
const HOST: Owner = Owner::World(Maker::Source(3));
const PARAMETERS: AssetType = AssetType::Parameters;

/// The parts a self-check names, resident by their hash.
#[derive(Default)]
struct Parts(BTreeMap<u64, Arc<Resident>>);

impl Residents for Parts {
    fn get(&self, hash: u64) -> Option<Arc<Resident>> {
        self.0.get(&hash).cloned()
    }
}

impl Parts {
    /// A part of a given length in bytes: what a budget counts.
    fn sized(&mut self, salt: u32, dependencies: &[Dependency], len: usize) -> u64 {
        let bytes = part_bytes(RepulsionTable::SPECIFICATION, dependencies, len - part_len(dependencies, 0), salt);
        let hash = fnv1a(&bytes);
        let part = Part::decode(&bytes).expect("a valid part");
        self.0.insert(hash, Arc::new(Resident { bytes, part }));
        hash
    }

    /// An arrival to a hash that names the size the manifest would give.
    fn arrival(&self, asset: AssetId, hash: u64) -> Arrival {
        Arrival::at(asset, hash).sized(self.0[&hash].bytes.len() as u64)
    }
}

fn required(asset: AssetId) -> Command {
    Command::Want { asset, asset_type: PARAMETERS, owner: HOST, policy: Policy::Required }
}

#[test]
fn the_groups_of_a_record_are_derived_and_decided_apart_as_its_fixed_point_decides_them() {
    let mut parts = Parts::default();
    let (d, a, p) = (parts.sized(1, &[], 60), parts.sized(2, &[], 100), parts.sized(3, &[(D, PARAMETERS)], 80));
    // A table under a budget that requires `A` and `P`, whose part lists
    // `D`: the group of `A` fits, and the group of `P` with `D` does not.
    let mut table = Table { budget: Some(150), ..Table::default() };
    table.apply(&Record::commands(1, vec![required(A), required(P)]), &parts, &|_, _| true);
    let record = Record::arrivals(2, vec![parts.arrival(D, d), parts.arrival(A, a), parts.arrival(P, p)]);
    let before = table.typed_bytes();
    let without_a_want = table.entries_without_a_want();
    assert!(without_a_want.is_empty());
    let (rejected, roots) = table.groups_decided(&record, &parts, &without_a_want);
    assert_eq!(table.typed_bytes(), before, "a derivation made apart changes nothing");
    assert_eq!(roots, 2, "the closure of each root of the table is read");
    assert_eq!(rejected, BTreeSet::from([D, P]));
    // The fixed point rejects the same arrivals.
    let applied = table.apply(&record, &parts, &|_, _| true);
    assert_eq!(applied.rejected.iter().map(|(asset, _)| *asset).collect::<BTreeSet<_>>(), rejected);
    assert!(!table.groups_form().is_empty());
    // A table with no budget derives nothing.
    let mut free = Table::default();
    free.apply(&Record::commands(1, vec![required(A)]), &parts, &|_, _| true);
    assert_eq!(free.groups_decided(&record, &parts, &[]), (BTreeSet::new(), 0));
    // An entry that holds no want is one the next fixed point takes as due to fall.
    free.apply(&Record::arrivals(2, vec![parts.arrival(A, a)]), &parts, &|_, _| true);
    free.apply(&Record::commands(3, vec![Command::Release { asset: A, owner: HOST }]), &parts, &|_, _| true);
    assert_eq!(free.entries_without_a_want(), vec![A]);
}

#[test]
fn the_verdict_of_the_gate_reads_the_count_of_a_restore_and_names_none_of_its_entries() {
    let mut parts = Parts::default();
    let (d, a, p) = (parts.sized(1, &[], 60), parts.sized(2, &[], 100), parts.sized(3, &[(D, PARAMETERS)], 80));
    // A table that holds `A`, `P` and the dependency `D` of `P` available,
    // read back as a restore under the content at hand leaves it: every
    // available entry awaits its answer.
    let mut table = Table::default();
    table.apply(&Record::commands(1, vec![required(A), required(P)]), &parts, &|_, _| true);
    table.apply(&Record::arrivals(2, vec![parts.arrival(D, d), parts.arrival(A, a), parts.arrival(P, p)]), &parts, &|_, _| true);
    let restored = |rules: Rules| {
        let mut restored = Table::decode(&table.typed_bytes(), rules).expect("the bytes of a table decode");
        restored.awaiting = restored.entries.keys().copied().collect();
        restored
    };
    let answer = |asset: AssetId, hash: u64| parts.arrival(asset, hash).marked();
    let nothing = Record { tick: 3, ..Record::default() };
    let one = Record::arrivals(3, vec![answer(A, a)]);
    let all = Record::arrivals(3, vec![answer(D, d), answer(A, a), answer(P, p)]);
    // The form that copies the table gives the verdicts of the incremental one.
    for rules in [Rules::default(), Rules { gate_copies_the_table: true, ..Rules::default() }] {
        let restored = restored(rules);
        assert_eq!(restored.gate_verdict(&nothing, &parts), (false, 3), "nothing is pending: every entry is awaited");
        assert_eq!(restored.gate(&nothing, &parts), vec![D, A, P]);
        assert_eq!(restored.gate_verdict(&one, &parts), (false, 2), "one answer is pending");
        assert_eq!(restored.gate(&one, &parts), vec![D, P], "the whole answer names what the verdict counts");
        assert_eq!(restored.gate_verdict(&all, &parts), (true, 0), "every answer is pending");
        assert!(restored.gate(&all, &parts).is_empty());
        assert_eq!(restored.awaiting.len(), 3, "an evaluation applies nothing");
    }
    // A table that is not gated is open whatever its pending record holds.
    assert_eq!(table.gate_verdict(&all, &parts), (true, 0));
    // A required asset unanswered and no restore: the verdict is closed
    // until its arrival is pending, and nothing is awaited by a restore.
    let mut gated = Table::default();
    gated.apply(&Record::commands(1, vec![required(A)]), &parts, &|_, _| true);
    assert_eq!(gated.gate_verdict(&Record { tick: 2, ..Record::default() }, &parts), (false, 0));
    assert_eq!(gated.gate_verdict(&Record::arrivals(2, vec![parts.arrival(D, d)]), &parts), (false, 0));
    assert_eq!(gated.gate_verdict(&Record::arrivals(2, vec![parts.arrival(A, a)]), &parts), (true, 0));
}

#[test]
fn the_table_counts_each_change_it_follows_and_a_gate_that_is_not_asked_keeps_nothing() {
    let mut parts = Parts::default();
    let (d, a, p) = (parts.sized(1, &[], 60), parts.sized(2, &[], 100), parts.sized(3, &[(D, PARAMETERS)], 80));
    let records = [
        // Two wants applied.
        Record::commands(1, vec![required(A), required(P)]),
        // The want the core adds on `D`, then three arrivals applied.
        Record::arrivals(2, vec![parts.arrival(D, d), parts.arrival(A, a), parts.arrival(P, p)]),
        // A release, and the want of the core on `D` that ends with it.
        Record::commands(3, vec![Command::Release { asset: P, owner: HOST }]),
        // The falls of `P` and of `D`.
        Record { tick: 4, ..Record::default() },
    ];
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        let mut seen = Vec::new();
        for record in &records {
            table.apply(record, &parts, &|_, _| true);
            seen.push((table.changes_followed(), table.blocked()));
        }
        (seen, table.typed_bytes())
    };
    let (with_the_form, table) = run(Rules::default());
    assert_eq!(with_the_form, vec![(2, true), (6, false), (8, false), (10, false)]);
    // The variant of the build that keeps neither the set nor the count,
    // whose gate the host does not ask: the same changes, the same table,
    // and a world that steps at every boundary.
    let (without, same_table) = run(Rules { required_set_not_kept: true, gate_not_asked: true, ..Rules::default() });
    assert_eq!(without, vec![(2, false), (6, false), (8, false), (10, false)]);
    assert_eq!(same_table, table);
    // Under that variant the count of a restore is not kept either.
    let mut unasked = Table::decode(&table, Rules { required_set_not_kept: true, gate_not_asked: true, ..Rules::default() }).expect("the bytes of a table decode");
    unasked.awaiting = BTreeSet::from([A]);
    unasked.apply(&Record::arrivals(5, vec![parts.arrival(A, a).marked()]), &parts, &|_, _| true);
    assert_eq!(unasked.awaiting.len(), 1);
    assert!(!unasked.blocked());
}
