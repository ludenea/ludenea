//! The self-checks of the loading capability: what it posts to a world's
//! queue for a want that leaves, a restore and a move of the manifest, over
//! a loose store written by hand. They hold no oracle and step no world: a
//! world is its queue and the table a host extracts for it.

use std::sync::atomic::{AtomicU32, Ordering};

use content_store::{encode_domain, encode_root};
use prototype_core::table::{Dependency, RepulsionTable, part_bytes};

use super::*;

const A: AssetId = 0x0a;
const D: AssetId = 0x0d;
const E: AssetId = 0x0e;
const F: AssetId = 0x0f;
const N: AssetId = 0x20;
const O: AssetId = 0x21;
const P: AssetId = 0x30;
const Q: AssetId = 0x31;
const R: AssetId = 0x32;
const U: AssetId = 0x40;
const PARAMETERS: AssetType = AssetType::Parameters;
const SECOND: AssetType = AssetType::Second;

static STORES: AtomicU32 = AtomicU32::new(0);

/// One asset of a revision: its identifier, the type the manifest holds it
/// under, a version that makes its content distinct, and what its part lists.
type Content<'a> = (AssetId, AssetType, u32, &'a [Dependency]);

/// A loose store written by hand, in a directory of its own.
struct Loose {
    directory: PathBuf,
}

impl Loose {
    fn new() -> Loose {
        let directory = std::env::temp_dir().join(format!("asset-loader-check-{}-{}", std::process::id(), STORES.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("objects")).expect("a scratch directory is created");
        Loose { directory }
    }

    fn put(&self, bytes: &[u8]) -> u64 {
        let hash = content_hash(bytes);
        fs::write(self.directory.join("objects").join(hex(hash)), bytes).expect("an object is written");
        hash
    }

    /// Writes the parts of a revision and its manifest, and puts it in place.
    fn revision(&self, contents: &[Content]) -> Domain {
        let mut domain = Domain::new();
        for (asset, asset_type, version, dependencies) in contents {
            let bytes = part(*version, dependencies);
            let listed = dependencies.iter().map(|(dependency, listed)| (*dependency, listed.stable_name().to_string())).collect();
            let entry = DomainEntry { asset_type: asset_type.stable_name().to_string(), hash: self.put(&bytes), size: bytes.len() as u64, dependencies: listed };
            domain.insert(*asset, entry);
        }
        self.place(&domain);
        domain
    }

    /// Puts a manifest in place as the revision.
    fn place(&self, domain: &Domain) {
        let hash = self.put(&encode_domain(domain));
        fs::write(self.directory.join("root"), encode_root(hash)).expect("the root document is written");
    }

    fn remove(&self, hash: u64) {
        fs::remove_file(self.directory.join("objects").join(hex(hash))).expect("the object is in the store");
    }
}

impl Drop for Loose {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn part(version: u32, dependencies: &[Dependency]) -> Vec<u8> {
    part_bytes(RepulsionTable { strength: version as f32, radius: 8.0 }, dependencies, 8, version)
}

fn hash_of(version: u32, dependencies: &[Dependency]) -> u64 {
    content_hash(&part(version, dependencies))
}

fn held(asset: AssetId, asset_type: AssetType, availability: Availability, wanted: bool) -> EntryView {
    EntryView { asset, asset_type, availability, wanted }
}

/// A world as the capability sees it: its queue and what it extracts through.
struct Attached {
    queue: Arc<ArrivalQueue>,
    extract: Arc<dyn Fn(Extraction) + Send + Sync>,
}

impl Attached {
    fn new(loader: &Loader, set: Option<u64>, entries: &[EntryView], unanswered: &[(AssetId, AssetType)], restore: &[AssetId]) -> Attached {
        let queue = Arc::new(ArrivalQueue::default());
        let attachment = Attachment { queue: queue.clone(), set, entries: entries.to_vec(), unanswered: unanswered.to_vec(), restore: restore.to_vec() };
        Attached { extract: loader.attach(attachment).expect("the world is attached"), queue }
    }

    /// What stands in the queue, in identifier order, once the capability
    /// has nothing left to post; the queue is emptied.
    fn posted(&self, loader: &Loader) -> Vec<Arrival> {
        loader.settle();
        let mut pending = BTreeMap::new();
        self.queue.drain_into(&mut pending);
        pending.into_values().collect()
    }

    fn extract(&self, left: &[(AssetId, AssetType)], entries: &[EntryView], restore_applied: bool) {
        (self.extract)(Extraction { tick: 1, left: left.to_vec(), wanted: left.to_vec(), entries: entries.to_vec(), restore_applied });
    }
}

#[test]
fn a_want_that_leaves_is_answered_by_its_closure_without_what_the_table_holds_available() {
    let store = Loose::new();
    let lists: &[Dependency] = &[(D, PARAMETERS), (E, PARAMETERS)];
    let domain = store.revision(&[(D, PARAMETERS, 1, &[]), (E, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, lists), (A, PARAMETERS, 1, &[])]);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let world = Attached::new(&loader, None, &[], &[], &[]);
    assert!(world.posted(&loader).is_empty(), "a table that asks nothing is posted nothing");
    let table = [held(D, PARAMETERS, Availability::Available(domain[&D].hash), true), held(P, PARAMETERS, Availability::Unavailable, true)];
    world.extract(&[(P, PARAMETERS)], &table, false);
    assert_eq!(world.posted(&loader), vec![Arrival::at(E, domain[&E].hash), Arrival::at(P, domain[&P].hash)], "one complete batch, the asset in force left out");
    // A table that extracts no want that leaves is posted nothing.
    world.extract(&[], &table, false);
    assert!(world.posted(&loader).is_empty());
    assert_eq!(loader.report().reads, 3, "the manifest and two parts: a part the table holds available is not read");
    // Not taken: every want answered, an asset in force included.
    let every = Loader::start(&store.directory, Options { every_want_answered: true, ..Options::default() }).expect("the capability starts");
    let world = Attached::new(&every, None, &[], &[], &[]);
    world.extract(&[(D, PARAMETERS)], &table, false);
    assert_eq!(world.posted(&every), vec![Arrival::at(D, domain[&D].hash)]);
}

#[test]
fn an_asset_the_manifest_holds_under_another_type_than_its_entry_fails_for_that_reason() {
    let store = Loose::new();
    let domain = store.revision(&[(A, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, &[(D, SECOND)]), (D, SECOND, 1, &[])]);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let world = Attached::new(&loader, None, &[], &[(A, SECOND), (U, PARAMETERS)], &[]);
    assert_eq!(world.posted(&loader), vec![Arrival::failed(A, REASON_ANOTHER_TYPE), Arrival::failed(U, REASON_UNKNOWN_ASSET)]);
    // A want leaves with the type its entry holds: once the entry has taken
    // the manifest's type, the same asset loads.
    world.extract(&[(A, PARAMETERS)], &[held(A, PARAMETERS, Availability::Unavailable, true)], false);
    assert_eq!(world.posted(&loader), vec![Arrival::at(A, domain[&A].hash)]);
    // An asset of a closure is checked against the entry the table holds for it.
    let table = [held(D, PARAMETERS, Availability::Unavailable, true), held(P, PARAMETERS, Availability::Unavailable, true)];
    world.extract(&[(P, PARAMETERS)], &table, false);
    assert_eq!(world.posted(&loader), vec![Arrival::failed(D, REASON_ANOTHER_TYPE), Arrival::at(P, domain[&P].hash)]);
    // The control: no type is checked.
    let unchecked = Loader::start(&store.directory, Options { types_not_checked: true, ..Options::default() }).expect("the capability starts");
    let world = Attached::new(&unchecked, None, &[], &[(A, SECOND)], &[]);
    assert_eq!(world.posted(&unchecked), vec![Arrival::at(A, domain[&A].hash)]);
}

#[test]
fn a_part_whose_dependencies_are_not_those_the_manifest_lists_fails() {
    let store = Loose::new();
    let mut domain = store.revision(&[(D, SECOND, 1, &[]), (P, PARAMETERS, 1, &[(D, SECOND)]), (Q, PARAMETERS, 1, &[(D, SECOND)]), (R, PARAMETERS, 1, &[(D, PARAMETERS)])]);
    // The manifest lists for P a dependency its part does not list, lists
    // the dependency of Q under another type than the part does, and holds
    // the dependency of R under another type than R's part and entry list.
    domain.get_mut(&P).expect("an entry").dependencies = vec![(E, SECOND.stable_name().to_string())];
    domain.get_mut(&Q).expect("an entry").dependencies = vec![(D, PARAMETERS.stable_name().to_string())];
    store.place(&domain);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let outcome = |asset: AssetId| {
        let world = Attached::new(&loader, None, &[], &[(asset, PARAMETERS)], &[]);
        world.posted(&loader).into_iter().find(|arrival| arrival.asset == asset).map(|arrival| arrival.outcome)
    };
    assert_eq!(outcome(P), Some(Outcome::Failure(REASON_DEPENDENCIES_DIFFER)));
    assert_eq!(outcome(Q), Some(Outcome::Failure(REASON_DEPENDENCIES_DIFFER)));
    assert_eq!(outcome(R), Some(Outcome::Failure(REASON_DEPENDENCIES_DIFFER)), "each under the type the manifest holds that dependency under");
    let world = Attached::new(&loader, None, &[], &[(D, SECOND)], &[]);
    assert_eq!(world.posted(&loader), vec![Arrival::at(D, domain[&D].hash)]);
}

#[test]
fn the_batch_of_a_revision_holds_what_it_changes_that_the_table_names_and_the_closure_of_a_changed_part() {
    let first: [Content; 3] = [(P, PARAMETERS, 1, &[]), (Q, PARAMETERS, 1, &[]), (R, PARAMETERS, 1, &[])];
    let second: [Content; 5] = [(P, PARAMETERS, 2, &[(N, PARAMETERS), (R, PARAMETERS)]), (Q, PARAMETERS, 2, &[]), (R, PARAMETERS, 1, &[]), (N, PARAMETERS, 1, &[]), (A, PARAMETERS, 1, &[])];
    let run = |options: Options| {
        let store = Loose::new();
        let before = store.revision(&first);
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let available = |asset: AssetId, wanted: bool| held(asset, PARAMETERS, Availability::Available(before[&asset].hash), wanted);
        // P and R are wanted, Q is an entry with no want.
        let world = Attached::new(&loader, None, &[available(P, true), available(Q, false), available(R, true)], &[], &[]);
        let after = store.revision(&second);
        loader.revision_in_place().expect("the revision is read");
        (world.posted(&loader), after)
    };
    let (posted, after) = run(Options::default());
    let expected = vec![Arrival::at(N, after[&N].hash), Arrival::at(P, after[&P].hash), Arrival::at(Q, after[&Q].hash)];
    assert_eq!(posted, expected, "the new dependency with its parent, the entry with no want, and for no other asset");
    // The text as it stood: the changed assets a want names, and nothing else.
    let (posted, after) = run(Options { moves_leave_entries_without_a_want: true, moves_without_the_closure: true, ..Options::default() });
    assert_eq!(posted, vec![Arrival::at(P, after[&P].hash)]);
}

#[test]
fn an_asset_that_leaves_the_manifest_fails_and_a_replacement_that_cannot_be_put_in_force_is_a_report() {
    let store = Loose::new();
    let before = store.revision(&[(A, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, &[])]);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let table = [held(A, PARAMETERS, Availability::Available(before[&A].hash), true), held(P, PARAMETERS, Availability::Available(before[&P].hash), true)];
    let world = Attached::new(&loader, None, &table, &[], &[]);
    // A new version whose artifact is gone, and one the manifest holds under another type.
    let after = store.revision(&[(A, PARAMETERS, 2, &[]), (P, SECOND, 1, &[])]);
    store.remove(after[&A].hash);
    loader.revision_in_place().expect("the revision is read");
    assert!(world.posted(&loader).is_empty(), "the assets stay at their hashes");
    assert_eq!(loader.report().refusals, vec![(A, REASON_ARTIFACT_GONE), (P, REASON_ANOTHER_TYPE)]);
    // An asset that leaves the manifest is a different event and does fail.
    store.revision(&[(P, PARAMETERS, 1, &[])]);
    loader.revision_in_place().expect("the revision is read");
    assert_eq!(world.posted(&loader), vec![Arrival::failed(A, REASON_UNKNOWN_ASSET), Arrival::at(P, before[&P].hash)]);
}

#[test]
fn a_move_reaches_a_failed_entry_whose_asset_enters_the_manifest() {
    let run = |options: Options| {
        let store = Loose::new();
        store.revision(&[(P, PARAMETERS, 1, &[])]);
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let world = Attached::new(&loader, None, &[held(A, PARAMETERS, Availability::Failed(REASON_UNKNOWN_ASSET), true)], &[], &[]);
        let after = store.revision(&[(P, PARAMETERS, 1, &[]), (A, PARAMETERS, 1, &[])]);
        loader.revision_in_place().expect("the revision is read");
        (world.posted(&loader), after[&A].hash)
    };
    let (posted, hash) = run(Options::default());
    assert_eq!(posted, vec![Arrival::at(A, hash)], "the move changes the asset, and the table names it");
    // The other reading: a failed entry moves by a want alone.
    let (posted, _) = run(Options { moves_leave_failed_entries: true, ..Options::default() });
    assert!(posted.is_empty());
}

#[test]
fn a_move_that_changes_the_type_of_an_asset_in_force_is_a_report() {
    let run = |options: Options| {
        let store = Loose::new();
        let before = store.revision(&[(P, PARAMETERS, 1, &[])]);
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let world = Attached::new(&loader, None, &[held(P, PARAMETERS, Availability::Available(before[&P].hash), true)], &[], &[]);
        store.revision(&[(P, SECOND, 1, &[])]);
        loader.revision_in_place().expect("the revision is read");
        (world.posted(&loader), loader.report().refusals)
    };
    assert_eq!(run(Options::default()), (Vec::new(), vec![(P, REASON_ANOTHER_TYPE)]), "a replacement that cannot be put in force leaves what is in force");
    // The other reading: the asset fails for its type.
    let other = run(Options { a_changed_type_fails_an_asset_in_force: true, ..Options::default() });
    assert_eq!(other, (vec![Arrival::failed(P, REASON_ANOTHER_TYPE)], Vec::new()));
}

#[test]
fn the_answers_to_a_restore_follow_the_closure_and_a_revision_is_held_until_the_first_tick() {
    let store = Loose::new();
    let lists: &[Dependency] = &[(N, PARAMETERS), (F, PARAMETERS)];
    let at_hand = store.revision(&[(P, PARAMETERS, 2, lists), (N, PARAMETERS, 1, &[]), (F, PARAMETERS, 1, &[]), (U, PARAMETERS, 1, &[]), (Q, PARAMETERS, 1, &[])]);
    // The saved table: P available under a want at another hash, O available
    // with no want, F failed under a want, U wanted and unanswered, Q an
    // entry with no want that never rose.
    let saved = [
        held(F, PARAMETERS, Availability::Failed(REASON_ARTIFACT_GONE), true),
        held(O, PARAMETERS, Availability::Available(7), false),
        held(P, PARAMETERS, Availability::Available(hash_of(1, &[])), true),
        held(Q, PARAMETERS, Availability::Unavailable, false),
        held(U, PARAMETERS, Availability::Unavailable, true),
    ];
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let world = Attached::new(&loader, None, &saved, &[(U, PARAMETERS)], &[O, P]);
    let expected = vec![
        Arrival::at(N, at_hand[&N].hash),
        Arrival::failed(O, REASON_UNKNOWN_ASSET).marked(),
        Arrival::at(P, at_hand[&P].hash).marked(),
        Arrival::at(U, at_hand[&U].hash),
    ];
    assert_eq!(world.posted(&loader), expected, "the new dependency as an ordinary arrival; the entry held failed under a want receives nothing");
    // A revision that lands while the world waits is held, then posted as a reload.
    let later = store.revision(&[(P, PARAMETERS, 3, &[]), (N, PARAMETERS, 1, &[]), (F, PARAMETERS, 1, &[]), (U, PARAMETERS, 1, &[]), (Q, PARAMETERS, 1, &[])]);
    loader.revision_in_place().expect("the revision is read");
    assert!(world.posted(&loader).is_empty());
    assert_eq!(loader.report().held_moves, 1);
    let after_the_first_tick = [
        held(F, PARAMETERS, Availability::Failed(REASON_ARTIFACT_GONE), true),
        held(N, PARAMETERS, Availability::Available(at_hand[&N].hash), true),
        held(P, PARAMETERS, Availability::Available(at_hand[&P].hash), true),
        held(U, PARAMETERS, Availability::Available(at_hand[&U].hash), true),
    ];
    world.extract(&[], &after_the_first_tick, true);
    assert_eq!(world.posted(&loader), vec![Arrival::at(P, later[&P].hash)]);
    // The other reading: every asset of the closure that the table does not
    // hold available is posted, the failed entry under a want included.
    let complete = Loader::start(&store.directory, Options { restore_answers_complete_the_batch: true, ..Options::default() }).expect("the capability starts");
    store.place(&at_hand);
    complete.revision_in_place().expect("the revision is read");
    let world = Attached::new(&complete, None, &saved, &[], &[O, P]);
    assert!(world.posted(&complete).contains(&Arrival::at(F, at_hand[&F].hash)));
    // Not taken: the answers and nothing else.
    let literal = Loader::start(&store.directory, Options { restore_answers_do_not_follow_the_closure: true, ..Options::default() }).expect("the capability starts");
    let world = Attached::new(&literal, None, &saved, &[], &[O, P]);
    assert_eq!(world.posted(&literal), vec![Arrival::failed(O, REASON_UNKNOWN_ASSET).marked(), Arrival::at(P, at_hand[&P].hash).marked()]);
}

#[test]
fn a_world_under_a_named_set_is_resolved_in_its_set_and_receives_no_revision() {
    let store = Loose::new();
    let set = store.revision(&[(A, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, &[])]);
    let named = Some(set_hash(&set));
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let table = [held(A, PARAMETERS, Availability::Available(set[&A].hash), true)];
    let under_set = Attached::new(&loader, named, &table, &[], &[]);
    let no_set = Attached::new(&loader, None, &table, &[], &[]);
    let later = store.revision(&[(A, PARAMETERS, 2, &[]), (P, PARAMETERS, 2, &[]), (N, PARAMETERS, 1, &[])]);
    loader.revision_in_place().expect("the revision is read");
    assert!(under_set.posted(&loader).is_empty(), "a world created under a content set receives nothing");
    assert_eq!(no_set.posted(&loader), vec![Arrival::at(A, later[&A].hash)]);
    // A want made meanwhile is answered inside the set, and one it cannot
    // answer there is a failed transition.
    under_set.extract(&[(P, PARAMETERS), (N, PARAMETERS)], &table, false);
    assert_eq!(under_set.posted(&loader), vec![Arrival::failed(N, REASON_UNKNOWN_ASSET), Arrival::at(P, set[&P].hash)]);
    no_set.extract(&[(P, PARAMETERS), (N, PARAMETERS)], &table, false);
    assert_eq!(no_set.posted(&loader), vec![Arrival::at(N, later[&N].hash), Arrival::at(P, later[&P].hash)]);
}

#[test]
fn several_worlds_each_receive_their_own_batches_whatever_the_delay() {
    let store = Loose::new();
    let domain = store.revision(&[(D, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, &[(D, PARAMETERS)]), (A, PARAMETERS, 1, &[])]);
    let wanted = |asset: AssetId| [held(asset, PARAMETERS, Availability::Unavailable, true)];
    let own_p = vec![Arrival::at(D, domain[&D].hash), Arrival::at(P, domain[&P].hash)];
    let own_a = vec![Arrival::at(A, domain[&A].hash)];
    for delay in [Delay::None, Delay::ReverseOfTheOrder, Delay::Clock] {
        let loader = Loader::start(&store.directory, Options { delay, ..Options::default() }).expect("the capability starts");
        let first = Attached::new(&loader, None, &[], &[], &[]);
        let second = Attached::new(&loader, None, &[], &[], &[]);
        first.extract(&[(P, PARAMETERS)], &wanted(P), false);
        second.extract(&[(A, PARAMETERS)], &wanted(A), false);
        assert_eq!(first.posted(&loader), own_p, "the batch that answers the first world, once the capability has nothing left to post");
        assert_eq!(second.posted(&loader), own_a, "and the batch that answers the second, on its own queue");
        assert_eq!(loader.report().batches, 2);
    }
    // The control: a batch is posted on the queue of every world.
    let every = Loader::start(&store.directory, Options { batches_posted_to_every_world: true, ..Options::default() }).expect("the capability starts");
    let first = Attached::new(&every, None, &[], &[], &[]);
    let second = Attached::new(&every, None, &[], &[], &[]);
    first.extract(&[(P, PARAMETERS)], &wanted(P), false);
    assert_eq!(second.posted(&every), own_p);
}

#[test]
fn under_the_candidate_on_the_closure_of_a_want_an_arrival_to_a_hash_names_its_root() {
    let store = Loose::new();
    let shared: &[Dependency] = &[(D, PARAMETERS), (E, PARAMETERS)];
    let domain = store.revision(&[
        (D, PARAMETERS, 1, &[]),
        (E, PARAMETERS, 1, &[]),
        (P, PARAMETERS, 1, shared),
        (Q, PARAMETERS, 1, &[(E, PARAMETERS)]),
    ]);
    let loader = Loader::start(&store.directory, Options { roots_marked: true, ..Options::default() }).expect("the capability starts");
    let world = Attached::new(&loader, None, &[], &[], &[]);
    let table = [held(P, PARAMETERS, Availability::Unavailable, true), held(Q, PARAMETERS, Availability::Unavailable, true), held(U, PARAMETERS, Availability::Unavailable, true)];
    world.extract(&[(Q, PARAMETERS), (P, PARAMETERS), (U, PARAMETERS)], &table, false);
    let at = |asset: AssetId, root: AssetId| Arrival::at(asset, domain[&asset].hash).for_root(root);
    // `E` stands in the closures of both wants: it names the first of the
    // two assets in identifier order. A failure has no size and names none.
    assert_eq!(world.posted(&loader), vec![at(D, P), at(E, P), at(P, P), at(Q, Q), Arrival::failed(U, REASON_UNKNOWN_ASSET)]);
    // The batch of a revision: a changed asset names itself, and an asset of
    // the closure of a changed part names the changed asset that brought it.
    let table = [
        held(D, PARAMETERS, Availability::Available(domain[&D].hash), true),
        held(E, PARAMETERS, Availability::Available(domain[&E].hash), true),
        held(P, PARAMETERS, Availability::Available(domain[&P].hash), true),
        held(Q, PARAMETERS, Availability::Available(domain[&Q].hash), true),
    ];
    world.extract(&[], &table, false);
    assert!(world.posted(&loader).is_empty());
    let gains: &[Dependency] = &[(E, PARAMETERS), (N, PARAMETERS)];
    let later = store.revision(&[(D, PARAMETERS, 2, &[]), (E, PARAMETERS, 1, &[]), (N, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, shared), (Q, PARAMETERS, 2, gains)]);
    loader.revision_in_place().expect("the revision is read");
    let moved = |asset: AssetId, root: AssetId| Arrival::at(asset, later[&asset].hash).for_root(root);
    assert_eq!(world.posted(&loader), vec![moved(D, D), moved(N, Q), moved(Q, Q)]);
    // No other candidate writes the mark.
    let plain = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let world = Attached::new(&plain, None, &[], &[(Q, PARAMETERS)], &[]);
    assert!(world.posted(&plain).iter().all(|arrival| arrival.root.is_none()));
}

#[test]
fn a_hold_keeps_the_capability_still_and_the_arrivals_of_a_move_may_be_posted_one_per_tick() {
    let store = Loose::new();
    let domain = store.revision(&[(A, PARAMETERS, 1, &[]), (D, PARAMETERS, 3, &[])]);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let world = Attached::new(&loader, None, &[], &[], &[]);
    let waiting = |queue: &ArrivalQueue| {
        let mut pending = BTreeMap::new();
        queue.drain_into(&mut pending);
        pending.len()
    };
    // While the host holds the capability, what a tick asked of it is not
    // posted: a step taken then meets a closed gate.
    let hold = loader.hold();
    world.extract(&[(A, PARAMETERS)], &[held(A, PARAMETERS, Availability::Unavailable, true)], false);
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(waiting(&world.queue), 0);
    drop(hold);
    assert_eq!(world.posted(&loader), vec![Arrival::at(A, domain[&A].hash)]);
    // The control of a mount and of a revision: one arrival per tick, in
    // identifier order, each posted when the host waits before a step.
    let slow = Loader::start(&store.directory, Options { moves_posted_one_per_tick: true, ..Options::default() }).expect("the capability starts");
    let table = [held(A, PARAMETERS, Availability::Available(domain[&A].hash), true), held(D, PARAMETERS, Availability::Available(domain[&D].hash), true)];
    let world = Attached::new(&slow, None, &table, &[], &[]);
    let later = store.revision(&[(A, PARAMETERS, 2, &[]), (D, PARAMETERS, 4, &[])]);
    slow.revision_in_place().expect("the revision is read");
    assert_eq!(world.posted(&slow), vec![Arrival::at(A, later[&A].hash)]);
    assert_eq!(world.posted(&slow), vec![Arrival::at(D, later[&D].hash)]);
    assert!(world.posted(&slow).is_empty());
}

#[test]
fn a_restore_under_the_recorded_content_is_confirmed_in_the_content_at_hand() {
    let store = Loose::new();
    let domain = store.revision(&[(A, PARAMETERS, 1, &[]), (D, PARAMETERS, 3, &[])]);
    store.remove(domain[&D].hash);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let confirmed = loader.confirm(None, A, domain[&A].hash).expect("the content holds the asset at the recorded hash");
    assert_eq!(content_hash(&confirmed.bytes), domain[&A].hash, "the part is had, outside any input record");
    assert!(loader.confirm(None, A, hash_of(2, &[])).is_err(), "another hash than the content holds");
    assert!(loader.confirm(None, U, domain[&A].hash).is_err(), "an asset the content does not hold");
    assert!(loader.confirm(None, D, domain[&D].hash).is_err(), "a part that cannot be had");
}
