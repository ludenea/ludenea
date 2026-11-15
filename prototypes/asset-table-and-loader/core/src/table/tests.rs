//! The self-checks of the asset table: the rules the design text states, as
//! amended, each applied to the table alone. They hold no oracle.

use super::*;

const D: AssetId = 0x10;
const F: AssetId = 0x18;
const A: AssetId = 0x20;
const P: AssetId = 0x30;
const X: AssetId = 0x40;
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

/// A scripted arrival to a hash that names no size names the one the
/// manifest would give: the length of its part.
fn sized(parts: &Parts, arrivals: Vec<Arrival>) -> Vec<Arrival> {
    let named = |arrival: Arrival| match arrival.outcome {
        Outcome::Hash(hash) if arrival.size == 0 => arrival.sized(parts.get(hash).map_or(0, |resident| resident.bytes.len() as u64)),
        _ => arrival,
    };
    arrivals.into_iter().map(named).collect()
}

fn apply(table: &mut Table, parts: &Parts, commands: Vec<Command>, arrivals: Vec<Arrival>) -> Applied {
    let mut record = Record::arrivals(1, sized(parts, arrivals));
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

#[test]
fn the_wants_of_former_dependencies_end_once_the_arrivals_of_the_batch_are_applied() {
    let mut parts = Parts::default();
    let x = parts.part(1, &[]);
    let changed = parts.part(2, &[]);
    let first = parts.part(3, &[(X, PARAMETERS)]);
    let second = parts.part(4, &[]);
    // The parent's identifier comes before its former dependency's.
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(P, first), Arrival::at(X, x)]);
        apply(&mut table, &parts, Vec::new(), vec![Arrival::at(P, second), Arrival::at(X, changed)]);
        table
    };
    let taken = run(Rules::default());
    assert_eq!(availability(&taken, X), Some(Availability::Available(changed)), "the reading taken: the former dependency takes its new hash");
    assert!(wants(&taken, X).is_empty());
    // The reading not taken, run as a control: the want ends at the parent's
    // own arrival, and the later arrival names an entry no want names.
    let at_the_parent = run(Rules { core_wants_recomputed_at_each_arrival: true, ..Rules::default() });
    assert_eq!(availability(&at_the_parent, X), Some(Availability::Available(x)));
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

/// On an entry failed for the reason of another type, a want the core adds
/// takes the type its parent's part lists, replaces the entry's type and is
/// the new load, whatever other want stands on the entry: a dependency its
/// parent's part lists correctly is not held failed until a command states
/// the right type. The reading not taken is the letter the text had: the
/// core's want states no type there, and each retry leaves with the wrong
/// type and fails again.
#[test]
fn on_an_entry_failed_for_its_type_the_cores_want_takes_the_type_its_parents_part_lists() {
    let mut parts = Parts::default();
    let p = parts.part(1, &[(D, PARAMETERS)]);
    // A command wants `D` under a wrong type and `D` fails for it; then a
    // parent whose part lists `D` under the manifest's type is wanted.
    let failed = |rules: Rules, released: bool| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, vec![want(D, SECOND, HOST, Policy::Optional)], vec![Arrival::failed(D, REASON_ANOTHER_TYPE)]);
        if released {
            apply(&mut table, &parts, vec![release(D, HOST)], Vec::new());
        }
        let applied = apply(&mut table, &parts, vec![want(P, PARAMETERS, BASE, Policy::Required)], vec![Arrival::at(P, p)]);
        (table, applied)
    };
    // The wrong want still stands: the type is replaced all the same, and
    // the want made under the former type stays.
    let (table, applied) = failed(Rules::default(), false);
    assert_eq!(table.entries[&D].asset_type, PARAMETERS);
    assert_eq!(availability(&table, D), Some(Availability::Unavailable), "the core's want is the new load");
    assert_eq!(wants(&table, D), vec![(HOST, Policy::Optional), (Owner::Asset(P), Policy::Required)]);
    assert!(applied.left.contains(&(D, PARAMETERS)), "the want leaves with the type the entry now holds");
    assert!(table.blocked(), "and gates, its parent being required");
    // The entry held no want when the core's want was added: the same.
    let (table, applied) = failed(Rules::default(), true);
    assert_eq!((table.entries[&D].asset_type, availability(&table, D)), (PARAMETERS, Some(Availability::Unavailable)));
    assert!(applied.left.contains(&(D, PARAMETERS)));
    // Not taken: the entry keeps the wrong type, and the retry leaves with it.
    let (mut table, applied) = failed(Rules { core_want_keeps_the_type_of_a_failed_entry: true, ..Rules::default() }, false);
    assert_eq!((table.entries[&D].asset_type, availability(&table, D)), (SECOND, Some(Availability::Unavailable)));
    assert!(applied.left.contains(&(D, SECOND)));
    // What corrects it there: a command's want of the manifest's type, once the failure has arrived again.
    apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(D, REASON_ANOTHER_TYPE)]);
    let applied = apply(&mut table, &parts, vec![want(D, PARAMETERS, TOOL, Policy::Optional)], Vec::new());
    assert_eq!(table.entries[&D].asset_type, PARAMETERS);
    assert_eq!(applied.left, vec![(D, PARAMETERS)]);
    // An entry failed for another reason keeps its type under the core's want.
    let mut other = Table::default();
    apply(&mut other, &parts, vec![want(D, SECOND, HOST, Policy::Optional)], vec![Arrival::failed(D, REASON_ARTIFACT_GONE)]);
    apply(&mut other, &parts, vec![want(P, PARAMETERS, BASE, Policy::Required)], vec![Arrival::at(P, p)]);
    assert_eq!((other.entries[&D].asset_type, availability(&other, D)), (SECOND, Some(Availability::Unavailable)));
    // An entry that fails for its type while a want of the core already
    // stands on it is not reached: the rule acts when the want is added.
    let mut standing = Table::default();
    apply(&mut standing, &parts, vec![want(D, SECOND, HOST, Policy::Optional), want(P, PARAMETERS, BASE, Policy::Required)], vec![Arrival::at(P, p), Arrival::failed(D, REASON_ANOTHER_TYPE)]);
    assert_eq!((standing.entries[&D].asset_type, availability(&standing, D)), (SECOND, Some(Availability::Failed(REASON_ANOTHER_TYPE))));
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
    let single = Rules { budget_unit: BudgetUnit::SingleArrival, ..Rules::default() };
    let (table, applied) = run(single, 150);
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
    let (table, applied) = run(Rules { budget_only_reports: true, ..single }, 150);
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

/// The rules under the unit of a rejection the design text takes, the group
/// of a root: the default.
fn proposed() -> Rules {
    Rules { budget_unit: BudgetUnit::DerivedClosure, ..Rules::default() }
}

/// The control of that unit under which the wants a rejected part lists are
/// added before the arrivals are applied.
fn with_the_wants_of_rejected_parts() -> Rules {
    Rules { wants_of_rejected_parts_added: true, ..proposed() }
}

#[test]
fn the_proposed_unit_rejects_the_closure_of_a_root_and_leaves_nothing_of_it() {
    let mut parts = Parts::default();
    let d = parts.sized(1, &[], 60);
    let a = parts.sized(2, &[], 100);
    let p = parts.sized(3, &[(D, PARAMETERS)], 80);
    let over = REASON_OVER_BUDGET;
    // A table under a budget that requires `A` and `P`, whose part lists
    // `D`, and one record that holds the three arrivals. None names a root.
    let run = |rules: Rules, budget: u64| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(budget);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required), want(P, PARAMETERS, HOST, Policy::Required)], Vec::new());
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(D, d), Arrival::at(A, a), Arrival::at(P, p)]);
        (table, applied)
    };
    // The roots are `A` and `P`, both under a required want, tested in the
    // identifier order of the roots: the group of `A` brings the count to
    // 100, and the group of `P`, its own arrival with the arrival of `D`,
    // would bring it to 240.
    let (mut table, applied) = run(proposed(), 150);
    assert_eq!(applied.rejected, vec![(D, d), (P, p)], "the step names every rejected arrival");
    assert_eq!(applied.events, vec![Event::Arrived { asset: A, hash: a }, Event::Failed { asset: P, code: over }]);
    assert_eq!(availability(&table, A), Some(Availability::Available(a)));
    assert_eq!(availability(&table, P), Some(Availability::Failed(over)), "the root of a rejected group fails");
    assert_eq!(availability(&table, D), None, "an asset a rejected part alone lists is named by no want: no entry, no failure, no event");
    assert_eq!(applied.failed_required, vec![(P, over)]);
    assert!(apply(&mut table, &parts, Vec::new(), Vec::new()).fallen.is_empty());
    // The control: the wants the rejected part lists are added before the
    // arrivals. The dependency fails under the want of its rejected parent,
    // is named, loses that want and falls at the next tick.
    let (mut table, applied) = run(with_the_wants_of_rejected_parts(), 150);
    assert_eq!(applied.rejected, vec![(D, d), (P, p)]);
    assert_eq!(applied.events, vec![Event::Failed { asset: D, code: over }, Event::Arrived { asset: A, hash: a }, Event::Failed { asset: P, code: over }]);
    assert_eq!(applied.failed_required, vec![(D, over), (P, over)]);
    assert_eq!(availability(&table, D), Some(Availability::Failed(over)));
    assert!(wants(&table, D).is_empty());
    assert_eq!(apply(&mut table, &parts, Vec::new(), Vec::new()).fallen, vec![D]);
    // A count equal to the budget is within it.
    let (table, applied) = run(proposed(), 240);
    assert!(applied.rejected.is_empty());
    assert!(table.entries.values().all(|entry| matches!(entry.availability, Availability::Available(_))));
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)], "the wants of an accepted part are added");
    // The control: the budget only reports, and every arrival is applied.
    let (table, applied) = run(Rules { budget_only_reports: true, ..proposed() }, 150);
    assert_eq!(applied.rejected, vec![(D, d), (P, p)]);
    assert!(table.entries.values().all(|entry| matches!(entry.availability, Availability::Available(_))));
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)]);
}

#[test]
fn an_arrival_that_two_closures_hold_is_applied_where_one_of_them_is_accepted() {
    let mut parts = Parts::default();
    let d = parts.sized(1, &[], 200);
    let x = parts.sized(2, &[], 900);
    let a = parts.sized(3, &[(D, PARAMETERS), (X, PARAMETERS)], 100);
    let p = parts.sized(4, &[(D, PARAMETERS)], 100);
    let over = REASON_OVER_BUDGET;
    // `A` and `P` both list `D`, and `A` lists `X` too. The group of `A`
    // would bring the count to 1200; the group of `P` brings it to 300.
    let mut table = Table::with_rules(proposed());
    table.budget = Some(1000);
    apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional), want(P, PARAMETERS, HOST, Policy::Required)], Vec::new());
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(D, d), Arrival::at(A, a), Arrival::at(P, p), Arrival::at(X, x)]);
    assert_eq!(applied.rejected, vec![(A, a), (X, x)], "the arrival of `D` is rejected with the group of `A` and accepted with the group of `P`");
    // A rejected arrival that fails its asset does so at its own place in
    // the identifier order, which is the place of its event.
    assert_eq!(applied.events, vec![Event::Arrived { asset: D, hash: d }, Event::Failed { asset: A, code: over }, Event::Arrived { asset: P, hash: p }]);
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)], "the accepted part alone gives its want: the failed root owns none");
    assert_eq!(availability(&table, X), None);
    assert!(applied.failed_required.is_empty(), "the rejected root is under an optional want");
}

#[test]
fn the_room_of_an_entry_that_falls_is_free_for_the_arrivals_of_the_same_record() {
    let mut parts = Parts::default();
    let a = parts.sized(1, &[], 50);
    let p = parts.sized(2, &[], 90);
    // `A` is in force, and its last want ends in the record that wants `P`:
    // it falls at the fixed point that holds the arrival of `P`.
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(100);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(A, a)]);
        apply(&mut table, &parts, vec![release(A, HOST), want(P, PARAMETERS, HOST, Policy::Optional)], Vec::new());
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(P, p)]);
        (table, applied)
    };
    let (table, applied) = run(proposed());
    assert!(applied.rejected.is_empty());
    assert_eq!(applied.fallen, vec![A]);
    assert_eq!(availability(&table, P), Some(Availability::Available(p)));
    // The control, and the three candidates: the count holds the entry that
    // falls, and the arrival is rejected for an asset that leaves the table.
    for rules in [Rules { budget_count_holds_the_falling_entries: true, ..proposed() }, Rules { budget_unit: BudgetUnit::SingleArrival, ..Rules::default() }] {
        let (table, applied) = run(rules);
        assert_eq!(applied.rejected, vec![(P, p)]);
        assert_eq!(applied.fallen, vec![A]);
        assert_eq!(availability(&table, P), Some(Availability::Failed(REASON_OVER_BUDGET)));
    }
    // An entry whose last want ends in the record of the arrival itself does
    // not fall at that fixed point: its room is not free.
    let mut table = Table::with_rules(proposed());
    table.budget = Some(100);
    apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional), want(P, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(A, a)]);
    let applied = apply(&mut table, &parts, vec![release(A, HOST)], vec![Arrival::at(P, p)]);
    assert_eq!(applied.rejected, vec![(P, p)]);
    assert!(applied.fallen.is_empty());
}

/// An entry due to fall that a closure reaches stays counted, whatever its
/// group becomes: the count errs on the side of a rejection. The second half
/// shows what that costs, and it is the text's own choice: a root that does
/// not reach the entry is rejected for the room of an asset that leaves the
/// table at the same fixed point.
#[test]
fn an_entry_due_to_fall_that_a_closure_reaches_stays_counted() {
    let mut parts = Parts::default();
    let d = parts.sized(1, &[], 600);
    let a = parts.sized(2, &[], 500);
    let p = parts.sized(3, &[(D, PARAMETERS)], 500);
    // `D` is in force and its last want ended during the tick before; `P`,
    // whose part lists `D`, is wanted and unanswered, and so is `A` where
    // `beside` is set.
    let run = |budget: u64, beside: bool| {
        let mut table = Table::with_rules(proposed());
        table.budget = Some(budget);
        apply(&mut table, &parts, vec![want(D, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(D, d)]);
        let mut commands = vec![release(D, HOST), want(P, PARAMETERS, HOST, Policy::Required)];
        let mut arrivals = vec![Arrival::at(P, p)];
        if beside {
            commands.push(want(A, PARAMETERS, HOST, Policy::Optional));
            arrivals.push(Arrival::at(A, a));
        }
        apply(&mut table, &parts, commands, Vec::new());
        let applied = apply(&mut table, &parts, Vec::new(), arrivals);
        (table, applied)
    };
    // The closure of `P` reaches `D` through the part of its arrival: the
    // count is 600, and `P` would bring it to 1100. No want is added from
    // the rejected part, so `D` falls at this same fixed point.
    let (table, applied) = run(1000, false);
    assert_eq!(applied.rejected, vec![(P, p)]);
    assert_eq!(applied.fallen, vec![D]);
    assert_eq!(availability(&table, P), Some(Availability::Failed(REASON_OVER_BUDGET)));
    // With room for both, the part is put in force and `D` does not fall.
    let (table, applied) = run(1100, false);
    assert!(applied.rejected.is_empty() && applied.fallen.is_empty());
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)]);
    // `A` reaches nothing but itself and would fit once `D` has left. It is
    // tested against a count that holds `D`, and is rejected too.
    let (table, applied) = run(1000, true);
    assert_eq!(applied.rejected, vec![(A, a), (P, p)]);
    assert_eq!(applied.fallen, vec![D]);
    assert!(table.entries.values().all(|entry| entry.availability == Availability::Failed(REASON_OVER_BUDGET)));
}

#[test]
fn a_rejected_replacement_leaves_its_parent_at_its_former_part_and_no_trace_of_the_new_one() {
    let mut parts = Parts::default();
    let d = parts.sized(1, &[], 600);
    let x = parts.sized(2, &[], 900);
    let first = parts.sized(3, &[(D, PARAMETERS)], 200);
    let second = parts.sized(4, &[(D, PARAMETERS), (X, PARAMETERS)], 200);
    let alone = parts.sized(5, &[], 500);
    let over = REASON_OVER_BUDGET;
    // `P` is in force at a part that lists `D`: the count is 800.
    let held = |rules: Rules, budget: u64| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(budget);
        apply(&mut table, &parts, vec![want(P, PARAMETERS, BASE, Policy::Required)], vec![Arrival::at(D, d), Arrival::at(P, first)]);
        table
    };
    // A replacement whose part lists a new dependency, `X`: the group would
    // bring the count to 1700.
    let replaced = |rules: Rules, budget: u64| {
        let mut table = held(rules, budget);
        let before = table.canonical_bytes();
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(P, second), Arrival::at(X, x)]);
        let unchanged = table.canonical_bytes() == before;
        (table, applied, unchanged)
    };
    let (table, applied, unchanged) = replaced(proposed(), 1000);
    assert_eq!(applied.rejected, vec![(P, second), (X, x)]);
    assert!(unchanged, "the parent stays at its former part, with the wants of that part and no trace of what the new part listed");
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)]);
    assert!(applied.events.is_empty() && applied.failed_required.is_empty() && applied.left.is_empty());
    // The control: the wants the rejected part lists are added before the
    // arrivals. The new dependency fails under that want, is named, loses
    // the want and falls at the next tick.
    let (mut table, applied, unchanged) = replaced(with_the_wants_of_rejected_parts(), 1000);
    assert_eq!(applied.rejected, vec![(P, second), (X, x)]);
    assert!(!unchanged);
    assert_eq!(applied.events, vec![Event::Failed { asset: X, code: over }]);
    assert_eq!(applied.failed_required, vec![(X, over)]);
    assert!(wants(&table, X).is_empty());
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)]);
    assert_eq!(apply(&mut table, &parts, Vec::new(), Vec::new()).fallen, vec![X]);
    // Within the budget, the replacement is put in force with its closure.
    let (table, applied, _) = replaced(proposed(), 1700);
    assert!(applied.rejected.is_empty());
    assert_eq!(availability(&table, P), Some(Availability::Available(second)));
    assert_eq!(wants(&table, X), vec![(Owner::Asset(P), Policy::Required)]);
    // A rejected replacement that no longer lists `D`: the former part is
    // still in force, and `D` keeps the want it gives.
    let mut table = held(proposed(), 1000);
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(P, alone)]);
    assert_eq!(applied.rejected, vec![(P, alone)]);
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)]);
}

#[test]
fn a_closure_is_read_from_the_part_in_force_and_from_the_part_of_the_arrival() {
    let mut parts = Parts::default();
    let small = parts.sized(1, &[], 600);
    let large = parts.sized(2, &[], 900);
    let first = parts.sized(3, &[(D, PARAMETERS)], 200);
    let second = parts.sized(4, &[], 200);
    // `P` is in force at a part that lists `D`. One record replaces `P` by a
    // part that lists nothing, and `D` by a larger part: the arrival of the
    // former dependency stands in the group of `P` through the part in
    // force, and the group would bring the count from 800 to 1100.
    let run = |budget: u64| {
        let mut table = Table::with_rules(proposed());
        table.budget = Some(budget);
        apply(&mut table, &parts, vec![want(P, PARAMETERS, BASE, Policy::Required)], vec![Arrival::at(D, small), Arrival::at(P, first)]);
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(D, large), Arrival::at(P, second)]);
        (table, applied)
    };
    let (table, applied) = run(1000);
    assert_eq!(applied.rejected, vec![(D, large), (P, second)]);
    assert!(applied.events.is_empty());
    assert_eq!(availability(&table, D), Some(Availability::Available(small)));
    assert_eq!(wants(&table, D), vec![(Owner::Asset(P), Policy::Required)], "the former part still needs its dependency");
    // Accepted, the arrival of the former dependency is applied, with its
    // event, before that dependency loses its want.
    let (table, applied) = run(1100);
    assert!(applied.rejected.is_empty());
    assert_eq!(applied.events, vec![Event::HashChanged { asset: D, from: small, to: large }, Event::HashChanged { asset: P, from: first, to: second }]);
    assert!(wants(&table, D).is_empty());
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

// ---------------------------------------------------------------------------
// The readings taken on the same-harness stage, each with the reading not
// taken beside it
// ---------------------------------------------------------------------------

#[test]
fn a_system_is_the_maker_of_its_want_between_contributors_and_command_sources() {
    let parts = Parts::default();
    let spawner = Owner::World(Maker::System(SystemName::new("logic/spawner")));
    let cleaner = Owner::World(Maker::System(SystemName::new("logic/cleaner")));
    let mut table = Table::default();
    let commands = vec![
        want(A, PARAMETERS, HOST, Policy::Optional),
        want(A, PARAMETERS, spawner, Policy::Optional),
        want(A, PARAMETERS, cleaner, Policy::Required),
        want(A, PARAMETERS, BASE, Policy::Optional),
    ];
    apply(&mut table, &parts, commands, Vec::new());
    // Contributors first, systems next by their stable name, command sources after.
    assert_eq!(wants(&table, A), vec![(BASE, Policy::Optional), (cleaner, Policy::Required), (spawner, Policy::Optional), (HOST, Policy::Optional)]);
    // Two systems hold two wants: the release of one leaves the other, and
    // neither is the want of the contributor.
    apply(&mut table, &parts, vec![release(A, cleaner)], Vec::new());
    assert_eq!(wants(&table, A), vec![(BASE, Policy::Optional), (spawner, Policy::Optional), (HOST, Policy::Optional)]);
    // The canonical bytes carry the kind and the stable name, and read back.
    let bytes = table.canonical_bytes();
    assert!(bytes.windows(13).any(|window| window == b"logic/spawner"));
    let read = Table::decode(&bytes, Rules::default()).expect("the table decodes");
    assert_eq!(read.entries[&A].wants, table.entries[&A].wants);
    assert_eq!(read.canonical_bytes(), bytes);
    let record = Record::commands(3, vec![want(A, PARAMETERS, spawner, Policy::Required), release(A, cleaner)]);
    assert_eq!(decode_log(&encode_log(std::slice::from_ref(&record))).expect("the log decodes"), vec![record]);
    // Under the reading that orders makers by their numeric identity, a
    // system, which has none, stands after them.
    let mut by_identity = Table::with_rules(Rules { makers_by_identity_alone: true, ..Rules::default() });
    apply(&mut by_identity, &parts, vec![want(A, PARAMETERS, spawner, Policy::Optional), want(A, PARAMETERS, BASE, Policy::Optional), want(A, PARAMETERS, HOST, Policy::Optional)], Vec::new());
    assert_eq!(wants(&by_identity, A), vec![(HOST, Policy::Optional), (BASE, Policy::Optional), (spawner, Policy::Optional)]);
}

#[test]
fn an_arrival_names_its_size_and_the_entry_holds_it_in_the_canonical_bytes() {
    let mut parts = Parts::default();
    let first = parts.sized(1, &[], 100);
    let second = parts.sized(2, &[], 140);
    let mut table = Table::default();
    apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(A, first).sized(321)]);
    assert_eq!(table.entries[&A].size, 321, "the entry holds the size its arrival named, and not the length of the part");
    // The bytes of the table hold it, and a table read back holds it.
    let bytes = table.canonical_bytes();
    let read = Table::decode(&bytes, Rules::default()).expect("the table decodes");
    assert_eq!((read.entries[&A].size, read.canonical_bytes() == bytes), (321, true));
    let mut other = Table::default();
    apply(&mut other, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(A, first).sized(322)]);
    assert_ne!(other.canonical_bytes(), bytes, "two tables that differ in the size of one entry differ in their bytes");
    // The control: the bytes as they stood before an entry held a size.
    let without = |table: &Table| Table { rules: Rules { size_left_out_of_the_bytes: true, ..Rules::default() }, ..table.clone() }.canonical_bytes();
    assert_eq!(without(&table), without(&other));
    assert_eq!(without(&table).len() + 8, bytes.len());
    // The bytes of a record hold it, and a failure names none.
    let record = Record::arrivals(2, vec![Arrival::at(A, first).sized(321), Arrival::failed(P, REASON_ARTIFACT_GONE).sized(9)]);
    assert_eq!(record.arrivals[1].size, 0);
    let log = encode_log(std::slice::from_ref(&record));
    assert_eq!(decode_log(&log).expect("the log decodes"), vec![record]);
    assert_ne!(log, encode_log(&[Record::arrivals(2, vec![Arrival::at(A, first).sized(320), Arrival::failed(P, REASON_ARTIFACT_GONE)])]));
    // An arrival at the hash in force leaves the entry as it was, its size
    // included; an arrival at another hash brings its own; a failure leaves
    // none.
    apply(&mut table, &parts, Vec::new(), vec![Arrival::at(A, first).sized(999)]);
    assert_eq!(table.entries[&A].size, 321);
    let mut other_reading = Table { rules: Rules { arrival_at_the_hash_in_force_brings_its_size: true, ..Rules::default() }, ..table.clone() };
    let applied = apply(&mut other_reading, &parts, Vec::new(), vec![Arrival::at(A, first).sized(999)]);
    assert_eq!((other_reading.entries[&A].size, applied.events.len()), (999, 0), "the other reading: the entry takes the size named, with no event");
    apply(&mut table, &parts, Vec::new(), vec![Arrival::at(A, second).sized(555)]);
    assert_eq!(table.entries[&A].size, 555);
    apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(A, REASON_ARTIFACT_GONE)]);
    assert_eq!(table.entries[&A].size, 0);
}

#[test]
fn the_count_of_a_budget_reads_the_size_an_entry_holds_and_not_the_length_of_its_part() {
    let mut parts = Parts::default();
    let a = parts.sized(1, &[], 100);
    let p = parts.sized(2, &[], 100);
    // `A` is put in force by an arrival that names 600 for a part of 100
    // bytes, as a part with a prepared form built once would be; `P` names 500.
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(1000);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required), want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(A, a).sized(600)]);
        apply(&mut table, &parts, Vec::new(), vec![Arrival::at(P, p).sized(500)])
    };
    assert_eq!(run(Rules::default()).rejected, vec![(P, p)], "600 and 500 stand over a budget of 1000");
    assert!(run(Rules { budget_counts_the_length_of_the_part: true, ..Rules::default() }).rejected.is_empty(), "the stand-in read before: two parts of 100 bytes");
}

#[test]
fn the_groups_of_the_roots_that_hold_a_required_want_are_tested_before_the_others() {
    let mut parts = Parts::default();
    let d = parts.sized(1, &[], 60);
    let a = parts.sized(2, &[], 100);
    let p = parts.sized(3, &[(D, PARAMETERS)], 80);
    let over = REASON_OVER_BUDGET;
    // `A`, optional, stands before `P`, required, in identifier order: each
    // closure fits alone under 150, and the two together do not.
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(150);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional), want(P, PARAMETERS, HOST, Policy::Required)], Vec::new());
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(D, d), Arrival::at(A, a), Arrival::at(P, p)]);
        (table, applied)
    };
    let (table, applied) = run(Rules::default());
    assert_eq!(applied.rejected, vec![(A, a)], "the required closure is tested first and takes the room");
    assert_eq!(availability(&table, P), Some(Availability::Available(p)));
    assert_eq!(availability(&table, A), Some(Availability::Failed(over)));
    assert!(applied.failed_required.is_empty());
    // Not taken: in identifier order alone the optional closure takes the room of the required one.
    let (table, applied) = run(Rules { budget_groups_in_identifier_order_alone: true, ..Rules::default() });
    assert_eq!(applied.rejected, vec![(D, d), (P, p)]);
    assert_eq!(availability(&table, A), Some(Availability::Available(a)));
    assert_eq!(applied.failed_required, vec![(P, over)]);
    // A root holds a required want whichever maker made it, the core
    // included. `F`, required, is in force at a part that lists `P`, which a
    // command wants as optional: `P` is a root, and required through the
    // want of its parent. One record brings a replacement of `F` that does
    // not fit, the first load of `P`, and `A`, an optional root that stands
    // before `P`; `P` and `A` each fit alone and not together.
    let first = parts.sized(4, &[(P, PARAMETERS)], 100);
    let second = parts.sized(5, &[(P, PARAMETERS)], 1000);
    let leaf = parts.sized(6, &[], 80);
    let mut table = Table { budget: Some(250), ..Table::default() };
    let commands = vec![want(F, PARAMETERS, HOST, Policy::Required), want(P, PARAMETERS, TOOL, Policy::Optional), want(A, PARAMETERS, HOST, Policy::Optional)];
    apply(&mut table, &parts, commands, vec![Arrival::at(F, first)]);
    assert_eq!(wants(&table, P), vec![(TOOL, Policy::Optional), (Owner::Asset(F), Policy::Required)]);
    let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(F, second), Arrival::at(A, a), Arrival::at(P, leaf)]);
    assert_eq!(applied.rejected, vec![(F, second), (A, a)], "the group of `P` is tested with the required ones, before the group of `A`");
    assert_eq!(availability(&table, P), Some(Availability::Available(leaf)));
    assert_eq!(availability(&table, F), Some(Availability::Available(first)));
}

#[test]
fn an_entry_that_fails_at_the_same_fixed_point_is_left_out_of_the_count() {
    let mut parts = Parts::default();
    let a = parts.sized(1, &[], 60);
    let p = parts.sized(2, &[], 90);
    // `A` is in force under a want; one record fails it and brings `P`.
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(100);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional), want(P, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(A, a)]);
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::failed(A, REASON_UNKNOWN_ASSET), Arrival::at(P, p)]);
        (table, applied)
    };
    let (table, applied) = run(Rules::default());
    assert!(applied.rejected.is_empty(), "the room of the entry that fails is free for the arrival of the same record");
    assert_eq!(availability(&table, P), Some(Availability::Available(p)));
    let (table, applied) = run(Rules { budget_count_holds_the_failing_entries: true, ..Rules::default() });
    assert_eq!(applied.rejected, vec![(P, p)]);
    assert_eq!(availability(&table, P), Some(Availability::Failed(REASON_OVER_BUDGET)));
}

#[test]
fn a_group_that_does_not_raise_the_count_is_accepted_whatever_the_count() {
    let mut parts = Parts::default();
    let large = parts.sized(1, &[], 900);
    let small = parts.sized(2, &[], 700);
    let p = parts.sized(3, &[], 50);
    // The count stands at 900 over a budget lowered to 500: the replacement
    // that brings it to 700 is still over the budget.
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(1000);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required), want(P, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(A, large)]);
        table.budget = Some(500);
        let applied = apply(&mut table, &parts, Vec::new(), vec![Arrival::at(A, small), Arrival::at(P, p)]);
        (table, applied)
    };
    let (table, applied) = run(Rules::default());
    assert_eq!(applied.rejected, vec![(P, p)], "the group that raises the count is rejected as before");
    assert_eq!(availability(&table, A), Some(Availability::Available(small)), "the world takes the replacement that brings its count down");
    let (table, applied) = run(Rules { budget_tests_a_group_that_does_not_raise_the_count: true, ..Rules::default() });
    assert_eq!(applied.rejected, vec![(A, small), (P, p)]);
    assert_eq!(availability(&table, A), Some(Availability::Available(large)));
}

/// A table gated on `D`, the dependency of a required parent `P` in force.
/// Returns it with a second content of `P`, which lists nothing.
fn gated_on_a_dependency(parts: &mut Parts, rules: Rules) -> (Table, u64) {
    let first = parts.part(1, &[(D, PARAMETERS)]);
    let second = parts.part(2, &[]);
    let mut table = Table::with_rules(rules);
    apply(&mut table, parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(P, first)]);
    assert_eq!(table.required_unanswered(), vec![D]);
    (table, second)
}

#[test]
fn the_gate_does_not_see_the_wants_that_a_pending_element_ends_through_the_recomputation() {
    let copying = Rules { gate_copies_the_table: true, ..Rules::default() };
    for (rules, sees) in [(Rules::default(), false), (copying, true), (Rules { required_set_not_kept: true, ..Rules::default() }, false)] {
        let mut parts = Parts::default();
        let (table, second) = gated_on_a_dependency(&mut parts, rules);
        let d = parts.part(3, &[]);
        // A pending release of the parent's last required want, a pending
        // failure of the parent, a pending replacement whose part no longer
        // lists the dependency: each leaves, once applied, no required want
        // on the dependency.
        let release_of_the_parent = Record::commands(2, vec![release(P, HOST)]);
        let failure_of_the_parent = Record::arrivals(2, vec![Arrival::failed(P, REASON_UNKNOWN_ASSET)]);
        let replacement = Record::arrivals(2, vec![Arrival::at(P, second)]);
        for pending in [&release_of_the_parent, &failure_of_the_parent, &replacement] {
            assert_eq!(table.gate_open(pending, &parts), sees, "the gate stays closed on the dependency until its own answer is pending");
            let mut applied = table.clone();
            applied.apply(pending, &parts, &|_, _| true);
            assert!(!applied.blocked(), "though the record, once applied, leaves no required want on it");
        }
        // Its own answer pending, the gate opens.
        let mut with_its_answer = replacement.clone();
        with_its_answer.arrivals.push(Arrival::at(D, d));
        with_its_answer.arrivals.sort_by_key(|arrival| arrival.asset);
        assert!(table.gate_open(&with_its_answer, &parts));
        assert_eq!(table.gate(&replacement, &parts), if sees { Vec::new() } else { vec![D] });
    }
}

#[test]
fn a_pending_release_answers_a_required_asset_only_where_it_ends_its_last_required_want() {
    let parts = Parts::default();
    let any_release = Rules { gate_takes_a_pending_release_as_an_answer: true, ..Rules::default() };
    let sees_policies = Rules { gate_sees_pending_policy_changes: true, ..Rules::default() };
    let gated = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required), want(A, PARAMETERS, TOOL, Policy::Required), want(A, PARAMETERS, BASE, Policy::Optional)], Vec::new());
        table
    };
    let one = Record::commands(2, vec![release(A, HOST)]);
    let both = Record::commands(2, vec![release(A, HOST), release(A, TOOL)]);
    for rules in [Rules::default(), Rules { gate_copies_the_table: true, ..Rules::default() }] {
        assert_eq!(gated(rules).gate(&one, &parts), vec![A], "a required want is left on the asset");
        assert!(gated(rules).gate_open(&both, &parts), "no required want is left: the optional one does not gate");
    }
    assert!(gated(any_release).gate_open(&one, &parts), "the control: any pending release is an answer");
    // A pending command that changes a required want to optional is not
    // seen: it is neither a pending arrival nor a pending release.
    let lowered = Record::commands(2, vec![release(A, HOST), want(A, PARAMETERS, TOOL, Policy::Optional)]);
    assert_eq!(gated(Rules::default()).gate(&lowered, &parts), vec![A]);
    assert!(gated(sees_policies).gate_open(&lowered, &parts), "the control sees the change of policy");
    let mut applied = gated(Rules::default());
    applied.apply(&lowered, &parts, &|_, _| true);
    assert!(!applied.blocked(), "the record applied, the asset is under optional wants alone");
}

#[test]
fn the_gate_of_a_restore_reads_a_count_and_a_want_the_core_would_add_is_followed_through_the_pending_parts() {
    let mut parts = Parts::default();
    let f = parts.part(1, &[(P, PARAMETERS)]);
    let p = parts.part(2, &[(D, PARAMETERS)]);
    let d = parts.part(3, &[]);
    let a = parts.part(4, &[]);
    // A restored table that awaits the answers of `A`, under a want, and of
    // `X`, an entry with no want: the count is two.
    let mut saved = Table::default();
    apply(&mut saved, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional), want(X, PARAMETERS, HOST, Policy::Optional)], vec![Arrival::at(A, a), Arrival::at(X, a)]);
    apply(&mut saved, &parts, vec![release(X, HOST)], Vec::new());
    let mut restored = Table::decode(&saved.canonical_bytes(), Rules::default()).expect("the table decodes");
    restored.awaiting = BTreeSet::from([A, X]);
    assert!(restored.blocked(), "a restored world is gated on its answers whatever its state holds");
    let one = Record::arrivals(5, vec![Arrival::at(A, a).marked()]);
    assert_eq!(restored.gate(&one, &parts), vec![X], "an entry that holds no want is awaited as the others are");
    let both = Record::arrivals(5, vec![Arrival::at(A, a).marked(), Arrival::at(X, a).marked()]);
    assert!(restored.gate_open(&both, &parts));
    // A world gated on `F`, whose pending part lists `P`, whose pending part
    // lists `D`, of which no arrival is pending: the want the core would add
    // on `P` gives the part of `P` its policy.
    let gated = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, vec![want(F, PARAMETERS, HOST, Policy::Required)], Vec::new());
        table
    };
    let incomplete = Record::arrivals(2, vec![Arrival::at(F, f), Arrival::at(P, p)]);
    assert_eq!(gated(Rules::default()).gate(&incomplete, &parts), vec![D]);
    assert_eq!(gated(Rules { gate_copies_the_table: true, ..Rules::default() }).gate(&incomplete, &parts), vec![D]);
    let one_level = Rules { gate_counts_core_wants_from_the_tables_wants_alone: true, ..Rules::default() };
    assert!(gated(one_level).gate_open(&incomplete, &parts), "the other reading: the table holds no want on `P`, whose part adds nothing");
    let complete = Record::arrivals(2, vec![Arrival::at(D, d), Arrival::at(F, f), Arrival::at(P, p)]);
    assert!(gated(Rules::default()).gate_open(&complete, &parts));
}

#[test]
fn the_incremental_gate_does_not_apply_the_budget_to_a_pending_batch() {
    let mut parts = Parts::default();
    let d = parts.sized(1, &[], 600);
    let p = parts.sized(2, &[(D, PARAMETERS)], 500);
    // A world gated on `P`, whose closure stands over the budget.
    let gated = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(1000);
        apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], Vec::new());
        table
    };
    let copying = Rules { gate_copies_the_table: true, ..Rules::default() };
    let whole = Record::arrivals(2, sized(&parts, vec![Arrival::at(D, d), Arrival::at(P, p)]));
    assert!(gated(Rules::default()).gate_open(&whole, &parts), "a complete batch opens the gate, rejected or not");
    assert!(gated(copying).gate_open(&whole, &parts));
    let mut applied = gated(Rules::default());
    assert_eq!(applied.apply(&whole, &parts, &|_, _| true).rejected, vec![(D, d), (P, p)]);
    assert!(!applied.blocked(), "a rejected first load is a failure, and a failure is an answer");
    // The two forms part on a parent pending without its dependency, where
    // its own arrival stands over the budget: applied, the parent is
    // rejected and fails, and the wants of its part are not added. The copy
    // sees it; the incremental form, which derives no group, waits for the
    // dependency.
    let over = Record::arrivals(2, vec![Arrival::at(P, p).sized(1001)]);
    assert!(gated(copying).gate_open(&over, &parts));
    assert_eq!(gated(Rules::default()).gate(&over, &parts), vec![D]);
}

#[test]
fn the_query_of_a_restore_gives_the_wants_that_stand_on_each_entry_not_held_available() {
    let mut parts = Parts::default();
    let p = parts.part(1, &[(D, PARAMETERS)]);
    let mut table = Table::default();
    apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required), want(A, PARAMETERS, TOOL, Policy::Optional)], vec![Arrival::at(P, p), Arrival::failed(D, REASON_ARTIFACT_GONE)]);
    assert_eq!(table.standing_wants(), vec![(D, vec![(Owner::Asset(P), Policy::Required)]), (A, vec![(TOOL, Policy::Optional)])]);
}

// --- the rules the scenarios of the stand-in stage turn on ---

#[test]
fn a_world_event_sets_the_budget_the_arrivals_of_its_record_are_tested_against() {
    let mut parts = Parts::default();
    let a = parts.sized(1, &[], 100);
    let record = |parts: &Parts| {
        let mut record = Record::arrivals(2, sized(parts, vec![Arrival::at(A, a)]));
        record.events = vec![WorldEvent::Budget(80)];
        record
    };
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(150);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Optional)], Vec::new());
        let applied = table.apply(&record(&parts), &parts, &|_, _| true);
        (table.budget, table.entries[&A].availability, applied.rejected)
    };
    // The event changes the configuration at the boundary of its tick.
    assert_eq!(run(Rules::default()), (Some(80), Availability::Failed(REASON_OVER_BUDGET), vec![(A, a)]));
    // The control: the budget a record changes is in force from the tick after.
    let late = Rules { budget_event_in_force_from_the_next_tick: true, ..Rules::default() };
    assert_eq!(run(late), (Some(80), Availability::Available(a), Vec::new()));
    // A log holds the event; under the control that leaves the size out of
    // the canonical bytes, an arrival read back from a log names none.
    let record = record(&parts);
    assert_eq!(decode_log(&encode_log(std::slice::from_ref(&record))), Ok(vec![record.clone()]));
    let unsized_bytes = Rules { size_left_out_of_the_bytes: true, ..Rules::default() };
    let read_back = decode_log_under(&encode_log_under(std::slice::from_ref(&record), &unsized_bytes), &unsized_bytes).expect("the log decodes");
    assert_eq!((read_back[0].arrivals[0].size, &read_back[0].events), (0, &record.events));
}

#[test]
fn a_root_holds_a_required_want_where_it_stands_in_the_required_set_once_the_commands_are_applied() {
    let mut parts = Parts::default();
    let a = parts.sized(1, &[], 100);
    let p = parts.sized(2, &[], 100);
    // Two roots that each fit alone. The record of their arrivals makes the
    // first optional and the second required.
    let run = |rules: Rules| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(150);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required), want(P, PARAMETERS, HOST, Policy::Optional)], Vec::new());
        let commands = vec![want(A, PARAMETERS, HOST, Policy::Optional), want(P, PARAMETERS, HOST, Policy::Required)];
        apply(&mut table, &parts, commands, vec![Arrival::at(A, a), Arrival::at(P, p)]).rejected
    };
    assert_eq!(run(Rules::default()), vec![(A, a)]);
    assert_eq!(run(Rules { budget_required_roots_read_before_the_commands: true, ..Rules::default() }), vec![(P, p)]);
    assert_eq!(run(Rules { budget_groups_in_identifier_order_alone: true, ..Rules::default() }), vec![(P, p)]);
}

#[test]
fn an_entry_in_force_with_no_want_stays_counted_though_the_record_holds_its_failure() {
    let mut parts = Parts::default();
    let a = parts.sized(1, &[], 60);
    let x = parts.sized(2, &[], 60);
    let larger = parts.sized(3, &[], 100);
    let run = |rules: Rules, released: bool| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(130);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, BASE, Policy::Required), want(X, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(A, a), Arrival::at(X, x)]);
        let commands = if released { vec![release(X, HOST)] } else { Vec::new() };
        let applied = apply(&mut table, &parts, commands, vec![Arrival::at(A, larger), Arrival::failed(X, REASON_UNKNOWN_ASSET)]);
        (applied.rejected, table.entries[&X].availability)
    };
    // The failure fails an entry that holds a want: its room is free.
    assert_eq!(run(Rules::default(), false), (Vec::new(), Availability::Failed(REASON_UNKNOWN_ASSET)));
    // An entry that holds no want: the failure changes nothing, the entry
    // stays counted, and the replacement beside it is rejected.
    assert_eq!(run(Rules::default(), true), (vec![(A, larger)], Availability::Available(x)));
    let freed = Rules { budget_count_leaves_out_every_entry_an_arrival_to_failed_names: true, ..Rules::default() };
    assert_eq!(run(freed, true), (Vec::new(), Availability::Available(x)), "the control ends the fixed point over the budget");
    let held = Rules { budget_count_holds_the_failing_entries: true, ..Rules::default() };
    assert_eq!(run(held, false).0, vec![(A, larger)]);
}

#[test]
fn a_group_that_does_not_raise_the_count_is_accepted_over_a_lowered_budget() {
    let mut parts = Parts::default();
    let a = parts.sized(1, &[], 100);
    let smaller = parts.sized(2, &[], 80);
    let same = parts.sized(3, &[], 100);
    let larger = parts.sized(4, &[], 110);
    // An asset in force, a budget lowered under the count, and a replacement
    // in the record of the lowering. Returns the count of rejected arrivals.
    let run = |rules: Rules, to: u64| {
        let mut table = Table::with_rules(rules);
        table.budget = Some(150);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(A, a)]);
        let mut record = Record::arrivals(2, sized(&parts, vec![Arrival::at(A, to)]));
        record.events = vec![WorldEvent::Budget(50)];
        let rejected = table.apply(&record, &parts, &|_, _| true).rejected.len();
        assert!(matches!(table.entries[&A].availability, Availability::Available(_)), "a lowering takes nothing out of force");
        rejected
    };
    assert_eq!((run(Rules::default(), smaller), run(Rules::default(), same), run(Rules::default(), larger)), (0, 0, 1));
    let lowers = Rules { budget_accepts_over_the_count_only_a_group_that_lowers_it: true, ..Rules::default() };
    assert_eq!((run(lowers, smaller), run(lowers, same)), (0, 1));
    let tested = Rules { budget_tests_a_group_that_does_not_raise_the_count: true, ..Rules::default() };
    assert_eq!(run(tested, smaller), 1);
}

#[test]
fn a_pending_release_answers_only_where_no_required_want_is_left_and_a_pending_change_of_policy_is_not_seen() {
    let parts = Parts::default();
    // An asset the host requires, with a second want of the tool.
    let gated = |rules: Rules, second: Policy| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, vec![want(A, PARAMETERS, HOST, Policy::Required), want(A, PARAMETERS, TOOL, second)], Vec::new());
        table
    };
    let released = Record::commands(2, vec![release(A, HOST)]);
    assert_eq!(gated(Rules::default(), Policy::Required).gate(&released, &parts), vec![A], "a required want is left");
    assert!(gated(Rules::default(), Policy::Optional).gate_open(&released, &parts), "the last required want ends");
    let last_want = Rules { gate_release_answers_where_no_want_is_left: true, ..Rules::default() };
    assert_eq!(gated(last_want, Policy::Optional).gate(&released, &parts), vec![A]);
    let whatever = Rules { gate_takes_a_pending_release_as_an_answer: true, ..Rules::default() };
    assert!(gated(whatever, Policy::Required).gate_open(&released, &parts));
    // A pending change of policy is neither an arrival nor a release.
    let made_optional = Record::commands(2, vec![want(A, PARAMETERS, HOST, Policy::Optional)]);
    assert_eq!(gated(Rules::default(), Policy::Optional).gate(&made_optional, &parts), vec![A]);
    let sees = Rules { gate_sees_pending_policy_changes: true, ..Rules::default() };
    assert!(gated(sees, Policy::Optional).gate_open(&made_optional, &parts));
    // The copy of the table takes it too, under the same control, and sees
    // the wants its recomputation ends.
    let copies = Rules { gate_copies_the_table: true, ..sees };
    assert!(gated(copies, Policy::Optional).gate_open(&made_optional, &parts));
    assert_eq!(gated(Rules { gate_copies_the_table: true, ..Rules::default() }, Policy::Optional).gate(&made_optional, &parts), vec![A]);
}

#[test]
fn the_wants_of_the_first_step_start_from_an_asset_that_holds_a_want_once_the_commands_are_applied() {
    let mut parts = Parts::default();
    let d = parts.part(1, &[]);
    let first = parts.part(2, &[]);
    let second = parts.part(3, &[(D, PARAMETERS)]);
    // A parent in force, released, then replaced by a part that lists a
    // dependency, in a record that wants it again or not.
    let run = |rules: Rules, wanted_again: bool| {
        let mut table = Table::with_rules(rules);
        apply(&mut table, &parts, vec![want(P, PARAMETERS, HOST, Policy::Required)], vec![Arrival::at(P, first)]);
        apply(&mut table, &parts, vec![release(P, HOST)], Vec::new());
        let commands = if wanted_again { vec![want(P, PARAMETERS, TOOL, Policy::Optional)] } else { Vec::new() };
        apply(&mut table, &parts, commands, vec![Arrival::at(D, d), Arrival::at(P, second)]);
        let availability = |asset: AssetId| table.entries.get(&asset).map(|entry| entry.availability);
        (availability(P), availability(D))
    };
    // No want names the parent: its part adds nothing, neither arrival is
    // applied, and the parent falls.
    assert_eq!(run(Rules::default(), false), (None, None));
    // Wanted again by a command of the record: the first step starts from it.
    assert_eq!(run(Rules::default(), true), (Some(Availability::Available(second)), Some(Availability::Available(d))));
    let before = Rules { first_step_starts_from_the_wants_held_before_the_commands: true, ..Rules::default() };
    assert_eq!(run(before, true), (Some(Availability::Available(second)), Some(Availability::Unavailable)));
    let every = Rules { first_step_follows_every_arriving_part: true, ..Rules::default() };
    assert_eq!(run(every, false), (None, Some(Availability::Available(d))));
}

#[test]
fn the_admission_check_refuses_a_valid_part_whose_first_parameter_is_over_its_bound() {
    let check = AdmissionCheck { bound: 96.0 };
    let part = |strength: f32| Part::decode(&part_bytes(RepulsionTable { strength, radius: 8.0 }, &[], 8, 0)).expect("the bound stands inside what the validator accepts");
    assert!(check.admits(&part(64.0)));
    assert!(check.admits(&part(96.0)), "a first parameter equal to the bound is not greater than it");
    assert!(!check.admits(&part(96.5)));
    // The reason of a refusal on a first load, by the name the sheets give it.
    assert_eq!(reason_named("not-admitted").map(|reason| reason.code), Some(REASON_ADMISSION_REFUSED));
}
