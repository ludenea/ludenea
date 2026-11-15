//! The self-checks of the rules that a second reading of the design text
//! corrected, and of the controls of their scenarios, each applied to the
//! table alone: the gate's count through the pending parts, the count of a
//! budget over an entry in force that holds no want, and an arrival for an
//! entry with no want that a want of the first step names. They hold no
//! oracle.

use super::*;

const B: AssetId = 0x08;
const D: AssetId = 0x10;
const K: AssetId = 0x18;
const M: AssetId = 0x20;
const N: AssetId = 0x28;
const P: AssetId = 0x30;
const R: AssetId = 0x38;
const S: AssetId = 0x40;
const Z: AssetId = 0x48;
const HOST: Owner = Owner::World(Maker::Source(3));
const TOOL: Owner = Owner::World(Maker::Source(5));
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
    /// A part that lists these assets, each under the type of parameters,
    /// and whose length in bytes is the size its arrival names.
    fn sized(&mut self, salt: u32, lists: &[AssetId], len: usize) -> u64 {
        let dependencies: Vec<Dependency> = lists.iter().map(|asset| (*asset, PARAMETERS)).collect();
        let bytes = part_bytes(RepulsionTable::SPECIFICATION, &dependencies, len - part_len(&dependencies, 0), salt);
        let hash = fnv1a(&bytes);
        let part = Part::decode(&bytes).expect("a valid part");
        self.0.insert(hash, Arc::new(Resident { bytes, part }));
        hash
    }

    fn part(&mut self, salt: u32, lists: &[AssetId]) -> u64 {
        let len = part_len(&lists.iter().map(|asset| (*asset, PARAMETERS)).collect::<Vec<_>>(), 4);
        self.sized(salt, lists, len)
    }

    /// An arrival to a hash that names the size the manifest would give.
    fn arrival(&self, asset: AssetId, hash: u64) -> Arrival {
        Arrival::at(asset, hash).sized(self.0[&hash].bytes.len() as u64)
    }
}

fn want(asset: AssetId, owner: Owner, policy: Policy) -> Command {
    Command::Want { asset, asset_type: PARAMETERS, owner, policy }
}

fn release(asset: AssetId, owner: Owner) -> Command {
    Command::Release { asset, owner }
}

fn apply(table: &mut Table, parts: &Parts, commands: Vec<Command>, arrivals: Vec<Arrival>) -> Applied {
    let mut record = Record::arrivals(1, arrivals);
    record.commands = commands;
    table.apply(&record, parts, &|_, _| true)
}

fn availability(table: &Table, asset: AssetId) -> Option<Availability> {
    table.entries.get(&asset).map(|entry| entry.availability)
}

fn wants(table: &Table, asset: AssetId) -> Vec<(Owner, Policy)> {
    table.entries.get(&asset).map_or_else(Vec::new, |entry| entry.wants.clone())
}

/// The assets whose arrival the application of a record rejected.
fn rejected(applied: &Applied) -> Vec<AssetId> {
    applied.rejected.iter().map(|(asset, _)| *asset).collect()
}

/// The sum of the sizes that the available entries of a table hold.
fn in_force(table: &Table) -> u64 {
    table.entries.values().filter(|entry| matches!(entry.availability, Availability::Available(_))).map(|entry| entry.size).sum()
}

#[test]
fn the_gate_counts_through_the_pending_parts_to_any_depth_and_each_control_counts_otherwise() {
    let mut parts = Parts::default();
    let k = parts.part(1, &[]);
    let m = parts.part(2, &[K]);
    let r = parts.part(3, &[M]);
    let s = parts.part(4, &[R]);
    let gated = |rules: Rules, commands: Vec<Command>| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, commands, Vec::new());
        table
    };
    let on = |root: AssetId| vec![want(root, HOST, Policy::Required)];
    // A world gated on `R`, whose pending part lists `M`, whose pending part
    // lists `K`: without the arrival of `K` the gate stays closed on it, and
    // the complete batch opens it.
    let without_k = Record::arrivals(2, vec![parts.arrival(M, m), parts.arrival(R, r)]);
    let complete = Record::arrivals(2, vec![parts.arrival(K, k), parts.arrival(M, m), parts.arrival(R, r)]);
    assert_eq!(gated(Rules::default(), on(R)).gate(&without_k, &parts), vec![K]);
    assert!(gated(Rules::default(), on(R)).gate_open(&complete, &parts));
    // The build that counts two levels counts this one, and loses the third:
    // a world gated on `S`, whose closure has one level more.
    let two_levels = Rules { gate_follows_the_pending_parts_two_levels_alone: true, ..Rules::default() };
    let deeper = Record::arrivals(2, vec![parts.arrival(M, m), parts.arrival(R, r), parts.arrival(S, s)]);
    assert_eq!(gated(two_levels, on(R)).gate(&without_k, &parts), vec![K]);
    assert_eq!(gated(Rules::default(), on(S)).gate(&deeper, &parts), vec![K]);
    assert!(gated(two_levels, on(S)).gate_open(&deeper, &parts), "the want on `M` comes from the part of `R`, which the table holds under no want");
    // A pending parent that the table holds under an optional want of its
    // own counts the required want the core would add on it.
    let beside_its_own = vec![want(R, HOST, Policy::Required), want(M, TOOL, Policy::Optional)];
    assert_eq!(gated(Rules::default(), beside_its_own.clone()).gate(&without_k, &parts), vec![K]);
    let the_tables_wants = Rules { gate_counts_core_wants_from_the_tables_wants_alone: true, ..Rules::default() };
    assert!(gated(the_tables_wants, beside_its_own).gate_open(&without_k, &parts));
    // Under an optional root the want of the second level is optional, and
    // one control counts it as required.
    let optional = vec![want(R, HOST, Policy::Optional), want(S, HOST, Policy::Required)];
    let as_required = Rules { gate_counts_as_required_under_an_unwanted_pending_parent: true, ..Rules::default() };
    let beside = Record::arrivals(2, vec![parts.arrival(M, m), parts.arrival(R, r), Arrival::failed(S, REASON_INVALID_PART)]);
    assert!(gated(Rules::default(), optional.clone()).gate_open(&beside, &parts));
    assert_eq!(gated(as_required, optional).gate(&beside, &parts), vec![K]);
    // What answers the want under a pending parent that the table holds
    // under no want: a pending arrival or an entry in force, and each of two
    // controls takes one of them alone.
    let from_the_table = Rules { gate_answers_under_an_unwanted_pending_parent_from_the_table_alone: true, ..Rules::default() };
    let from_the_arrivals = Rules { gate_answers_under_an_unwanted_pending_parent_from_the_pending_arrivals_alone: true, ..Rules::default() };
    assert_eq!(gated(from_the_table, on(R)).gate(&complete, &parts), vec![K]);
    assert!(gated(from_the_arrivals, on(R)).gate_open(&complete, &parts));
    let in_force = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, vec![want(K, TOOL, Policy::Optional)], vec![parts.arrival(K, k)]);
        apply(&mut table, &parts, on(R), Vec::new());
        table
    };
    assert!(in_force(Rules::default()).gate_open(&without_k, &parts));
    assert!(in_force(from_the_table).gate_open(&without_k, &parts));
    assert_eq!(in_force(from_the_arrivals).gate(&without_k, &parts), vec![K]);
}

#[test]
fn an_entry_in_force_that_holds_no_want_stays_counted_though_the_record_holds_its_arrival_to_failed() {
    let mut parts = Parts::default();
    let (b, d, n, p) = (parts.sized(1, &[], 2_000), parts.sized(2, &[], 4_000), parts.sized(3, &[], 5_000), parts.sized(4, &[D], 2_500));
    // `B` required and `D` optional, both in force; then `N` and `P`, whose
    // part lists `D`, wanted as optional. `N` fits beside `B` and does not
    // fit beside `B` and `D`.
    let opened = |rules: Rules, budget: u64| {
        let mut table = Table { budget: Some(budget), ..Table::with_rules(rules) };
        apply(&mut table, &parts, vec![want(B, HOST, Policy::Required), want(D, HOST, Policy::Optional)], vec![parts.arrival(B, b), parts.arrival(D, d)]);
        apply(&mut table, &parts, vec![want(N, HOST, Policy::Optional), want(P, HOST, Policy::Optional)], Vec::new());
        table
    };
    let three = || vec![Arrival::failed(D, REASON_UNKNOWN_ASSET), parts.arrival(N, n), parts.arrival(P, p)];
    let as_before = Rules { budget_count_leaves_out_every_entry_an_arrival_to_failed_names: true, ..Rules::default() };

    // The record releases the last want of `D`: it holds no want once the
    // commands are applied and stays counted. `N` is rejected, the first
    // load of `P` fits beside it, and the want of `P` fails it.
    let mut table = opened(Rules::default(), 9_000);
    let applied = apply(&mut table, &parts, vec![release(D, HOST)], three());
    assert_eq!(rejected(&applied), vec![N]);
    assert_eq!(availability(&table, D), Some(Availability::Failed(REASON_UNKNOWN_ASSET)));
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Optional)]);
    assert_eq!(in_force(&table), 4_500);
    // Under the sentence as it read before, its room is free: `N` takes it,
    // `P` is rejected, no want names `D`, its arrival to failed changes
    // nothing, and the world ends the fixed point over its budget.
    let mut table = opened(as_before, 9_000);
    let applied = apply(&mut table, &parts, vec![release(D, HOST)], three());
    assert_eq!(rejected(&applied), vec![P]);
    assert_eq!(availability(&table, D), Some(Availability::Available(d)));
    assert!(wants(&table, D).is_empty());
    assert_eq!(in_force(&table), 11_000);
    // Where the first load does not fit beside the entry either, both are
    // rejected and the entry stays in force with no want.
    let mut table = opened(Rules::default(), 8_000);
    let applied = apply(&mut table, &parts, vec![release(D, HOST)], three());
    assert_eq!(rejected(&applied), vec![N, P]);
    assert_eq!(availability(&table, D), Some(Availability::Available(d)));
    assert_eq!(in_force(&table), 6_000);

    // The entry keeps its want: the arrival to failed fails it whatever the
    // test gives, and its room is free. One control counts it.
    let mut table = opened(Rules::default(), 9_000);
    let applied = apply(&mut table, &parts, Vec::new(), three());
    assert_eq!(rejected(&applied), vec![P]);
    assert_eq!(wants(&table, D), vec![(HOST, Policy::Optional)]);
    assert_eq!(in_force(&table), 7_000);
    let mut table = opened(Rules { budget_count_holds_the_failing_entries: true, ..Rules::default() }, 9_000);
    assert_eq!(rejected(&apply(&mut table, &parts, Vec::new(), three())), vec![N]);

    // The entry under the want the core added for its parent, and no other:
    // a want all the same, and its room is free. One control counts it.
    let under_its_parent = |rules: Rules| {
        let mut table = Table { budget: Some(10_000), ..Table::with_rules(rules) };
        apply(&mut table, &parts, vec![want(B, HOST, Policy::Required), want(P, HOST, Policy::Optional)], vec![parts.arrival(B, b), parts.arrival(D, d), parts.arrival(P, p)]);
        apply(&mut table, &parts, vec![want(N, HOST, Policy::Optional)], Vec::new());
        assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Optional)]);
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(D, REASON_UNKNOWN_ASSET), parts.arrival(N, n)]);
        assert_eq!(availability(&table, D), Some(Availability::Failed(REASON_UNKNOWN_ASSET)));
        rejected(&applied)
    };
    assert!(under_its_parent(Rules::default()).is_empty());
    assert_eq!(under_its_parent(Rules { budget_count_leaves_out_a_failing_entry_under_an_own_want_alone: true, ..Rules::default() }), vec![N]);

    // The entry due to fall, its last want released by the record before:
    // the closure of `P` reaches it through the part of the arrival of `P`,
    // so it stays counted. Under the other reading of what the count leaves
    // out, its room is free, `P` is rejected and the entry falls.
    let due_to_fall = |rules: Rules| {
        let mut table = opened(rules, 9_000);
        apply(&mut table, &parts, vec![release(D, HOST)], Vec::new());
        let applied = apply(&mut table, &parts, Vec::new(), three());
        (rejected(&applied), availability(&table, D))
    };
    assert_eq!(due_to_fall(Rules::default()), (vec![N], Some(Availability::Failed(REASON_UNKNOWN_ASSET))));
    let other = Rules { budget_count_leaves_out_an_entry_due_to_fall_that_an_arrival_to_failed_names: true, ..Rules::default() };
    assert_eq!(due_to_fall(other), (vec![P], None));
    assert_eq!(due_to_fall(as_before), (vec![P], None));
    // That other reading frees nothing of an entry whose last want the
    // record itself releases: it is not due to fall at this fixed point.
    let mut table = opened(other, 9_000);
    assert_eq!(rejected(&apply(&mut table, &parts, vec![release(D, HOST)], three())), vec![N]);
}

#[test]
fn an_arrival_for_an_entry_with_no_want_is_applied_where_a_want_of_the_first_step_names_it() {
    let mut parts = Parts::default();
    let (b1, b2, m1) = (parts.part(1, &[]), parts.part(2, &[]), parts.part(3, &[Z]));
    let (p1, p2, p3) = (parts.part(4, &[]), parts.part(5, &[B, Z]), parts.part(6, &[M]));
    let (z1, z2) = (parts.part(7, &[]), parts.part(8, &[]));
    let at = |hash: u64| Some(Availability::Available(hash));
    let of_p = vec![(Owner::Asset(P), Policy::Required)];
    // `P` required, `B` and `Z` optional, the three in force; `B` stands
    // before `P` in identifier order and `Z` after it.
    let opened = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        let commands = vec![want(P, HOST, Policy::Required), want(B, TOOL, Policy::Optional), want(Z, TOOL, Policy::Optional)];
        apply(&mut table, &parts, commands, vec![parts.arrival(B, b1), parts.arrival(P, p1), parts.arrival(Z, z1)]);
        table
    };
    let releases = || vec![release(B, TOOL), release(Z, TOOL)];
    // The same table once a record has released the last want of `B` and
    // of `Z`: two entries in force that hold no want.
    let released = |rules: Rules| {
        let mut table = opened(rules);
        apply(&mut table, &parts, releases(), Vec::new());
        table
    };
    let with_the_parent = || vec![parts.arrival(B, b2), parts.arrival(P, p2), parts.arrival(Z, z2)];
    let hashes = |table: &Table| (availability(table, B), availability(table, Z));
    let rule = |change: fn(&mut Rules)| {
        let mut rules = Rules::default();
        change(&mut rules);
        rules
    };
    let at_the_drain = rule(|rules| rules.arrival_for_an_entry_unwanted_at_the_drain_changes_nothing = true);
    let after_the_commands = rule(|rules| rules.arrival_for_an_entry_unwanted_after_the_commands_changes_nothing = true);
    let released_in_its_record = rule(|rules| rules.arrival_for_an_asset_released_in_its_record_changes_nothing = true);

    // The new part of the parent lists the two entries: the first step adds
    // its wants before any arrival is applied, and the three are applied.
    let mut table = released(Rules::default());
    let applied = apply(&mut table, &parts, Vec::new(), with_the_parent());
    assert_eq!(hashes(&table), (at(b2), at(z2)));
    assert_eq!((wants(&table, B), wants(&table, Z)), (of_p.clone(), of_p.clone()));
    assert_eq!(applied.events.len(), 3);
    // Two controls leave both entries at their former hash under the want
    // of their parent; a third reaches neither, no command of the record
    // having released their last want.
    for rules in [at_the_drain, after_the_commands] {
        let mut table = released(rules);
        let applied = apply(&mut table, &parts, Vec::new(), with_the_parent());
        assert_eq!(hashes(&table), (at(b1), at(z1)));
        assert_eq!((wants(&table, B), applied.events.len()), (of_p.clone(), 1));
    }
    let mut table = released(released_in_its_record);
    apply(&mut table, &parts, Vec::new(), with_the_parent());
    assert_eq!(hashes(&table), (at(b2), at(z2)));
    // The wants added at the arrival of the parent itself: the entry that
    // stands before its parent keeps its former hash, the one after it
    // takes the new one.
    let mut table = released(rule(|rules| rules.wants_of_a_part_added_where_its_arrival_is_applied = true));
    apply(&mut table, &parts, Vec::new(), with_the_parent());
    assert_eq!(hashes(&table), (at(b1), at(z2)));
    assert_eq!((wants(&table, B), wants(&table, Z)), (of_p.clone(), of_p.clone()));

    // Without the arrival of the parent no want names the two entries:
    // their arrivals change nothing and they fall. One control applies them
    // first.
    let without_the_parent = || vec![parts.arrival(B, b2), parts.arrival(Z, z2)];
    let mut table = released(Rules::default());
    let applied = apply(&mut table, &parts, Vec::new(), without_the_parent());
    assert_eq!((hashes(&table), applied.events.len(), applied.fallen.clone()), ((None, None), 0, vec![B, Z]));
    let mut table = released(rule(|rules| rules.unnamed_arrivals_applied = true));
    assert_eq!(apply(&mut table, &parts, Vec::new(), without_the_parent()).events.len(), 2);

    // Two levels down: the part of the parent lists `M`, whose part lists
    // `Z`. The first step follows both, and one control stops at the first.
    let at_depth_two = || vec![parts.arrival(M, m1), parts.arrival(P, p3), parts.arrival(Z, z2)];
    let mut table = released(Rules::default());
    apply(&mut table, &parts, Vec::new(), at_depth_two());
    assert_eq!((availability(&table, B), availability(&table, Z)), (None, at(z2)));
    assert_eq!(wants(&table, Z), vec![(Owner::Asset(M), Policy::Required)]);
    let mut table = released(rule(|rules| rules.first_step_adds_one_level_alone = true));
    apply(&mut table, &parts, Vec::new(), at_depth_two());
    assert_eq!((availability(&table, M), availability(&table, Z)), (at(m1), at(z1)));
    assert_eq!(wants(&table, Z), vec![(Owner::Asset(M), Policy::Required)]);

    // The batch in the record that releases the two wants: the releases are
    // applied first, the first step names the two entries again, and the
    // arrivals are applied. Two controls leave them at their former hash;
    // the one on an entry that held no want at the drain reaches neither.
    let in_the_record = |rules: Rules| {
        let mut table = opened(rules);
        apply(&mut table, &parts, releases(), with_the_parent());
        hashes(&table)
    };
    assert_eq!(in_the_record(Rules::default()), (at(b2), at(z2)));
    assert_eq!(in_the_record(released_in_its_record), (at(b1), at(z1)));
    assert_eq!(in_the_record(after_the_commands), (at(b1), at(z1)));
    assert_eq!(in_the_record(at_the_drain), (at(b2), at(z2)));

    // At a restore: the table read back, every available entry awaiting a
    // marked answer. The answer of `Z`, which holds no want, is applied
    // under the want added from the part of `M`, an ordinary arrival of the
    // batch; one control applies it under a want from the part of a marked
    // answer alone. Both count it as an answer that differed.
    let restored = |rules: Rules| {
        let mut table = Table::decode(&released(Rules::default()).canonical_bytes(), rules).expect("the table decodes");
        table.awaiting = BTreeSet::from([B, P, Z]);
        table
    };
    let from_an_answer_alone = rule(|rules| rules.marked_answer_named_from_the_part_of_an_answer_alone = true);
    let answers_at_depth_two = || vec![parts.arrival(B, b1).marked(), parts.arrival(M, m1), parts.arrival(P, p3).marked(), parts.arrival(Z, z2).marked()];
    let mut table = restored(Rules::default());
    let applied = apply(&mut table, &parts, Vec::new(), answers_at_depth_two());
    assert_eq!((availability(&table, Z), applied.differed.clone()), (at(z2), vec![P, Z]));
    let mut table = restored(from_an_answer_alone);
    let applied = apply(&mut table, &parts, Vec::new(), answers_at_depth_two());
    assert_eq!((availability(&table, Z), applied.differed.clone()), (at(z1), vec![P, Z]));
    assert_eq!(wants(&table, Z), vec![(Owner::Asset(M), Policy::Required)]);
    // Where the want comes from the part of a marked answer, that control
    // applies the answer as the default does.
    let mut table = restored(from_an_answer_alone);
    apply(&mut table, &parts, Vec::new(), vec![parts.arrival(B, b2).marked(), parts.arrival(P, p2).marked(), parts.arrival(Z, z2).marked()]);
    assert_eq!(hashes(&table), (at(b2), at(z2)));
}
