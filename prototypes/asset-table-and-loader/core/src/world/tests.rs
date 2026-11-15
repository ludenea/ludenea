//! The self-checks of the stand-in world: the restores and what leaves the
//! domain, as the design text states them once amended. They hold no oracle.

use std::collections::BTreeMap;

use reference_workload::fnv1a;

use super::*;
use crate::table::{Dependency, Outcome, Part, REASON_UNKNOWN_ASSET, part_bytes};

const A: AssetId = 0x1000_0000_0000_0000_0000_0000_0000_0011;
const B: AssetId = 0x2000_0000_0000_0000_0000_0000_0000_0021;
const T: AssetId = 0x3000_0000_0000_0000_0000_0000_0000_0031;
const U: AssetId = 0x4000_0000_0000_0000_0000_0000_0000_0041;
const K: AssetId = 0x5000_0000_0000_0000_0000_0000_0000_0051;
const HOST: Owner = Owner::World(Maker::Source(3));
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

fn config(asset_types: &[AssetType]) -> WorldConfig {
    let mut config = WorldConfig::new(Size::Small, 2, Mode::Identifiers, vec![A, B]);
    config.asset_types = asset_types.to_vec();
    config
}

fn want(asset: AssetId, asset_type: AssetType, policy: Policy) -> Command {
    Command::Want { asset, asset_type, owner: HOST, policy }
}

fn step(world: &mut World, ticks: u32) -> StepResult {
    world.step(ticks, Wait::No).expect("the world does not fail")
}

/// A world saved after tick 4: `A` available under a want, `B` available
/// with no want (its last want ended at tick 4), `U` wanted, required and
/// unanswered, and, where `second` is set, `T` of the second type available.
fn saved(parts: &Arc<Parts>, hashes: (u64, u64, u64), second: bool) -> (Vec<u8>, Vec<u64>) {
    let (a, b, t) = hashes;
    let mut first = Record::arrivals(1, vec![Arrival::at(A, a), Arrival::at(B, b)]);
    first.commands = vec![want(A, PARAMETERS, Policy::Required), want(B, PARAMETERS, Policy::Optional)];
    if second {
        first.commands.push(want(T, SECOND, Policy::Optional));
        first.arrivals.push(Arrival::at(T, t));
    }
    let release = Record::commands(4, vec![Command::Release { asset: B, owner: HOST }, want(U, PARAMETERS, Policy::Required)]);
    let mut world = World::new(config(&[PARAMETERS, SECOND]), vec![first, release], Links::new(parts.clone()));
    assert!(!step(&mut world, 4).gated);
    let save = world.save();
    assert_eq!(world.entry(B), Some((Availability::Available(b), Vec::new())), "the entry keeps its place for one tick boundary");
    // The run that goes on receives the answer to its unanswered want.
    let hash = parts.get(a).map(|_| a).expect("resident");
    world.graph.links.queue.post(&[Arrival::at(U, hash)]);
    assert!(!step(&mut world, 4).gated);
    (save, world.state_hashes())
}

fn parts() -> (Arc<Parts>, (u64, u64, u64)) {
    let mut parts = Parts::default();
    let hashes = (parts.part(1, &[]), parts.part(2, &[]), parts.part(3, &[]));
    (Arc::new(parts), hashes)
}

#[test]
fn a_save_header_holds_the_composition_by_the_names_of_its_types() {
    let (parts, hashes) = parts();
    let (save, _) = saved(&parts, hashes, true);
    let header = save_header(&save).expect("the header reads");
    assert_eq!((header.tick, header.content_set), (4, None));
    assert_eq!(header.asset_types, vec![PARAMETERS, SECOND]);
}

#[test]
fn a_restore_under_the_content_at_hand_answers_an_entry_with_no_want_and_starts_a_new_history_on_an_unanswered_want() {
    let (parts, hashes) = parts();
    let (a, b, _) = hashes;
    let (save, uninterrupted) = saved(&parts, hashes, false);
    let links = Links::new(parts.clone());
    let queue = links.queue.clone();
    let request = Restore::ContentAtHand { dropped: &[], drop_in_silence: false, types_from: None };
    let mut world = World::restore(config(&[PARAMETERS, SECOND]), &save, Vec::new(), links, request).expect("the restore is admitted");
    assert_eq!(world.restore_query(), vec![(A, a), (B, b)], "the entry with no want is answered as the others are");
    assert_eq!(world.unanswered(), vec![(U, PARAMETERS)]);
    assert_eq!(*world.history(), History::Undecided);
    let first = step(&mut world, 1);
    assert!(first.gated && first.ticks_ran == 0);
    assert_eq!(first.unanswered, vec![A, B, U]);
    queue.post(&[Arrival::at(A, a).marked(), Arrival::at(B, b).marked()]);
    let still = step(&mut world, 1);
    assert!(still.gated, "a required want the save held unanswered keeps the gate");
    assert_eq!(still.unanswered, vec![U], "the step names what keeps the gate closed, the pending answers counted");
    queue.post(&[Arrival::at(U, a)]);
    assert!(!step(&mut world, 4).gated);
    // Every answer was at the recorded hash, and still the save held an
    // unanswered want: a new history, recorded by the first event of the tick.
    let named = NewHistory { differed: Vec::new(), unanswered: vec![U], dropped: Vec::new() };
    assert_eq!(*world.history(), History::New(named.clone()));
    assert_eq!(world.events()[..2], [(5, Event::NewHistory(named)), (5, Event::Arrived { asset: U, hash: a })]);
    assert_eq!(world.entry(B), None, "not wanted again, the entry fell at the first tick");
    assert_eq!(world.state_hashes(), uninterrupted[4..], "here the answer is the one the saved run received, which nothing can say");
    // The text as it stood: the saved run continues.
    let mut config = config(&[PARAMETERS, SECOND]);
    config.rules.save_with_an_unanswered_want_continues = true;
    let links = Links::new(parts.clone());
    links.queue.post(&[Arrival::at(A, a).marked(), Arrival::at(B, b).marked(), Arrival::at(U, a)]);
    let request = Restore::ContentAtHand { dropped: &[], drop_in_silence: false, types_from: None };
    let mut continued = World::restore(config, &save, Vec::new(), links, request).expect("the restore is admitted");
    assert!(!step(&mut continued, 4).gated);
    assert_eq!(*continued.history(), History::Continues);
}

#[test]
fn a_step_on_a_gated_world_names_what_keeps_the_gate_closed_the_pending_record_counted() {
    let (parts, hashes) = parts();
    let (a, b, _) = hashes;
    let (save, _) = saved(&parts, hashes, false);
    let restore = |rules: Rules| {
        let mut config = config(&[PARAMETERS, SECOND]);
        config.rules = rules;
        let links = Links::new(parts.clone());
        links.queue.post(&[Arrival::at(A, a).marked(), Arrival::at(B, b).marked()]);
        let request = Restore::ContentAtHand { dropped: &[], drop_in_silence: false, types_from: None };
        World::restore(config, &save, Vec::new(), links, request).expect("the restore is admitted")
    };
    assert_eq!(step(&mut restore(Rules::default()), 1).unanswered, vec![U]);
    // The reading not taken, run as a control: what the state holds unanswered.
    let other = Rules { step_names_what_the_state_holds_unanswered: true, ..Rules::default() };
    assert_eq!(step(&mut restore(other), 1).unanswered, vec![A, B, U]);
}

#[test]
fn an_entry_with_no_want_that_the_first_record_wants_again_is_answered_before_it_falls() {
    let (parts, hashes) = parts();
    let (a, _, t) = hashes;
    let (save, _) = saved(&parts, hashes, false);
    let links = Links::new(parts.clone());
    // The answer of B is at another hash, and the first tick's record wants B again.
    links.queue.post(&[Arrival::at(A, a).marked(), Arrival::at(B, t).marked(), Arrival::at(U, a)]);
    let script = vec![Record::commands(5, vec![want(B, PARAMETERS, Policy::Optional)])];
    let request = Restore::ContentAtHand { dropped: &[], drop_in_silence: false, types_from: None };
    let mut world = World::restore(config(&[PARAMETERS, SECOND]), &save, script, links, request).expect("the restore is admitted");
    assert!(!step(&mut world, 2).gated);
    assert_eq!(world.entry(B), Some((Availability::Available(t), vec![(HOST, Policy::Optional)])));
    let named = NewHistory { differed: vec![B], unanswered: vec![U], dropped: Vec::new() };
    assert_eq!(world.events()[0], (5, Event::NewHistory(named)));
    assert!(!world.events().iter().any(|(_, event)| matches!(event, Event::HashChanged { .. })), "no event on a marked transition to another hash");
    // Not wanted again, the answer at another hash still differed, and the entry falls.
    let links = Links::new(parts.clone());
    links.queue.post(&[Arrival::at(A, a).marked(), Arrival::at(B, t).marked(), Arrival::at(U, a)]);
    let request = Restore::ContentAtHand { dropped: &[], drop_in_silence: false, types_from: None };
    let mut fallen = World::restore(config(&[PARAMETERS, SECOND]), &save, Vec::new(), links, request).expect("the restore is admitted");
    assert!(!step(&mut fallen, 1).gated);
    assert_eq!(fallen.entry(B), None);
    assert!(matches!(fallen.history(), History::New(named) if named.differed == [B]));
}

#[test]
fn a_restore_under_the_recorded_content_confirms_every_available_entry_and_nothing_of_an_unanswered_want() {
    let (parts, hashes) = parts();
    let (a, b, _) = hashes;
    let (save, uninterrupted) = saved(&parts, hashes, false);
    let asked = std::sync::Mutex::new(Vec::new());
    let confirm = |asset: AssetId, hash: u64| -> Result<Arc<Resident>, String> {
        asked.lock().expect("never poisoned").push((asset, hash));
        parts.get(hash).ok_or_else(|| "the part cannot be had".to_string())
    };
    let links = Links::new(parts.clone());
    links.queue.post(&[Arrival::at(U, a)]);
    let request = Restore::Continue { confirm: &confirm, confirm_form: None, dropped: &[] };
    let mut world = World::restore(config(&[PARAMETERS, SECOND]), &save, Vec::new(), links, request).expect("the restore is admitted");
    assert_eq!(*asked.lock().expect("never poisoned"), vec![(A, a), (B, b)], "an entry that holds no want is confirmed as the others are");
    assert_eq!(*world.history(), History::Continues);
    assert_eq!(world.unanswered(), vec![(U, PARAMETERS)], "loaded as any want is");
    assert!(!step(&mut world, 4).gated);
    assert_eq!(world.state_hashes(), uninterrupted[4..]);
    assert!(world.log().iter().all(|record| record.arrivals.iter().all(|arrival| !arrival.marked)), "the confirmation is no element of the record");
    assert!(!world.events().iter().any(|(_, event)| matches!(event, Event::NewHistory(_))));
    // A part that differs or cannot be had refuses the restore, naming the asset and the hash.
    let refuse = |asset: AssetId, hash: u64| -> Result<Arc<Resident>, String> {
        if asset == B { Err("the content names another hash".to_string()) } else { parts.get(hash).ok_or_else(String::new) }
    };
    let request = Restore::Continue { confirm: &refuse, confirm_form: None, dropped: &[] };
    let refused = World::restore(config(&[PARAMETERS, SECOND]), &save, Vec::new(), Links::new(parts.clone()), request).err();
    assert_eq!(refused, Some(Refusal::Content { asset: B, hash: b, reason: "the content names another hash".to_string() }));
}

#[test]
fn a_restore_into_another_composition_reads_the_type_each_entry_holds() {
    let (parts, hashes) = parts();
    let (a, b, _) = hashes;
    let (save, _) = saved(&parts, hashes, true);
    let restore = |asset_types: &[AssetType], dropped: &[AssetType], drop_in_silence: bool| {
        let links = Links::new(parts.clone());
        links.queue.post(&[Arrival::at(A, a).marked(), Arrival::at(B, b).marked(), Arrival::at(U, a)]);
        World::restore(config(asset_types), &save, Vec::new(), links, Restore::ContentAtHand { dropped, drop_in_silence, types_from: None })
    };
    // The type the new composition lacks refuses the restore by its name, unless the request names it as dropped.
    assert_eq!(restore(&[PARAMETERS], &[], false).err(), Some(Refusal::TypeLacking(SECOND)));
    // A type the request names that the new composition has is refused by name.
    assert_eq!(restore(&[PARAMETERS, SECOND], &[SECOND], false).err(), Some(Refusal::TypeNotLacking(SECOND)));
    let mut world = restore(&[PARAMETERS], &[SECOND], false).expect("the request names the type as dropped");
    assert_eq!(world.entry(T), None, "the entries of a dropped type leave the table with their wants");
    assert_eq!(world.restore_query(), vec![(A, a), (B, b)]);
    assert!(!step(&mut world, 1).gated);
    let named = NewHistory { differed: Vec::new(), unanswered: vec![U], dropped: vec![T] };
    assert_eq!(world.events()[0], (5, Event::NewHistory(named)), "one event names what differed and what was dropped");
    // The control: the entry dropped in silence is not named.
    let mut silent = restore(&[PARAMETERS], &[], true).expect("dropped in silence");
    assert!(!step(&mut silent, 1).gated);
    assert!(matches!(silent.history(), History::New(named) if named.dropped.is_empty()));
    // The rule holds as well under the recorded content.
    let confirm = |_: AssetId, hash: u64| parts.get(hash).ok_or_else(String::new);
    let request = Restore::Continue { confirm: &confirm, confirm_form: None, dropped: &[] };
    let refused = World::restore(config(&[PARAMETERS]), &save, Vec::new(), Links::new(parts.clone()), request).err();
    assert_eq!(refused, Some(Refusal::TypeLacking(SECOND)));
    let request = Restore::Continue { confirm: &confirm, confirm_form: None, dropped: &[SECOND] };
    let links = Links::new(parts.clone());
    links.queue.post(&[Arrival::at(U, a)]);
    let mut recorded = World::restore(config(&[PARAMETERS]), &save, Vec::new(), links, request).expect("the request names the type as dropped");
    assert!(!step(&mut recorded, 1).gated);
    let named = NewHistory { differed: Vec::new(), unanswered: Vec::new(), dropped: vec![T] };
    assert_eq!(*recorded.history(), History::New(named), "a restore into another composition always starts a new history");
}

#[test]
fn a_type_is_not_dropped_while_a_kept_entry_lists_a_dependency_of_it() {
    let mut parts = Parts::default();
    let t = parts.part(1, &[]);
    let k = parts.part(2, &[(T, SECOND)]);
    let parts = Arc::new(parts);
    let mut first = Record::arrivals(1, vec![Arrival::at(K, k), Arrival::at(T, t)]);
    first.commands = vec![want(K, PARAMETERS, Policy::Required)];
    let mut world = World::new(config(&[PARAMETERS, SECOND]), vec![first], Links::new(parts.clone()));
    assert!(!step(&mut world, 2).gated);
    assert_eq!(world.asset_type(T), Some(SECOND), "the type the parent's part lists for its dependency");
    let save = world.save();
    let request = Restore::ContentAtHand { dropped: &[SECOND], drop_in_silence: false, types_from: None };
    let refused = World::restore(config(&[PARAMETERS]), &save, Vec::new(), Links::new(parts.clone()), request).err();
    assert_eq!(refused, Some(Refusal::TypeListedByAKeptEntry { kept: PARAMETERS, dropped: SECOND }), "refused, naming both types");
}

#[test]
fn a_marked_answer_that_fails_names_the_asset_and_fails_the_instance() {
    let (parts, hashes) = parts();
    let (a, _, _) = hashes;
    let mut first = Record::arrivals(1, vec![Arrival::at(A, a)]);
    first.commands = vec![Command::Instantiate { asset: A, asset_type: PARAMETERS, policy: Policy::Required, reload: false }];
    let mut world = World::new(config(&[PARAMETERS]), vec![first], Links::new(parts.clone()));
    assert!(!step(&mut world, 2).gated);
    let save = world.save();
    let links = Links::new(parts.clone());
    links.queue.post(&[Arrival::failed(A, REASON_UNKNOWN_ASSET).marked()]);
    let request = Restore::ContentAtHand { dropped: &[], drop_in_silence: false, types_from: None };
    let mut restored = World::restore(config(&[PARAMETERS]), &save, Vec::new(), links, request).expect("the restore is admitted");
    let result = step(&mut restored, 1);
    assert_eq!(result.failed_required, vec![(A, REASON_UNKNOWN_ASSET)], "the step's result names the asset with its reason");
    let named = NewHistory { differed: vec![A], unanswered: Vec::new(), dropped: Vec::new() };
    assert_eq!(restored.events(), [(3, Event::NewHistory(named)), (3, Event::Failed { asset: A, code: REASON_UNKNOWN_ASSET })]);
    assert!(restored.instances()[0].failed);
    assert_eq!(restored.log()[0].arrivals[0].outcome, Outcome::Failure(REASON_UNKNOWN_ASSET));
}

#[test]
fn the_wants_that_leave_the_domain_are_extracted_at_the_tick_that_added_them() {
    let (parts, hashes) = parts();
    let (a, _, _) = hashes;
    let extracted = Arc::new(Mutex::new(Vec::new()));
    let mut links = Links::new(parts.clone());
    let sink = extracted.clone();
    links.extract = Some(Arc::new(move |extraction: Extraction| sink.lock().expect("never poisoned").push(extraction)));
    let queue = links.queue.clone();
    let tool = Owner::World(Maker::Source(5));
    let script = vec![
        Record::commands(1, vec![want(A, PARAMETERS, Policy::Required)]),
        Record::commands(3, vec![Command::Want { asset: A, asset_type: PARAMETERS, owner: tool, policy: Policy::Optional }]),
        Record::commands(4, vec![want(A, PARAMETERS, Policy::Optional)]),
        Record::commands(5, vec![Command::Want { asset: A, asset_type: SECOND, owner: Owner::World(Maker::Source(9)), policy: Policy::Optional }]),
    ];
    let world_config = config(&[PARAMETERS]).wanting(7, &[(B, PARAMETERS, Policy::Optional)]);
    let mut world = World::new(world_config, script, links);
    assert_eq!(world.unanswered(), vec![(B, PARAMETERS)], "the query at the boundary where no tick has ended");
    assert!(!step(&mut world, 1).gated, "a want gates from the next tick boundary");
    assert!(step(&mut world, 1).gated);
    queue.post(&[Arrival::at(A, a)]);
    assert!(!step(&mut world, 4).gated);
    let extracted = extracted.lock().expect("never poisoned");
    let left: Vec<(u32, Vec<(AssetId, AssetType)>)> = extracted.iter().map(|extraction| (extraction.tick, extraction.left.clone())).collect();
    // Tick 1 adds the want of an unanswered asset; tick 2 applies its arrival;
    // tick 3 adds a want of an asset in force, which leaves nothing; tick 4
    // only changes a policy; the want of tick 5 is refused and extracts nothing.
    assert_eq!(left, vec![(1, vec![(A, PARAMETERS)]), (2, Vec::new()), (3, Vec::new()), (4, Vec::new())]);
    let after_the_arrival = &extracted[1].entries;
    assert!(after_the_arrival.contains(&EntryView { asset: A, asset_type: PARAMETERS, availability: Availability::Available(a), wanted: true }));
    assert_eq!(world.refusals(), [(5, 0, Refused::AnotherType)]);
    assert_eq!(world.refused_commands(), 1);
}

#[test]
fn what_the_scripted_system_does_is_no_element_of_a_record_and_a_replay_runs_it_again() {
    let mut parts = Parts::default();
    let a = parts.part(1, &[]);
    let parts = Arc::new(parts);
    let entity = Owner::Entity { slot: 0, generation: 0 };
    let holder = Holder::Entity { slot: 0, generation: 0 };
    let system = |config: &mut WorldConfig| {
        let does = vec![(2, Act::Want { asset: A, asset_type: PARAMETERS, holder, policy: Policy::Required }), (5, Act::Release { asset: A, holder })];
        config.systems = vec![System { name: SystemName::new("prototype/scripted"), contributor: 9, does }];
    };
    let mut links = Links::new(parts.clone());
    let queue = links.queue.clone();
    let extracted = Arc::new(Mutex::new(Vec::new()));
    let sink = extracted.clone();
    links.extract = Some(Arc::new(move |extraction: Extraction| sink.lock().expect("never poisoned").push(extraction)));
    let mut world_config = config(&[PARAMETERS]);
    system(&mut world_config);
    let mut world = World::new(world_config, Vec::new(), links);
    assert!(world.alive(0, 0), "the entity that owns the want is alive");
    assert!(!step(&mut world, 2).gated, "the tick in which the system makes its want runs with the asset unanswered");
    assert_eq!(world.entry(A), Some((Availability::Unavailable, vec![(entity, Policy::Required)])));
    assert!(world.log().is_empty(), "what a system does stands in no input record");
    let left: Vec<(u32, Vec<(AssetId, AssetType)>)> = extracted.lock().expect("never poisoned").iter().map(|extraction| (extraction.tick, extraction.left.clone())).collect();
    assert_eq!(left.last(), Some(&(2, vec![(A, PARAMETERS)])), "the want leaves the domain in the state extracted at the end of its tick");
    let gated = step(&mut world, 1);
    assert!(gated.gated && gated.unanswered == vec![A], "a required want of a system gates from the next tick boundary");
    queue.post(&[Arrival::at(A, a)]);
    assert!(!step(&mut world, 5).gated);
    assert_eq!(world.entry(A), None, "the release of tick 5 left the entry with no want, which fell at tick 6");
    // The replay is handed the log, which holds the one arrival, and the
    // same world, whose system runs again.
    let mut again = config(&[PARAMETERS]);
    system(&mut again);
    again.workers = 1;
    let mut replayed = World::new(again, world.log().to_vec(), Links::new(parts.clone()));
    assert!(!step(&mut replayed, 7).gated);
    assert_eq!(replayed.state_hashes(), world.state_hashes());
    // The same wants by commands of the records of the same ticks give the same states.
    let script = vec![
        Record::commands(2, vec![Command::Want { asset: A, asset_type: PARAMETERS, owner: entity, policy: Policy::Required }]),
        Record::arrivals(3, vec![Arrival::at(A, a)]),
        Record::commands(5, vec![Command::Release { asset: A, owner: entity }]),
    ];
    let mut by_command = World::new(config(&[PARAMETERS]), script, Links::new(parts));
    assert!(!step(&mut by_command, 7).gated);
    assert_eq!(by_command.state_hashes(), world.state_hashes());
}

#[test]
fn each_system_is_the_maker_of_the_want_it_makes_and_two_systems_of_one_contributor_hold_two_wants() {
    let mut parts = Parts::default();
    let a = parts.part(1, &[]);
    let parts = Arc::new(parts);
    let (spawner, cleaner) = (SystemName::new("logic/spawner"), SystemName::new("logic/cleaner"));
    let composition = Owner::World(Maker::Contributor(7));
    let wants = |policy: Policy| Act::Want { asset: A, asset_type: PARAMETERS, holder: Holder::World, policy };
    // Two systems brought by the contributor whose composition wants `A`:
    // both want it at tick 1, and the first releases its want at tick 4.
    let systems = vec![
        System { name: spawner, contributor: 7, does: vec![(1, wants(Policy::Optional)), (4, Act::Release { asset: A, holder: Holder::World })] },
        System { name: cleaner, contributor: 7, does: vec![(1, wants(Policy::Required))] },
    ];
    let configured = |rules: Rules| {
        let mut world_config = config(&[PARAMETERS]).wanting(7, &[(A, PARAMETERS, Policy::Optional)]);
        world_config.systems = systems.clone();
        world_config.rules = rules;
        world_config
    };
    let run = |rules: Rules| {
        let links = Links::new(parts.clone());
        let queue = links.queue.clone();
        let mut world = World::new(configured(rules), Vec::new(), links);
        queue.post(&[Arrival::at(A, a).sized(64)]);
        assert!(!step(&mut world, 2).gated);
        let after_the_wants = world.entry(A).map(|(_, wants)| wants);
        assert!(!step(&mut world, 3).gated);
        (world, after_the_wants)
    };
    let (world, after_the_wants) = run(Rules::default());
    let by = |name: SystemName| Owner::World(Maker::System(name));
    // The composition's want is left as it was, and each system holds its
    // own, between the contributor's and a command source's, by its name.
    assert_eq!(after_the_wants, Some(vec![(composition, Policy::Optional), (by(cleaner), Policy::Required), (by(spawner), Policy::Optional)]));
    assert_eq!(world.entry(A), Some((Availability::Available(a), vec![(composition, Policy::Optional), (by(cleaner), Policy::Required)])), "the release of one leaves the other");
    assert_eq!(world.size(A), Some(64), "the entry holds the size its arrival named");
    assert!(world.log().iter().all(|record| record.commands.is_empty()), "what a system does stands in no input record");
    // A save holds the stable name of a system and the size of an entry.
    let confirm = |_: AssetId, hash: u64| parts.get(hash).ok_or_else(|| "the part cannot be had".to_string());
    let request = Restore::Continue { confirm: &confirm, confirm_form: None, dropped: &[] };
    let restored = World::restore(configured(Rules::default()), &world.save(), Vec::new(), Links::new(parts.clone()), request).expect("the restore is admitted");
    assert_eq!(restored.typed_table_bytes(), world.typed_table_bytes());
    assert_eq!((restored.size(A), restored.entry(A)), (Some(64), world.entry(A)));
    // Not taken: the wants are made under the contributor that brings the
    // systems. The two systems and the composition share one want, whose
    // policy the last to act decides and which the release of one ends.
    let (world, after_the_wants) = run(Rules { system_wants_under_their_contributor: true, ..Rules::default() });
    assert_eq!(after_the_wants, Some(vec![(composition, Policy::Required)]));
    assert_eq!(world.entry(A), None, "the one want released at tick 4, the asset fell at tick 5");
    // The other reading of the order among systems: by their stable names,
    // the cleaner first. Under one maker the last to act decides the policy.
    let by_name = Rules { system_wants_under_their_contributor: true, systems_applied_in_name_order: true, ..Rules::default() };
    assert_eq!(run(by_name).1, Some(vec![(composition, Policy::Optional)]));
    assert_eq!(run(Rules { systems_applied_in_name_order: true, ..Rules::default() }).1, run(Rules::default()).1, "where each system is its own maker the order does not reach the table here");
}

#[test]
fn the_state_is_extracted_at_the_end_of_every_tick_in_which_the_table_changed() {
    let mut parts = Parts::default();
    let a = parts.part(1, &[]);
    let parts = Arc::new(parts);
    let entity = Owner::Entity { slot: 0, generation: 0 };
    // Under the reading in which a despawn of a record ends its entity's
    // wants at the structural change fixed point, the table changes there
    // and nowhere else at tick 2.
    let run = |rules: Rules| {
        let mut first = Record::arrivals(1, vec![Arrival::at(A, a)]);
        first.commands = vec![Command::Want { asset: A, asset_type: PARAMETERS, owner: entity, policy: Policy::Optional }];
        let script = vec![first, Record::commands(2, vec![Command::Despawn { slot: 0, generation: 0 }])];
        let mut links = Links::new(parts.clone());
        let extracted = Arc::new(Mutex::new(Vec::new()));
        let sink = extracted.clone();
        links.extract = Some(Arc::new(move |extraction: Extraction| sink.lock().expect("never poisoned").push(extraction)));
        let mut world_config = config(&[PARAMETERS]);
        world_config.rules = Rules { despawn_ends_wants_at_the_structural_point: true, ..rules };
        let mut world = World::new(world_config, script, links);
        assert!(!step(&mut world, 4).gated);
        assert_eq!(world.entry(A), None, "the asset fell at tick 3");
        let extracted = extracted.lock().expect("never poisoned");
        extracted.iter().map(|extraction| (extraction.tick, extraction.entries.iter().map(|entry| (entry.asset, entry.wanted)).collect::<Vec<_>>())).collect::<Vec<_>>()
    };
    // The availability source reads, at the end of tick 2, that no want
    // names the asset any more: its view is never older than the last tick.
    assert_eq!(run(Rules::default()), vec![(1, vec![(A, true)]), (2, vec![(A, false)]), (3, Vec::new())]);
    // The build as it stood: nothing leaves at tick 2, the record having
    // changed nothing at its input fixed point.
    assert_eq!(run(Rules { extracted_at_the_input_fixed_point: true, ..Rules::default() }), vec![(1, vec![(A, true)]), (3, Vec::new())]);
}

#[test]
fn a_world_event_is_an_element_of_the_log_and_the_header_of_a_save_holds_the_budget_in_force() {
    let (parts, (a, _, _)) = parts();
    // A world created under a budget, which the host lowers between its
    // first tick and its second.
    let run = |rules: Rules| {
        let mut config = config(&[PARAMETERS]);
        config.budget = Some(1_000);
        config.rules = rules;
        let mut world = World::new(config, Vec::new(), Links::new(parts.clone()));
        world.post(want(A, PARAMETERS, Policy::Optional));
        world.graph.links.queue.post(&[Arrival::at(A, a).sized(600)]);
        assert!(!step(&mut world, 1).gated);
        world.post_event(WorldEvent::Budget(500));
        assert!(!step(&mut world, 1).gated);
        assert_eq!(world.size(A), Some(600), "a lowering takes nothing out of force");
        (world.budget(), world.log().to_vec(), world.save())
    };
    let (budget, log, save) = run(Rules::default());
    assert_eq!(budget, Some(500));
    assert_eq!(log[1].events, vec![WorldEvent::Budget(500)], "the event stands in the record of its tick");
    assert_eq!(save_header(&save).expect("a header").budget, Some(500));
    // A restored world whose request states no budget stands under the one
    // the header of its save holds.
    let request = Restore::ContentAtHand { dropped: &[], drop_in_silence: false, types_from: None };
    let restored = World::restore(config(&[PARAMETERS]), &save, Vec::new(), Links::new(parts.clone()), request).expect("the restore is not refused");
    assert_eq!(restored.budget(), Some(500));
    // The controls: the event in no record, and the budget of the creation
    // in the header.
    let (_, log, _) = run(Rules { budget_events_left_out_of_the_log: true, ..Rules::default() });
    assert_eq!(log.len(), 1);
    let (_, _, save) = run(Rules { save_holds_the_budget_of_creation: true, ..Rules::default() });
    assert_eq!(save_header(&save).expect("a header").budget, Some(1_000));
}

/// A store that hands the prepared form of a part: the bytes under the hash
/// of the form an entry names, and none where it names none.
struct WithForms {
    parts: Arc<Parts>,
    forms: BTreeMap<u64, Arc<Vec<u8>>>,
}

impl Residents for WithForms {
    fn get(&self, hash: u64) -> Option<Arc<Resident>> {
        self.parts.get(hash)
    }

    fn form(&self, _part: u64, named: Option<u64>) -> Option<Arc<Vec<u8>>> {
        self.forms.get(&named?).cloned()
    }
}

/// A world whose composition declares a prepared form built once for the
/// second type: the entry holds the hash of the form its arrival named, a
/// save holds it, the consumer of the form is handed the bytes under that
/// hash at every tick, and a restore under the recorded content confirms the
/// form at its recorded hash or is refused, naming the asset and that hash.
#[test]
fn a_form_built_once_is_handed_by_its_hash_and_confirmed_at_a_restore_under_the_recorded_content() {
    let (parts, (a, _, t)) = parts();
    let form = b"the form built once".to_vec();
    let built = fnv1a(&form);
    let store = Arc::new(WithForms { parts: parts.clone(), forms: BTreeMap::from([(built, Arc::new(form))]) });
    let declaring = || {
        let mut config = config(&[PARAMETERS, SECOND]);
        config.reads_the_forms_of = vec![SECOND];
        config
    };
    let mut first = Record::arrivals(2, vec![Arrival::at(A, a).sized(10), Arrival::at(T, t).with_form(built).sized(30)]);
    first.commands = vec![want(A, PARAMETERS, Policy::Required), want(T, SECOND, Policy::Required)];
    let mut world = World::new(declaring(), vec![first], Links::new(store.clone()));
    assert!(!step(&mut world, 4).gated);
    // The consumer runs once in every tick, for each available entry whose
    // type has the form: nothing at tick 1, the form of `T` from tick 2 on.
    assert_eq!(world.handed(), [(2, T, Some(built)), (3, T, Some(built)), (4, T, Some(built))]);
    let save = world.save();
    let confirm = |_: AssetId, hash: u64| parts.get(hash).ok_or_else(|| "the part cannot be had".to_string());
    let asked = std::sync::Mutex::new(Vec::new());
    let confirm_form = |asset: AssetId, part: u64, form: u64| -> Result<(), String> {
        asked.lock().expect("never poisoned").push((asset, part, form));
        Ok(())
    };
    let request = Restore::Continue { confirm: &confirm, confirm_form: Some(&confirm_form), dropped: &[] };
    let mut restored = World::restore(declaring(), &save, Vec::new(), Links::new(store.clone()), request).expect("the restore is admitted");
    assert_eq!(*asked.lock().expect("never poisoned"), vec![(T, t, built)], "the form is confirmed at its recorded hash, with the hash of its part");
    assert_eq!(*restored.history(), History::Continues);
    assert!(!step(&mut restored, 1).gated);
    assert_eq!(restored.handed(), [(5, T, Some(built))]);
    // A form that is not confirmed refuses the restore by its recorded hash.
    let refuse = |_: AssetId, _: u64, _: u64| -> Result<(), String> { Err("the form cannot be had".to_string()) };
    let request = Restore::Continue { confirm: &confirm, confirm_form: Some(&refuse), dropped: &[] };
    let refused = World::restore(declaring(), &save, Vec::new(), Links::new(store.clone()), request).err();
    assert_eq!(refused, Some(Refusal::Content { asset: T, hash: built, reason: "the form cannot be had".to_string() }));
    // A request that gives nothing to confirm a form with confirms none.
    let request = Restore::Continue { confirm: &confirm, confirm_form: None, dropped: &[] };
    assert!(matches!(World::restore(declaring(), &save, Vec::new(), Links::new(store.clone()), request), Err(Refusal::Content { asset: T, .. })));
    // Under the control the confirmation reads the hash of the part alone.
    let mut part_alone = declaring();
    part_alone.rules.confirmation_reads_the_part_alone = true;
    let request = Restore::Continue { confirm: &confirm, confirm_form: Some(&refuse), dropped: &[] };
    assert!(World::restore(part_alone, &save, Vec::new(), Links::new(store), request).is_ok());
}
