//! The self-checks of the asset table: the rules the design text states, as
//! amended, each applied to the table alone. They hold no oracle.

use super::*;

const D: AssetId = 0x10;
const F: AssetId = 0x18;
const A: AssetId = 0x20;
const P: AssetId = 0x30;
const HOST: Owner = Owner::World(Maker::Source(3));
const TOOL: Owner = Owner::World(Maker::Source(5));
const BASE: Owner = Owner::World(Maker::Contributor(7));
const PARAMETERS: AssetType = AssetType::Parameters;
const SECOND: AssetType = AssetType::Second;

/// The parts a self-check names, resident by their hash.
#[derive(Default)]
struct Parts(BTreeMap<u64, Arc<Resident>>);

impl Residents for Parts {
    fn get(&self, hash: u64) -> Option<Arc<Resident>> {
        self.0.get(&hash).cloned()
    }
}

impl Parts {
    fn part(&mut self, salt: u32, dependencies: &[Dependency]) -> u64 {
        let bytes = part_bytes(RepulsionTable::SPECIFICATION, dependencies, 4, salt);
        let hash = fnv1a(&bytes);
        let part = Part::decode(&bytes).expect("a valid part");
        self.0.insert(hash, Arc::new(Resident { bytes, part }));
        hash
    }
}

fn want(asset: AssetId, asset_type: AssetType, owner: Owner, policy: Policy) -> Command {
    Command::Want { asset, asset_type, owner, policy }
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

#[test]
fn a_part_lists_each_dependency_with_its_type_and_a_part_without_one_keeps_its_bytes() {
    let plain = part_bytes(RepulsionTable::SPECIFICATION, &[], 8, 1);
    assert_eq!(plain.len(), 28, "a format word, two parameters, two counts and the payload");
    let listing = part_bytes(RepulsionTable::SPECIFICATION, &[(D, SECOND)], 8, 1);
    assert_eq!(listing.len(), part_len(&[(D, SECOND)], 8));
    assert_eq!(Part::decode(&listing).expect("a valid part").dependencies, vec![(D, SECOND)]);
    let other_type = part_bytes(RepulsionTable::SPECIFICATION, &[(D, PARAMETERS)], 8, 1);
    assert_ne!(fnv1a(&listing), fnv1a(&other_type), "the content hash of an asset that has a dependency covers the dependency's type");
    assert_eq!(Part::decode(&listing[..listing.len() - 1]).err(), Some(REASON_INVALID_PART));
}

#[test]
fn the_first_want_writes_the_type_and_a_want_of_another_type_is_a_recorded_refusal() {
    let parts = Parts::default();
    let mut table = Table::default();
    let applied = apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional), want(A, SECOND, TOOL, Policy::Required)], Vec::new());
    assert_eq!(applied.refusals, vec![(1, Refused::AnotherType)]);
    assert_eq!(table.entries[&A].asset_type, PARAMETERS);
    assert_eq!(wants(&table, A), vec![(HOST, Policy::Optional)]);
    assert_eq!(applied.left, vec![(A, PARAMETERS)], "the refused want leaves nothing");
    // The maker that holds the want cannot restate it under another type either.
    let applied = apply(&mut table, &parts, vec![want(A, SECOND, HOST, Policy::Required)], Vec::new());
    assert_eq!(applied.refusals, vec![(0, Refused::AnotherType)]);
    assert_eq!(wants(&table, A), vec![(HOST, Policy::Optional)], "a refusal changes nothing, the policy included");
    // The type is carried by the canonical bytes, as the stable name of its schema entry.
    let bytes = table.canonical_bytes();
    let name = PARAMETERS.stable_name().as_bytes();
    assert!(bytes.windows(name.len()).any(|window| window == name));
    let read = Table::decode(&bytes, Rules::default()).expect("the table decodes");
    assert_eq!(read.entries[&A].asset_type, PARAMETERS);
    assert_eq!(read.canonical_bytes(), bytes);
    let mut second = Table::default();
    apply(&mut second, &parts, vec![want(A, SECOND, HOST, Policy::Optional)], Vec::new());
    assert_ne!(second.canonical_bytes(), bytes, "two tables that differ in the type of one entry differ in their bytes");
}

#[test]
fn an_entry_failed_for_its_type_takes_the_type_of_the_next_want_that_states_another() {
    let parts = Parts::default();
    let mut table = Table::default();
    apply(&mut table, &parts, vec![want(A, SECOND, HOST, Policy::Required)], Vec::new());
    // Until that failure has arrived, a want of another type is refused.
    let applied = apply(&mut table, &parts, vec![want(A, PARAMETERS, TOOL, Policy::Optional)], Vec::new());
    assert_eq!(applied.refusals, vec![(0, Refused::AnotherType)]);
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(A, REASON_ANOTHER_TYPE)]);
    assert_eq!(applied.events, vec![Event::Failed { asset: A, code: REASON_ANOTHER_TYPE }]);
    assert!(!table.blocked(), "a failure is an answer");
    let applied = apply(&mut table, &parts, vec![want(A, PARAMETERS, TOOL, Policy::Optional)], Vec::new());
    assert!(applied.refusals.is_empty());
    assert_eq!(table.entries[&A].asset_type, PARAMETERS);
    assert_eq!(availability(&table, A), Some(Availability::Unavailable), "the want is the new load");
    assert_eq!(applied.left, vec![(A, PARAMETERS)], "a want leaves with the type its entry holds");
    assert_eq!(wants(&table, A), vec![(HOST, Policy::Required), (TOOL, Policy::Optional)]);
    assert!(table.blocked(), "the retry gates where a required want names the asset");
    // An entry failed for another reason keeps its type.
    let mut other = Table::default();
    apply(&mut other, &parts, vec![want(A, SECOND, HOST, Policy::Required)], vec![Arrival::failed(A, REASON_UNKNOWN_ASSET)]);
    let applied = apply(&mut other, &parts, vec![want(A, PARAMETERS, TOOL, Policy::Optional)], Vec::new());
    assert_eq!(applied.refusals, vec![(0, Refused::AnotherType)]);
    assert_eq!(availability(&other, A), Some(Availability::Failed(REASON_UNKNOWN_ASSET)));
    // Not taken: the first want deciding even when wrong.
    let mut held = Table::with_rules(Rules { first_type_holds: true, ..Rules::default() });
    apply(&mut held, &parts, vec![want(A, SECOND, HOST, Policy::Required)], vec![Arrival::failed(A, REASON_ANOTHER_TYPE)]);
    let applied = apply(&mut held, &parts, vec![want(A, PARAMETERS, TOOL, Policy::Optional)], Vec::new());
    assert_eq!(applied.refusals, vec![(0, Refused::AnotherType)]);
}

#[test]
fn a_want_added_on_a_failed_entry_returns_it_to_unavailable_and_a_change_of_policy_does_not() {
    let parts = Parts::default();
    let mut table = Table::default();
    apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required)], vec![Arrival::failed(A, REASON_ARTIFACT_GONE)]);
    assert_eq!(availability(&table, A), Some(Availability::Failed(REASON_ARTIFACT_GONE)));
    // A command that only changes the policy of a want its maker holds adds no want and retries nothing.
    let applied = apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional)], Vec::new());
    assert_eq!(availability(&table, A), Some(Availability::Failed(REASON_ARTIFACT_GONE)));
    assert!(applied.left.is_empty());
    assert_eq!(applied.wanted, vec![(A, PARAMETERS)]);
    // A want another maker adds is the new load.
    let applied = apply(&mut table, &parts, vec![want(A, PARAMETERS, TOOL, Policy::Required)], Vec::new());
    assert_eq!(availability(&table, A), Some(Availability::Unavailable));
    assert_eq!(applied.left, vec![(A, PARAMETERS)]);
    assert!(table.blocked(), "a required want gates on the retry");
    let bytes = table.canonical_bytes();
    assert_eq!(Table::decode(&bytes, Rules::default()).expect("the table decodes").unanswered(), vec![(A, PARAMETERS)], "a save holds the retry as any unanswered want");
    // A maker that holds a want retries by releasing it and wanting again.
    let mut again = Table::default();
    apply(&mut again, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::failed(A, REASON_ARTIFACT_GONE)]);
    let applied = apply(&mut again, &parts, vec![release(A, HOST), want(A, PARAMETERS, HOST, Policy::Optional)], Vec::new());
    assert_eq!(availability(&again, A), Some(Availability::Unavailable));
    assert_eq!(applied.left, vec![(A, PARAMETERS)]);
    // Not taken: the entry staying failed until the retry's arrival.
    let mut stays = Table::with_rules(Rules { failed_entry_stays_failed_on_a_want: true, ..Rules::default() });
    apply(&mut stays, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required)], vec![Arrival::failed(A, REASON_ARTIFACT_GONE)]);
    let applied = apply(&mut stays, &parts, vec![want(A, PARAMETERS, TOOL, Policy::Required)], Vec::new());
    assert_eq!(availability(&stays, A), Some(Availability::Failed(REASON_ARTIFACT_GONE)));
    assert_eq!(applied.left, vec![(A, PARAMETERS)]);
    assert!(!stays.blocked());
}

#[test]
fn a_want_leaves_only_where_its_asset_is_unanswered_or_failed() {
    let mut parts = Parts::default();
    let hash = parts.part(1, &[]);
    let mut table = Table::default();
    let applied = apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(A, hash)]);
    assert_eq!(applied.left, vec![(A, PARAMETERS)], "the test is made when the want is applied, before the arrival of the same record");
    assert_eq!(applied.events, vec![Event::Arrived { asset: A, hash }]);
    // A want added for an asset the table holds available leaves nothing.
    let applied = apply(&mut table, &parts, vec![want(A, PARAMETERS, TOOL, Policy::Required)], Vec::new());
    assert!(applied.left.is_empty());
    assert_eq!(applied.wanted, vec![(A, PARAMETERS)]);
    // A second want of an asset still unanswered leaves as the first did.
    let applied = apply(&mut table, &parts, vec![want(D, PARAMETERS, HOST, Policy::Optional)], Vec::new());
    assert_eq!(applied.left, vec![(D, PARAMETERS)]);
    let applied = apply(&mut table, &parts, vec![want(D, PARAMETERS, TOOL, Policy::Optional)], Vec::new());
    assert_eq!(applied.left, vec![(D, PARAMETERS)]);
    // A want added and ended inside one record is not in the state extracted at the tick's end.
    let applied = apply(&mut table, &parts, vec![want(F, PARAMETERS, HOST, Policy::Optional), release(F, HOST)], Vec::new());
    assert!(applied.left.is_empty());
    assert_eq!(availability(&table, F), Some(Availability::Unavailable), "the entry keeps its place for one tick boundary");
    let mut leaves = Table::with_rules(Rules { a_want_ended_in_its_record_still_leaves: true, ..Rules::default() });
    let applied = apply(&mut leaves, &parts, vec![want(F, PARAMETERS, HOST, Policy::Optional), release(F, HOST)], Vec::new());
    assert_eq!(applied.left, vec![(F, PARAMETERS)]);
}

#[test]
fn the_core_adds_and_recomputes_the_wants_of_dependencies_wherever_a_want_is_applied_or_released() {
    let mut parts = Parts::default();
    let d = parts.part(1, &[]);
    let p = parts.part(2, &[(D, PARAMETERS)]);
    let mut table = Table::default();
    let applied = apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(P, p), Arrival::at(D, d)]);
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Optional)]);
    assert_eq!(table.entries[&D].asset_type, PARAMETERS, "the type the parent's part lists for it");
    assert_eq!(applied.left, vec![(P, PARAMETERS)], "the arrival of the batch answers the want the core added");
    assert_eq!(applied.events.len(), 2);
    // A required want added on a parent that held optional wants alone makes
    // its dependencies' wants required at the same point, with no batch.
    apply(&mut table, &parts, vec![want(P, PARAMETERS, TOOL, Policy::Required)], Vec::new());
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)]);
    apply(&mut table, &parts, vec![release(P, TOOL)], Vec::new());
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Optional)]);
    // The dependency's want ends when the parent's last want ends.
    apply(&mut table, &parts, vec![release(P, HOST)], Vec::new());
    assert!(wants(&table, P).is_empty() && wants(&table, D).is_empty());
    assert_eq!(availability(&table, D), Some(Availability::Available(d)), "still available until it falls");
    // A parent wanted again at the tick at which it would have fallen keeps
    // its dependencies, which have not fallen yet.
    let applied = apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], Vec::new());
    assert!(applied.fallen.is_empty());
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)]);
    assert_eq!(availability(&table, D), Some(Availability::Available(d)));
    assert!(applied.left.is_empty(), "both are available: nothing leaves");
    // Left alone, they fall together at the fixed point of the tick after.
    apply(&mut table, &parts, vec![release(P, HOST)], Vec::new());
    let applied = apply(&mut table, &parts, Vec::new(), Vec::new());
    assert_eq!(applied.fallen, vec![D, P]);
    assert!(table.entries.is_empty());
}

#[test]
fn a_parent_whose_part_changes_gives_its_wants_to_the_dependencies_the_new_part_lists() {
    let mut parts = Parts::default();
    let d = parts.part(1, &[]);
    let f = parts.part(2, &[]);
    let first = parts.part(3, &[(D, PARAMETERS)]);
    let second = parts.part(4, &[(F, PARAMETERS)]);
    let mut table = Table::default();
    apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(P, first), Arrival::at(D, d)]);
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(P, second), Arrival::at(F, f)]);
    assert_eq!(wants(&table, F), vec![(Owner::Asset(P), Policy::Required)]);
    assert!(wants(&table, D).is_empty(), "the former dependency lost its want");
    assert_eq!(applied.events, vec![Event::Arrived { asset: F, hash: f }, Event::HashChanged { asset: P, from: first, to: second }]);
    // A parent that brings a dependency no arrival of its batch answers: the
    // want the core added leaves the domain, and gates where it is required.
    let mut alone = Table::default();
    apply(&mut alone, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(P, first), Arrival::at(D, d)]);
    let applied = apply(&mut alone, &parts, Vec::new(), vec![Arrival::at(P, second)]);
    assert_eq!(applied.left, vec![(F, PARAMETERS)]);
    assert!(alone.blocked());
    assert_eq!(alone.required_unanswered(), vec![F]);
}

#[test]
fn a_want_the_core_adds_states_a_type_only_where_it_is_the_first_and_is_never_refused() {
    let mut parts = Parts::default();
    let p = parts.part(1, &[(D, PARAMETERS)]);
    let mut table = Table::default();
    apply(&mut table, &parts, vec![want(D, SECOND, HOST, Policy::Optional), want(P, PARAMETERS, HOST, Policy::Required)], Vec::new());
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(P, p)]);
    assert!(applied.refusals.is_empty());
    assert_eq!(table.entries[&D].asset_type, SECOND, "the entry keeps the type its first want stated");
    assert_eq!(wants(&table, D), vec![(HOST, Policy::Optional), (Owner::Asset(P), Policy::Required)]);
    // Added on a failed entry, the core's want is the new load as any other.
    let mut failed = Table::default();
    apply(&mut failed, &parts, vec![want(D, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::failed(D, REASON_UNKNOWN_ASSET)]);
    let applied = apply(&mut failed, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(P, p)]);
    assert_eq!(availability(&failed, D), Some(Availability::Unavailable));
    assert_eq!(applied.left, vec![(P, PARAMETERS), (D, PARAMETERS)]);
}

#[test]
fn one_event_per_asset_that_became_available_changed_or_failed_and_none_for_an_arrival_that_changes_nothing() {
    let mut parts = Parts::default();
    let (one, two) = (parts.part(1, &[]), parts.part(2, &[]));
    let mut table = Table::default();
    let applied = apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(A, one)]);
    assert_eq!(applied.events, vec![Event::Arrived { asset: A, hash: one }], "a first arrival has its event");
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(A, one)]);
    assert!(applied.events.is_empty(), "the same hash changes nothing");
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(A, two)]);
    assert_eq!(applied.events, vec![Event::HashChanged { asset: A, from: one, to: two }]);
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(A, REASON_UNKNOWN_ASSET)]);
    assert_eq!(applied.events, vec![Event::Failed { asset: A, code: REASON_UNKNOWN_ASSET }]);
    assert_eq!(applied.failed_required, vec![(A, REASON_UNKNOWN_ASSET)]);
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(A, REASON_UNKNOWN_ASSET)]);
    assert!(applied.events.is_empty() && applied.failed_required.is_empty(), "failed for the same reason changes nothing");
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(A, REASON_ARTIFACT_GONE)]);
    assert_eq!(applied.events, vec![Event::Failed { asset: A, code: REASON_ARTIFACT_GONE }], "another reason is another failure");
    // An arrival for an asset no want names is recorded and changes nothing.
    let applied = apply(&mut table, &parts, vec![release(A, HOST)], vec![Arrival::at(A, one)]);
    assert!(applied.events.is_empty());
    assert_eq!(applied.unnamed_arrivals, 1);
    assert_eq!(availability(&table, A), Some(Availability::Failed(REASON_ARTIFACT_GONE)));
}

#[test]
fn an_answer_to_a_restore_at_a_hash_emits_nothing_and_one_that_fails_is_a_failure_as_any_other() {
    let mut parts = Parts::default();
    let (one, two) = (parts.part(1, &[]), parts.part(2, &[]));
    let instantiate = Command::Instantiate { asset: A, asset_type: PARAMETERS, policy: Policy::Required, reload: true };
    let saved = {
        let mut table = Table::default();
        apply(&mut table, &parts, vec![instantiate], vec![Arrival::at(A, one)]);
        assert_eq!(table.instances[0].instantiations, 1);
        table.canonical_bytes()
    };
    let restored = |rules: Rules| {
        let mut table = Table::decode(&saved, rules).expect("the table decodes");
        table.awaiting.insert(A);
        table.refresh();
        table
    };
    let mut same = restored(Rules::default());
    assert!(same.blocked(), "the world is gated on its answers");
    let applied = apply(&mut same, &parts, Vec::new(), vec![Arrival::at(A, one).marked()]);
    assert!(applied.events.is_empty() && applied.differed.is_empty() && !same.blocked());
    let mut another = restored(Rules::default());
    let applied = apply(&mut another, &parts, Vec::new(), vec![Arrival::at(A, two).marked()]);
    assert!(applied.events.is_empty(), "no event on a marked transition to another hash");
    assert_eq!(applied.differed, vec![A]);
    assert_eq!(another.instances[0].instantiations, 1, "nothing is instantiated again");
    let mut failed = restored(Rules::default());
    let applied = apply(&mut failed, &parts, Vec::new(), vec![Arrival::failed(A, REASON_UNKNOWN_ASSET).marked()]);
    assert_eq!(applied.events, vec![Event::Failed { asset: A, code: REASON_UNKNOWN_ASSET }]);
    assert_eq!(applied.failed_required, vec![(A, REASON_UNKNOWN_ASSET)]);
    assert_eq!(applied.differed, vec![A]);
    assert!(failed.instances[0].failed, "the instance that required the asset is failed");
    // Not taken: no event, the instance left as it was, or failed all the same.
    let mut silent = restored(Rules { marked_failure_emits_nothing: true, ..Rules::default() });
    let applied = apply(&mut silent, &parts, Vec::new(), vec![Arrival::failed(A, REASON_UNKNOWN_ASSET).marked()]);
    assert!(applied.events.is_empty() && !silent.instances[0].failed);
    let mut failing = restored(Rules { marked_failure_emits_nothing: true, marked_failure_fails_the_instance: true, ..Rules::default() });
    let applied = apply(&mut failing, &parts, Vec::new(), vec![Arrival::failed(A, REASON_UNKNOWN_ASSET).marked()]);
    assert!(applied.events.is_empty() && failing.instances[0].failed);
}

#[test]
fn the_wants_of_the_worlds_own_entry_stand_contributors_first_and_carry_the_kind_of_their_maker() {
    let parts = Parts::default();
    let mut table = Table::default();
    let entity = Owner::Entity { slot: 2, generation: 0 };
    let commands = vec![want(A, PARAMETERS, entity, Policy::Optional), want(A, PARAMETERS, TOOL, Policy::Optional), want(A, PARAMETERS, HOST, Policy::Optional), want(A, PARAMETERS, BASE, Policy::Required)];
    apply(&mut table, &parts, commands.clone(), Vec::new());
    let order: Vec<Owner> = wants(&table, A).into_iter().map(|(owner, _)| owner).collect();
    assert_eq!(order, vec![BASE, HOST, TOOL, entity], "contributors, then command sources, each by identity, then entities");
    let mut reversed = Table::default();
    apply(&mut reversed, &parts, commands.into_iter().rev().collect(), Vec::new());
    assert_eq!(reversed.canonical_bytes(), table.canonical_bytes(), "the bytes do not depend on the order of making");
    // Two makers of one identity and two kinds never share a want, and their wants differ in their bytes.
    let by = |owner: Owner| {
        let mut table = Table::default();
        apply(&mut table, &parts, vec![want(A, PARAMETERS, owner, Policy::Optional)], Vec::new());
        table.canonical_bytes()
    };
    assert_ne!(by(Owner::World(Maker::Contributor(3))), by(Owner::World(Maker::Source(3))));
}

#[test]
fn the_gate_counts_a_want_the_core_would_add_from_the_part_of_a_pending_arrival() {
    let mut parts = Parts::default();
    let d = parts.part(1, &[]);
    let p = parts.part(2, &[(D, PARAMETERS)]);
    let mut table = Table::default();
    apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], Vec::new());
    assert!(table.blocked());
    let alone = Record::arrivals(2, vec![Arrival::at(P, p)]);
    assert!(!table.gate_open(&alone, &parts), "a parent pending without its closure keeps the gate closed");
    let complete = Record::arrivals(2, vec![Arrival::at(P, p), Arrival::at(D, d)]);
    assert!(table.gate_open(&complete, &parts));
    // A pending command that adds a want is left out, one that releases a want counts as its answer.
    let mut record = alone.clone();
    record.commands = vec![want(A, PARAMETERS, HOST, Policy::Required), release(P, HOST)];
    assert!(table.gate_open(&record, &parts));
    // With the policy the wants the table holds give its parent.
    let mut optional = Table::default();
    apply(&mut optional, &parts, vec![want(P, PARAMETERS, HOST, Policy::Optional), want(A, PARAMETERS, HOST, Policy::Required)], Vec::new());
    let mut record = Record::arrivals(2, vec![Arrival::at(P, p), Arrival::at(A, d)]);
    assert!(optional.gate_open(&record, &parts), "the dependency of an optional parent does not gate");
    record.commands = vec![want(P, PARAMETERS, TOOL, Policy::Required)];
    assert!(optional.gate_open(&record, &parts), "a pending command that adds a want on the parent is left out");
    // The reading the text does not take.
    let leaves_out = Table::decode(&table.canonical_bytes(), Rules { gate_leaves_out_core_wants: true, ..Rules::default() }).expect("the table decodes");
    assert!(leaves_out.gate_open(&alone, &parts));
}

#[test]
fn a_parent_that_fails_ends_the_wants_of_its_dependencies() {
    let mut parts = Parts::default();
    let d = parts.part(1, &[]);
    let p = parts.part(2, &[(D, PARAMETERS)]);
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(P, p), Arrival::at(D, d)]);
        apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(P, REASON_UNKNOWN_ASSET)]);
        table
    };
    assert!(wants(&run(Rules::default()), D).is_empty(), "the reading taken: a failed parent has no part in force");
    let kept = run(Rules { core_wants_kept_under_a_failed_parent: true, ..Rules::default() });
    assert_eq!(wants(&kept, D), vec![(Owner::Asset(P), Policy::Required)], "the other reading keeps them until the parent's last want ends");
}

/// Found by reading, shown here: a rule that cannot be built as stated. The
/// design text has a restore under the content at hand post, with the
/// answers, an ordinary arrival for an asset of an answer's closure "that
/// the saved table holds under no want". An entry the save holds failed
/// under a want, which the part at hand of a restored parent newly lists, is
/// held under a want and so receives nothing: the core's want returns it to
/// unavailable, the gate counts that want, and the world never runs its
/// first tick. The batch that also holds an arrival for it opens the gate.
#[test]
fn a_failed_entry_under_a_want_that_a_restored_parent_newly_lists_keeps_the_gate_closed() {
    let mut parts = Parts::default();
    let d = parts.part(1, &[]);
    let f = parts.part(2, &[]);
    let first = parts.part(3, &[(D, PARAMETERS)]);
    let second = parts.part(4, &[(F, PARAMETERS)]);
    let saved = {
        let mut table = Table::default();
        let commands = vec![want(P, PARAMETERS, BASE, Policy::Required), want(F, PARAMETERS, HOST, Policy::Optional)];
        apply(&mut table, &parts, commands, vec![Arrival::at(P, first), Arrival::at(D, d), Arrival::failed(F, REASON_ARTIFACT_GONE)]);
        table.canonical_bytes()
    };
    let mut restored = Table::decode(&saved, Rules::default()).expect("the table decodes");
    restored.awaiting = [D, P].into_iter().collect();
    restored.refresh();
    let answers = vec![Arrival::at(D, d).marked(), Arrival::at(P, second).marked()];
    assert!(!restored.gate_open(&Record::arrivals(9, answers.clone()), &parts), "the gate counts the want the core adds on the failed entry");
    let mut complete = answers;
    complete.push(Arrival::at(F, f));
    assert!(restored.gate_open(&Record::arrivals(9, complete), &parts));
}

/// Found by reading, shown here: a case the design text leaves without a way
/// out. An entry takes another type from "the next want that states
/// another", and a want the core adds "states a type only where it is the
/// dependency's first". So the entry of a dependency that a command first
/// wanted under a wrong type, and that the core alone still wants, is never
/// corrected by its parent: each retry leaves with the wrong type and fails
/// again, until a command wants the dependency under the manifest's type or
/// the entry falls.
#[test]
fn an_entry_failed_for_its_type_that_the_core_alone_wants_keeps_the_wrong_type() {
    let mut parts = Parts::default();
    let p = parts.part(1, &[(D, PARAMETERS)]);
    let mut table = Table::default();
    let commands = vec![want(D, SECOND, HOST, Policy::Optional), want(P, PARAMETERS, BASE, Policy::Required)];
    apply(&mut table, &parts, commands, vec![Arrival::at(P, p), Arrival::failed(D, REASON_ANOTHER_TYPE)]);
    assert_eq!(availability(&table, D), Some(Availability::Failed(REASON_ANOTHER_TYPE)));
    // The maker of the wrong want releases it.
    apply(&mut table, &parts, vec![release(D, HOST)], Vec::new());
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)]);
    // The parent is released and wanted again: the core's want is the new
    // load of the failed entry, and states no type.
    let applied = apply(&mut table, &parts, vec![release(P, BASE), want(P, PARAMETERS, BASE, Policy::Required)], Vec::new());
    assert_eq!(table.entries[&D].asset_type, SECOND);
    assert_eq!(availability(&table, D), Some(Availability::Unavailable));
    assert_eq!(applied.left, vec![(D, SECOND)], "the retry leaves with the type the entry holds");
    // What corrects it: a command's want of the manifest's type, once the failure has arrived again.
    apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(D, REASON_ANOTHER_TYPE)]);
    let applied = apply(&mut table, &parts, vec![want(D, PARAMETERS, TOOL, Policy::Optional)], Vec::new());
    assert_eq!(table.entries[&D].asset_type, PARAMETERS);
    assert_eq!(applied.left, vec![(D, PARAMETERS)]);
}

#[test]
fn an_input_log_round_trips_with_its_types_and_a_reason_outside_the_list_is_refused() {
    let records = vec![
        Record::commands(1, vec![want(A, SECOND, HOST, Policy::Required), Command::Instantiate { asset: P, asset_type: PARAMETERS, policy: Policy::Optional, reload: true }]),
        Record::arrivals(2, vec![Arrival::failed(A, REASON_ANOTHER_TYPE), Arrival::at(P, 7).marked()]),
    ];
    let bytes = encode_log(&records);
    assert_eq!(decode_log(&bytes).expect("the log decodes"), records);
    let unlisted = encode_log(&[Record::arrivals(1, vec![Arrival::failed(A, 99)])]);
    assert!(decode_log(&unlisted).is_err());
    assert_eq!(REASONS.len(), 10);
    assert!(REASONS.iter().all(|listed| reason(listed.code).is_some_and(|found| found.name == listed.name) && reason_named(listed.name).is_some()));
}

impl Parts {
    /// A part of a given length in bytes: what a budget counts.
    fn sized(&mut self, salt: u32, dependencies: &[Dependency], len: usize) -> u64 {
        let bytes = part_bytes(RepulsionTable::SPECIFICATION, dependencies, len - part_len(dependencies, 0), salt);
        assert_eq!(bytes.len(), len);
        let hash = fnv1a(&bytes);
        let part = Part::decode(&bytes).expect("a valid part");
        self.0.insert(hash, Arc::new(Resident { bytes, part }));
        hash
    }
}

#[test]
fn the_candidates_of_a_budget_reject_one_arrival_the_closure_of_one_want_or_the_whole_drain() {
    let mut parts = Parts::default();
    let d = parts.sized(1, &[], 60);
    let a = parts.sized(2, &[], 100);
    let p = parts.sized(3, &[(D, PARAMETERS)], 80);
    let over = REASON_OVER_BUDGET;
    // A table under a budget that wants `A` and `P`, whose part lists `D`,
    // and one record that holds the three arrivals, each naming its root.
    let run = |rules: Rules, budget: u64| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(budget);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional), want(P, PARAMETERS, HOST, Policy::Required)], Vec::new());
        let arrivals = vec![Arrival::at(D, d).for_root(P), Arrival::at(A, a).for_root(A), Arrival::at(P, p).for_root(P)];
        let applied = apply(&mut table, &parts, Vec::new(), arrivals);
        (table, applied)
    };
    // Each arrival alone, in identifier order: `D` stands, `A` would bring
    // the count to 160, `P` brings it to 140.
    let (table, applied) = run(Rules::default(), 150);
    assert_eq!(applied.rejected, vec![(A, a)]);
    assert_eq!(applied.events, vec![Event::Arrived { asset: D, hash: d }, Event::Failed { asset: A, code: over }, Event::Arrived { asset: P, hash: p }]);
    assert_eq!(availability(&table, A), Some(Availability::Failed(over)), "a rejected arrival fails an entry the table does not hold available");
    assert!(applied.failed_required.is_empty(), "the rejected asset is under an optional want");
    // The closure of one want: the group of `A` is decided before the group
    // of `P`, whose two arrivals are rejected together.
    let (table, applied) = run(Rules { budget_unit: BudgetUnit::ClosureOfAWant, ..Rules::default() }, 150);
    assert_eq!(applied.rejected, vec![(D, d), (P, p)]);
    assert_eq!(availability(&table, A), Some(Availability::Available(a)));
    assert_eq!(availability(&table, P), Some(Availability::Failed(over)));
    assert_eq!(applied.failed_required, vec![(D, over), (P, over)], "the step names each required asset that failed");
    assert!(wants(&table, D).is_empty(), "a parent that failed has no part in force: the want it owned ended");
    // The whole drain: the three arrivals are one group.
    let (table, applied) = run(Rules { budget_unit: BudgetUnit::WholeDrain, ..Rules::default() }, 150);
    assert_eq!(applied.rejected, vec![(D, d), (A, a), (P, p)]);
    assert!(table.entries.values().all(|entry| !matches!(entry.availability, Availability::Available(_))));
    // A count equal to the budget is within it.
    let (table, applied) = run(Rules { budget_unit: BudgetUnit::WholeDrain, ..Rules::default() }, 240);
    assert!(applied.rejected.is_empty());
    assert!(table.entries.values().all(|entry| matches!(entry.availability, Availability::Available(_))));
    // The control: the budget only reports, and every arrival is applied.
    let (table, applied) = run(Rules { budget_only_reports: true, ..Rules::default() }, 150);
    assert_eq!(applied.rejected, vec![(A, a)]);
    assert_eq!(availability(&table, A), Some(Availability::Available(a)));
}

#[test]
fn an_arrival_at_another_hash_counts_the_new_size_in_place_of_the_former_one() {
    let mut parts = Parts::default();
    let small = parts.sized(1, &[], 100);
    let large = parts.sized(2, &[], 140);
    let held = |budget: u64| {
        let mut table = Table { budget: Some(budget), ..Table::default() };
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(A, small)]);
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(A, large)]);
        (table, applied)
    };
    let (table, applied) = held(140);
    assert!(applied.rejected.is_empty(), "the new size stands in place of the former one, and a count equal to the budget is within it");
    assert_eq!(availability(&table, A), Some(Availability::Available(large)));
    let (table, applied) = held(139);
    assert_eq!(applied.rejected, vec![(A, large)]);
    assert_eq!(availability(&table, A), Some(Availability::Available(small)), "the entry stays as it was");
    assert!(applied.events.is_empty() && applied.failed_required.is_empty(), "and no event is emitted");
    // An arrival to failed has no size, and an arrival no want names is not
    // applied: neither is tested.
    let mut table = Table { budget: Some(10), ..Table::default() };
    apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional)], Vec::new());
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(A, REASON_ARTIFACT_GONE), Arrival::at(P, large)]);
    assert!(applied.rejected.is_empty());
    assert_eq!(applied.unnamed_arrivals, 1);
}

#[test]
fn what_a_system_does_is_applied_after_the_arrivals_and_the_falls_of_its_tick() {
    let mut parts = Parts::default();
    let a = parts.part(1, &[]);
    let entity = Owner::Entity { slot: 4, generation: 0 };
    let alive = |_: u32, _: u32| true;
    // A want the system makes in the tick of an arrival of its asset: the
    // arrival was applied first and found no want.
    let mut table = Table::default();
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(A, a)]);
    assert_eq!(applied.unnamed_arrivals, 1);
    let applied = table.apply_system(&[want(A, PARAMETERS, entity, Policy::Required)], &alive);
    assert_eq!(availability(&table, A), Some(Availability::Unavailable));
    assert_eq!(applied.left, vec![(A, PARAMETERS)], "the want of a system leaves the domain as the want of a command does");
    assert!(table.blocked(), "and gates from the next tick boundary where it is required");
    // A release by the system, then a want at the tick of the fall: the
    // fall of that tick's input fixed point comes first, and the want is a
    // new load. The want of a command of the same tick's record finds the entry.
    let released = || {
        let mut table = Table::default();
        apply(&mut table, &parts, vec![want(A, PARAMETERS, entity, Policy::Optional)], vec![Arrival::at(A, a)]);
        assert!(table.apply_system(&[release(A, entity)], &alive).left.is_empty());
        assert_eq!(availability(&table, A), Some(Availability::Available(a)), "the entry keeps its place to the next input fixed point");
        table
    };
    let mut by_system = released();
    let applied = apply(&mut by_system, &parts, Vec::new(), Vec::new());
    assert_eq!(applied.fallen, vec![A]);
    let applied = by_system.apply_system(&[want(A, PARAMETERS, entity, Policy::Optional)], &alive);
    assert_eq!(availability(&by_system, A), Some(Availability::Unavailable));
    assert_eq!(applied.left, vec![(A, PARAMETERS)]);
    let mut by_command = released();
    let applied = apply(&mut by_command, &parts, vec![want(A, PARAMETERS, entity, Policy::Optional)], Vec::new());
    assert!(applied.fallen.is_empty() && applied.left.is_empty());
    assert_eq!(availability(&by_command, A), Some(Availability::Available(a)));
    // A despawn the system asks for ends the wants of its entity, and is named.
    let mut table = released();
    apply(&mut table, &parts, vec![want(A, PARAMETERS, entity, Policy::Optional)], Vec::new());
    let applied = table.apply_system(&[Command::Despawn { slot: 4, generation: 0 }], &alive);
    assert_eq!(applied.despawned, vec![(4, 0)]);
    assert!(wants(&table, A).is_empty());
}

#[test]
fn the_gate_of_a_world_that_is_not_gated_reads_no_pending_record() {
    let mut parts = Parts::default();
    let first = parts.part(1, &[]);
    let second = parts.part(2, &[(D, PARAMETERS)]);
    let mut table = Table::default();
    apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(P, first)]);
    assert!(!table.blocked());
    // A parent pending at a part that lists a dependency the table does not
    // hold, with no arrival of it: what no loading capability posts.
    let alone = Record::arrivals(2, vec![Arrival::at(P, second)]);
    let read = Table::decode(&table.canonical_bytes(), Rules { gate_read_on_a_world_that_is_not_gated: true, ..Rules::default() }).expect("the table decodes");
    assert!(table.gate_open(&alone, &parts), "a world that is not gated steps");
    assert!(!read.gate_open(&alone, &parts), "the other reading: the pending record closes the gate before it is applied");
    table.apply(&alone, &parts, &|_, _| true);
    assert_eq!(table.required_unanswered(), vec![D], "the record applied, the dependency gates from the next boundary");
}
