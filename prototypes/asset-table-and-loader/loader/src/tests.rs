//! The self-checks of the loading capability: what it posts to a world's
//! queue for a want that leaves, a restore and a move of the manifest, over
//! a loose store written by hand. They hold no oracle and step no world: a
//! world is its queue and the table a host extracts for it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

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
            let entry = DomainEntry { asset_type: asset_type.stable_name().to_string(), hash: self.put(&bytes), size: bytes.len() as u64, dependencies: listed, forms: Vec::new() };
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

/// The arrival of an asset as a manifest gives it: at the hash it names, with
/// the size it gives, which is what an arrival names.
fn of(domain: &Domain, asset: AssetId) -> Arrival {
    Arrival::at(asset, domain[&asset].hash).sized(domain[&asset].size)
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
        let attachment = Attachment { queue: queue.clone(), set, entries: entries.to_vec(), unanswered: unanswered.to_vec(), restore: restore.to_vec(), standing: Vec::new(), checks: Vec::new(), forms: Vec::new() };
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
    assert_eq!(world.posted(&loader), vec![of(&domain, E), of(&domain, P)], "one complete batch, the asset in force left out");
    // A table that extracts no want that leaves is posted nothing.
    world.extract(&[], &table, false);
    assert!(world.posted(&loader).is_empty());
    assert_eq!(loader.report().reads, 3, "the manifest and two parts: a part the table holds available is not read");
    // Not taken: every want answered, an asset in force included.
    let every = Loader::start(&store.directory, Options { every_want_answered: true, ..Options::default() }).expect("the capability starts");
    let world = Attached::new(&every, None, &[], &[], &[]);
    world.extract(&[(D, PARAMETERS)], &table, false);
    assert_eq!(world.posted(&every), vec![of(&domain, D)]);
}

/// A world whose composition declares the stand-in form built once for the
/// second type, as the capability sees it.
fn declaring(loader: &Loader, unanswered: &[(AssetId, AssetType)]) -> Attached {
    let queue = Arc::new(ArrivalQueue::default());
    let forms = vec![(SECOND, Preparer::Stamp)];
    let attachment = Attachment { queue: queue.clone(), set: None, entries: Vec::new(), unanswered: unanswered.to_vec(), restore: Vec::new(), standing: Vec::new(), checks: Vec::new(), forms };
    Attached { extract: loader.attach(attachment).expect("the world is attached"), queue }
}

/// A store whose manifest lists, for one asset of the second type, a form
/// built once under the producer's platform name; the store holds the form
/// under its hash where `held` is set. Returns the manifest, the hash of
/// the form built once and the form the machine builds for the same part.
fn with_a_form(store: &Loose, held: bool) -> (Domain, u64, Vec<u8>) {
    let mut domain = store.revision(&[(A, PARAMETERS, 1, &[]), (N, SECOND, 1, &[])]);
    let built = stamp(&part(1, &[]), "producer", None);
    let hash = if held { store.put(&built) } else { content_hash(&built) };
    domain.get_mut(&N).expect("an entry").forms = vec![content_store::PreparedForm { preparer: Preparer::Stamp.label(), hash, size: built.len() as u64 }];
    store.place(&domain);
    (domain, hash, stamp(&part(1, &[]), &local_platform(), None))
}

#[test]
fn an_arrival_names_the_form_built_once_its_world_declares_and_the_size_of_the_part_with_it() {
    assert_eq!(content_hash(b"one function"), hash_by_the_core(b"one function"), "the core and the store hash a content alike");
    let store = Loose::new();
    let (domain, built, _) = with_a_form(&store, true);
    let (part_hash, size) = (domain[&N].hash, domain[&N].size + domain[&N].forms[0].size);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let world = declaring(&loader, &[(A, PARAMETERS), (N, SECOND)]);
    assert_eq!(world.posted(&loader), vec![of(&domain, A), Arrival::at(N, part_hash).with_form(built).sized(size)]);
    // The form is resident with its part, and a system is handed its bytes
    // by the hash the entry names.
    let handed = loader.residents().form(part_hash, Some(built)).expect("the form is resident");
    assert_eq!(content_hash(&handed), built);
    // A world whose composition declares no form: the part alone, no form.
    let plain = Attached::new(&loader, None, &[], &[(N, SECOND)], &[]);
    assert_eq!(plain.posted(&loader), vec![of(&domain, N)]);
    // The controls: no form named; the size of the part alone; every form
    // of the manifest named, whatever the composition declares.
    let posted = |options: Options, declares: bool| {
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let world = if declares { declaring(&loader, &[(N, SECOND)]) } else { Attached::new(&loader, None, &[], &[(N, SECOND)], &[]) };
        world.posted(&loader)
    };
    assert_eq!(posted(Options { forms_not_named: true, ..Options::default() }, true), vec![Arrival::at(N, part_hash).sized(size)]);
    let part_alone = Options { arrivals_name_the_size_of_the_part_alone: true, ..Options::default() };
    assert_eq!(posted(part_alone, true), vec![Arrival::at(N, part_hash).with_form(built).sized(domain[&N].size)]);
    let every_form = Options { arrivals_name_every_form_of_the_manifest: true, ..Options::default() };
    assert_eq!(posted(every_form, false), vec![Arrival::at(N, part_hash).with_form(built).sized(size)]);
}

/// The content hash as the core computes it: the one a stamp holds of its
/// part, in its last eight bytes.
fn hash_by_the_core(bytes: &[u8]) -> u64 {
    let form = stamp(bytes, "", None);
    u64::from_le_bytes(form[form.len() - 8..].try_into().expect("eight bytes"))
}

#[test]
fn a_form_built_once_is_read_under_its_hash_and_the_machines_own_form_never_passes_for_it() {
    // The store holds no bytes under the hash of the form built once, and
    // the machine's cache of forms holds the form it built for the part.
    let store = Loose::new();
    let (domain, built, own) = with_a_form(&store, false);
    let part_hash = domain[&N].hash;
    let machine = Machine::beside(&store.directory);
    fs::create_dir_all(&machine.forms).expect("the cache of forms is created");
    fs::write(machine.form_path(part_hash, Preparer::Stamp), &own).expect("the machine's own form is kept");
    assert_ne!(content_hash(&own), built, "a form a machine built for itself has another hash");
    let first_load = |options: Options| {
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let world = declaring(&loader, &[(N, SECOND)]);
        let posted = world.posted(&loader);
        let handed = loader.residents().form(part_hash, Some(built)).map(|bytes| content_hash(&bytes));
        (posted, handed)
    };
    // The load fails: the form cannot be had. The reason is the one of an
    // artifact the store does not hold, and under the other reading the one
    // of a preparation.
    assert_eq!(first_load(Options::default()), (vec![Arrival::failed(N, REASON_ARTIFACT_GONE)], None));
    let as_a_preparation = Options { a_form_that_cannot_be_had_fails_as_a_preparation: true, ..Options::default() };
    assert_eq!(first_load(as_a_preparation).0, vec![Arrival::failed(N, REASON_PREPARATION_FAILED)]);
    // Control: the machine's own form is taken where the one named is gone,
    // and held under the hash named.
    let size = domain[&N].size + domain[&N].forms[0].size;
    let taken = Options { local_form_where_the_named_one_cannot_be_had: true, ..Options::default() };
    assert_eq!(first_load(taken), (vec![Arrival::at(N, part_hash).with_form(built).sized(size)], Some(content_hash(&own))));
    // A form whose bytes are not those its hash names does not verify.
    fs::write(store.directory.join("objects").join(hex(built)), &own).expect("an object is written");
    assert_eq!(first_load(Options::default()).0, vec![Arrival::failed(N, REASON_INVALID_PART)]);
    // With the form built once in the store beside the machine's own, the
    // bytes handed are those the hash names; under the control, the
    // machine's own, wherever the machine holds one.
    let _ = store.put(&stamp(&part(1, &[]), "producer", None));
    fs::write(store.directory.join("objects").join(hex(built)), stamp(&part(1, &[]), "producer", None)).expect("an object is written");
    assert_eq!(first_load(Options::default()).1, Some(built));
    assert_eq!(first_load(Options { forms_taken_from_the_machine: true, ..Options::default() }).1, Some(content_hash(&own)));
}

/// One content read through two selections: a client's, whose root document
/// names a presentation manifest beside the domain manifest, and a server's,
/// whose root names the domain manifest alone. What each posts to a world is
/// the same; the presentation manifest is read for its consumers alone.
#[test]
fn a_presentation_manifest_is_read_for_its_consumers_alone_and_a_server_reads_none() {
    use content_store::{Variant, encode_presentation, encode_root_with};
    let store = Loose::new();
    let lists: &[Dependency] = &[(D, PARAMETERS)];
    let domain = store.revision(&[(D, PARAMETERS, 1, &[]), (E, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, lists)]);
    // `P` has a payload and `E` as its presentation dependency; `E` has a
    // payload; `D` has none, and no entry.
    let entry = |payload: &[u8], dependencies: &[AssetId]| {
        let variant = Variant { name: "full".to_string(), hash: store.put(payload), size: payload.len() as u64 };
        PresentationEntry { variants: vec![variant], dependencies: dependencies.to_vec() }
    };
    let presented = |of_p: &[u8]| Presentation::from([(P, entry(of_p, &[E])), (E, entry(b"the payload of E", &[]))]);
    let place = |presentation: &Presentation| {
        let named = vec![("desktop".to_string(), store.put(&encode_presentation(presentation)))];
        fs::write(store.directory.join("root"), encode_root_with(set_hash(&domain), &named)).expect("the root document is written");
    };
    let table = [held(D, PARAMETERS, Availability::Available(domain[&D].hash), true), held(P, PARAMETERS, Availability::Available(domain[&P].hash), true)];
    // The server's selection: the root names the domain manifest alone.
    let server = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    assert!(server.presentation().is_none());
    let world = Attached::new(&server, None, &[], &[(P, PARAMETERS)], &[]);
    let batch = vec![of(&domain, D), of(&domain, P)];
    assert_eq!(world.posted(&server), batch);
    assert_eq!((server.report().presentation_manifests_read, server.report().payloads_read), (0, 0), "a server reads none");
    assert!(server.confirm(None, P, domain[&P].hash).is_ok());
    // The client's selection: the same batch, and the same content set.
    let first = presented(b"the payload of P, 1");
    place(&first);
    let client = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    assert_eq!(client.presentation(), Some(first.clone()));
    let world = Attached::new(&client, None, &[], &[(P, PARAMETERS)], &[]);
    assert_eq!(world.posted(&client), batch, "nothing of a presentation manifest reaches a world");
    assert_eq!((client.content_set(), server.content_set()), (set_hash(&domain), set_hash(&domain)));
    assert_eq!(client.report().payloads_read, 0, "a payload is loaded on the request of its consumer, and never by a want");
    // A consumer asks for a payload by the hash the manifest names.
    let payload = client.payload(first[&P].variants[0].hash).expect("the payload is had");
    assert_eq!(payload.as_slice(), b"the payload of P, 1");
    assert!(client.payload(first[&P].variants[0].hash).is_ok());
    assert_eq!((client.report().presentation_manifests_read, client.report().payloads_read), (1, 1), "a payload already resident is not read again");
    assert!(client.payload(0x55).is_err(), "a payload that cannot be loaded is reported to its consumer");
    // A presentation revision: another payload for `P`, and the domain
    // manifest as it was. The world receives nothing; under the control it
    // receives an arrival of `P`, at the hash it holds.
    let second = presented(b"the payload of P, 2");
    let after_a_presentation_revision = |options: Options| {
        place(&first);
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let world = Attached::new(&loader, None, &table, &[], &[]);
        place(&second);
        loader.revision_in_place().expect("the revision is read");
        (world.posted(&loader), loader.report().presentation_manifests_read)
    };
    assert_eq!(after_a_presentation_revision(Options::default()), (Vec::new(), 2));
    let as_a_revision = Options { presentation_changes_are_revisions: true, ..Options::default() };
    assert_eq!(after_a_presentation_revision(as_a_revision).0, vec![of(&domain, P)]);
    // The other controls, each on the client's selection.
    let posted = |options: Options| {
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let world = Attached::new(&loader, None, &[], &[(P, PARAMETERS)], &[]);
        world.posted(&loader)
    };
    let in_the_presentation_manifest = Options { resolves_in_the_presentation_manifest: true, ..Options::default() };
    assert_eq!(posted(in_the_presentation_manifest), vec![Arrival::failed(D, REASON_UNKNOWN_ASSET), Arrival::failed(P, REASON_INVALID_PART)]);
    let with_the_presentation_dependencies = Options { closure_follows_presentation_dependencies: true, ..Options::default() };
    assert_eq!(posted(with_the_presentation_dependencies), vec![of(&domain, D), of(&domain, E), of(&domain, P)]);
    let covering = Loader::start(&store.directory, Options { set_hash_covers_every_manifest: true, ..Options::default() }).expect("the capability starts");
    assert_ne!(covering.content_set(), set_hash(&domain));
    let confirming = Options { confirmation_confirms_payloads: true, ..Options::default() };
    assert!(Loader::start(&store.directory, confirming).expect("the capability starts").confirm(None, P, domain[&P].hash).is_ok());
    store.remove(second[&P].variants[0].hash);
    let waiting = Options { arrivals_wait_for_payloads: true, ..Options::default() };
    assert_eq!(posted(waiting), vec![of(&domain, D), Arrival::failed(P, REASON_ARTIFACT_GONE)]);
    assert!(Loader::start(&store.directory, confirming).expect("the capability starts").confirm(None, P, domain[&P].hash).is_err());
    // On a server's selection that control has no presentation manifest to
    // confirm a payload with, and the set a control covers is the domain's.
    store.place(&domain);
    assert!(Loader::start(&store.directory, confirming).expect("the capability starts").confirm(None, P, domain[&P].hash).is_err());
    let covering = Loader::start(&store.directory, Options { set_hash_covers_every_manifest: true, ..Options::default() }).expect("the capability starts");
    assert_eq!(covering.content_set(), set_hash(&domain));
}

/// Under a bound on the work at load: a piece whose estimate is above the
/// bound is an action of the background pool, and any other runs on the file
/// thread that read the artifact. What a world receives does not depend on
/// where the work ran. A bound of no duration puts every piece on the pool,
/// and a bound of an hour leaves every piece on the executor: no figure of
/// the machine is read here.
#[test]
fn a_bound_places_each_piece_of_work_at_load_on_a_file_thread_or_on_the_pool() {
    let store = Loose::new();
    let lists: &[Dependency] = &[(D, PARAMETERS), (N, SECOND)];
    let domain = store.revision(&[(D, PARAMETERS, 1, &[]), (N, SECOND, 2, &[]), (P, PARAMETERS, 3, lists)]);
    let forms = vec![(SECOND, Preparer::Grind)];
    let batch = vec![of(&domain, D), of(&domain, N), of(&domain, P)];
    let form = grind(&part(2, &[]));
    let under = |bound: Option<Duration>, cache: &str| {
        let machine = Machine { forms: store.directory.join(cache), bound: bound.map(|bound| LoadBound { bound, workers: 4 }), ..Machine::beside(&store.directory) };
        let kept = machine.form_path(domain[&N].hash, Preparer::Grind);
        let loader = Loader::start_on(&store.directory, machine, Options::default()).expect("the capability starts");
        let queue = Arc::new(ArrivalQueue::default());
        let attachment = Attachment { queue: queue.clone(), set: None, entries: Vec::new(), unanswered: vec![(P, PARAMETERS)], restore: Vec::new(), standing: Vec::new(), checks: Vec::new(), forms: forms.clone() };
        let world = Attached { extract: loader.attach(attachment).expect("the world is attached"), queue };
        assert_eq!(world.posted(&loader), batch, "what a world receives does not depend on where the work at load ran");
        // The form the machine builds is resident with its part, named by
        // no arrival, and kept in the machine's cache of forms.
        let handed = loader.residents().form(domain[&N].hash, None).expect("the form is resident");
        assert_eq!(*handed, form);
        assert_eq!(fs::read(&kept).expect("the form is kept in the machine's cache of forms"), form);
        assert_eq!(loader.report().reads, 4, "the manifest and three parts");
        loader
    };
    // Without a bound the instance has no pool and estimates nothing.
    assert!(under(None, "forms-unbound").bounded().is_none());
    // Every piece above the bound: seven actions, three verifications,
    // three validations and the form of the one part that has one.
    let on_the_pool = under(Some(Duration::ZERO), "forms-pool").bounded().expect("an instance under a bound");
    let ran = on_the_pool.ran();
    let count = |ran: &[Ran], work: Work| ran.iter().filter(|piece| piece.work == work).count();
    assert_eq!((count(&ran, Work::Verification), count(&ran, Work::Validation), count(&ran, Work::Form)), (3, 3, 1));
    assert!(ran.iter().all(|piece| matches!(piece.place, Place::Action(worker) if worker < 4)), "{ran:?}");
    assert!(ran.iter().all(|piece| piece.ended >= piece.started));
    assert_eq!(on_the_pool.posted().len(), 1, "the moment the batch stood on the world's queue is kept");
    assert!(ran.iter().all(|piece| piece.ended <= on_the_pool.posted()[0]), "the batch is posted once every piece of its closure has run");
    // The pieces of one artifact run one after the other, in order.
    let of_the_form = |work: Work| ran.iter().find(|piece| piece.hash == domain[&N].hash && piece.work == work).expect("a piece of the part that has a form");
    assert!(of_the_form(Work::Verification).ended <= of_the_form(Work::Validation).started && of_the_form(Work::Validation).ended <= of_the_form(Work::Form).started);
    // Every piece under the bound: on the file thread that read the artifact.
    let on_the_executor = under(Some(Duration::from_secs(3_600)), "forms-executor").bounded().expect("an instance under a bound");
    let ran = on_the_executor.ran();
    assert_eq!(ran.len(), 7);
    assert!(ran.iter().all(|piece| matches!(piece.place, Place::FileThread(thread) if thread < 2)), "{ran:?}");
    let threads_of = |hash: u64| ran.iter().filter(|piece| piece.hash == hash).map(|piece| piece.place).collect::<BTreeSet<_>>().len();
    assert_eq!(threads_of(domain[&N].hash), 1, "the pieces of one artifact run on the one file thread that read it");
    // The rates are measured when the instance starts, and an estimate is a
    // size divided by a rate.
    let rates = on_the_executor.rates;
    assert!(rates.verification > 0.0 && rates.validation > 0.0 && rates.form > 0.0 && rates.import_passes >= 1);
    assert!((rates.milliseconds(Work::Verification, 2_000_000) - 2.0 * rates.milliseconds(Work::Verification, 1_000_000)).abs() < 1e-9);
    // An artifact the store does not hold is left to the load path, which
    // fails it: the rest of the batch is posted.
    store.remove(domain[&N].hash);
    let machine = Machine { forms: store.directory.join("forms-gone"), bound: Some(LoadBound { bound: Duration::ZERO, workers: 4 }), ..Machine::beside(&store.directory) };
    let loader = Loader::start_on(&store.directory, machine, Options::default()).expect("the capability starts");
    let world = Attached::new(&loader, None, &[], &[(P, PARAMETERS)], &[]);
    assert_eq!(world.posted(&loader), vec![of(&domain, D), Arrival::failed(N, REASON_ARTIFACT_GONE), of(&domain, P)]);
}

/// The stand-in background pool runs its actions in the order submitted,
/// and the store port serves a host's read requests among the capability's.
#[test]
fn the_pool_runs_actions_in_the_order_submitted_and_the_port_serves_a_hosts_reads() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let pool = Pool::new(1);
    let handle = pool.handle();
    let (done, finished) = channel();
    for action in 0..50_usize {
        let (order, done) = (order.clone(), done.clone());
        handle.submit(Box::new(move |worker| {
            order.lock().expect("never poisoned").push((action, worker));
            let _ = done.send(());
        }));
    }
    for _ in 0..50 {
        finished.recv().expect("an action ran");
    }
    assert_eq!(*order.lock().expect("never poisoned"), (0..50).map(|action| (action, 0)).collect::<Vec<_>>());
    assert_eq!(handle.queued(), 0);
    // Several workers: every action runs once.
    let pool = Pool::new(4);
    let ran = Arc::new(AtomicU32::new(0));
    for _ in 0..200 {
        let (ran, done) = (ran.clone(), done.clone());
        pool.handle().submit(Box::new(move |worker| {
            assert!(worker < 4);
            ran.fetch_add(1, Ordering::SeqCst);
            let _ = done.send(());
        }));
    }
    for _ in 0..200 {
        finished.recv().expect("an action ran");
    }
    assert_eq!(ran.load(Ordering::SeqCst), 200);
    // A host's request reads one artifact whole from the loose store.
    let store = Loose::new();
    let domain = store.revision(&[(A, PARAMETERS, 1, &[])]);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let (reply, served) = channel();
    let before = Instant::now();
    assert!(loader.port().request(domain[&A].hash, &reply) && loader.port().request(0x99, &reply));
    // Two file threads serve the two requests: either may answer first.
    let mut both = [served.recv().expect("the request is served"), served.recv().expect("the request is served")];
    both.sort_by_key(|served| !served.found);
    let (held, gone) = (both[0], both[1]);
    assert!(held.found && !gone.found);
    assert!(held.thread < 2 && before <= held.requested && held.requested <= held.started);
}

#[test]
fn a_restore_confirms_a_form_at_its_recorded_hash_and_a_replay_reads_it_by_that_hash() {
    let store = Loose::new();
    let (domain, built, own) = with_a_form(&store, true);
    let part_hash = domain[&N].hash;
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    assert_eq!(loader.confirm_form(None, N, part_hash, built), Ok(()));
    assert!(loader.confirm_form(None, N, part_hash, content_hash(&own)).is_err(), "another hash than the content lists for the asset");
    assert!(loader.confirm_form(None, A, domain[&A].hash, built).is_err(), "an asset the content lists no form for");
    // A replay reads the part and the form by the hashes its record names.
    let forms = [(SECOND, Preparer::Stamp)];
    let record = Record::arrivals(3, vec![Arrival::at(N, part_hash).with_form(built)]);
    assert_eq!(loader.replay(None, &record, &forms), Ok(()));
    // The machine's own manifest: the same part, the machine's own form.
    // The store holds the form built once beside it.
    let own_hash = store.put(&own);
    let mut own_domain = domain.clone();
    own_domain.get_mut(&N).expect("an entry").forms[0].hash = own_hash;
    store.place(&own_domain);
    let beside = |options: Options| Loader::start(&store.directory, options).expect("the capability starts");
    let loader = beside(Options::default());
    assert!(loader.confirm_form(None, N, part_hash, built).is_err(), "the manifest the world would stand under holds another form");
    assert_eq!(beside(Options { confirmation_reads_the_store_alone: true, ..Options::default() }).confirm_form(None, N, part_hash, built), Ok(()));
    assert_eq!(loader.replay(None, &record, &forms), Ok(()), "a replay reads by hash, whatever manifest its instance holds");
    assert_eq!(loader.residents().form(part_hash, Some(built)).map(|bytes| content_hash(&bytes)), Some(built));
    // A record that names no form, which a control alone gives: the form is
    // the one the manifest lists, here the machine's own.
    let unnamed = Record::arrivals(3, vec![Arrival::at(N, part_hash)]);
    let loader = beside(Options::default());
    assert_eq!(loader.replay(None, &unnamed, &forms), Ok(()));
    assert_eq!(loader.residents().form(part_hash, None).map(|bytes| content_hash(&bytes)), Some(own_hash));
    // The form built once gone from the store: the replay is refused at
    // that record, naming the hash; so is one whose part is gone.
    store.remove(built);
    let loader = beside(Options::default());
    assert_eq!(loader.replay(None, &record, &forms), Err(built));
    let gone = Record::arrivals(4, vec![Arrival::at(A, 0x77)]);
    assert_eq!(loader.replay(None, &gone, &forms), Err(0x77));
    // Control: the replay takes the form its machine built for the part.
    let taking = beside(Options { local_form_where_the_named_one_cannot_be_had: true, ..Options::default() });
    assert_eq!(taking.replay(None, &record, &forms), Ok(()));
    assert_eq!(taking.residents().form(part_hash, Some(built)).map(|bytes| content_hash(&bytes)), Some(own_hash));
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
    assert_eq!(world.posted(&loader), vec![of(&domain, A)]);
    // An asset of a closure is checked against the entry the table holds for it.
    let table = [held(D, PARAMETERS, Availability::Unavailable, true), held(P, PARAMETERS, Availability::Unavailable, true)];
    world.extract(&[(P, PARAMETERS)], &table, false);
    assert_eq!(world.posted(&loader), vec![Arrival::failed(D, REASON_ANOTHER_TYPE), of(&domain, P)]);
    // The control: no type is checked.
    let unchecked = Loader::start(&store.directory, Options { types_not_checked: true, ..Options::default() }).expect("the capability starts");
    let world = Attached::new(&unchecked, None, &[], &[(A, SECOND)], &[]);
    assert_eq!(world.posted(&unchecked), vec![of(&domain, A)]);
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
    assert_eq!(world.posted(&loader), vec![of(&domain, D)]);
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
    let expected = vec![of(&after, N), of(&after, P), of(&after, Q)];
    assert_eq!(posted, expected, "the new dependency with its parent, the entry with no want, and for no other asset");
    // The text as it stood: the changed assets a want names, and nothing else.
    let (posted, after) = run(Options { moves_leave_entries_without_a_want: true, moves_without_the_closure: true, ..Options::default() });
    assert_eq!(posted, vec![of(&after, P)]);
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
    assert_eq!(world.posted(&loader), vec![Arrival::failed(A, REASON_UNKNOWN_ASSET), of(&before, P)]);
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
        (world.posted(&loader), of(&after, A))
    };
    let (posted, arrival) = run(Options::default());
    assert_eq!(posted, vec![arrival], "the move changes the asset, and the table names it");
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
fn the_answers_to_a_restore_complete_the_batch_of_each_closure_and_a_revision_is_held_until_the_first_tick() {
    let store = Loose::new();
    let lists: &[Dependency] = &[(N, PARAMETERS), (F, PARAMETERS), (E, PARAMETERS), (Q, PARAMETERS)];
    let below: &[Dependency] = &[(D, PARAMETERS)];
    let leaves: [Content; 6] = [(N, PARAMETERS, 1, &[]), (F, PARAMETERS, 1, &[]), (E, PARAMETERS, 1, &[]), (Q, PARAMETERS, 1, &[]), (U, PARAMETERS, 1, &[]), (D, PARAMETERS, 1, &[])];
    let with = |parent: Content<'static>| -> Vec<Content<'static>> { leaves.iter().copied().chain([parent, (R, PARAMETERS, 1, below)]).collect() };
    let at_hand = store.revision(&with((P, PARAMETERS, 2, lists)));
    // The saved table. P is available under a want at another hash, and its
    // part at hand lists N, which the table does not name; F, failed under
    // the want P itself owns; E, failed under the want of a command; and Q,
    // unavailable under no want. O is available with no want and gone from
    // the content at hand. R is available with no want, and its part lists
    // D, which the table does not name. U is wanted and unanswered.
    let host = Owner::World(prototype_core::table::Maker::Source(3));
    let saved = [
        held(E, PARAMETERS, Availability::Failed(REASON_ARTIFACT_GONE), true),
        held(F, PARAMETERS, Availability::Failed(REASON_ARTIFACT_GONE), true),
        held(O, PARAMETERS, Availability::Available(7), false),
        held(P, PARAMETERS, Availability::Available(hash_of(1, &[])), true),
        held(Q, PARAMETERS, Availability::Unavailable, false),
        held(R, PARAMETERS, Availability::Available(at_hand[&R].hash), false),
        held(U, PARAMETERS, Availability::Unavailable, true),
    ];
    // What the query before the first tick gives of the entries the table
    // does not hold available: the wants that stand on each, with their owners.
    let standing = vec![(E, vec![(host, Policy::Optional)]), (F, vec![(Owner::Asset(P), Policy::Required)]), (Q, Vec::new()), (U, vec![(host, Policy::Required)])];
    let restored = |loader: &Loader| {
        let queue = Arc::new(ArrivalQueue::default());
        let attachment = Attachment { queue: queue.clone(), set: None, entries: saved.to_vec(), unanswered: vec![(U, PARAMETERS)], restore: vec![O, P, R], standing: standing.clone(), checks: Vec::new(), forms: Vec::new() };
        Attached { extract: loader.attach(attachment).expect("the world is attached"), queue }
    };
    let answers = [Arrival::failed(O, REASON_UNKNOWN_ASSET).marked(), of(&at_hand, P).marked(), of(&at_hand, R).marked()];
    let with_the_answers = |ordinary: &[AssetId]| {
        let mut batch: Vec<Arrival> = ordinary.iter().map(|asset| of(&at_hand, *asset)).chain(answers).collect();
        batch.sort_by_key(|arrival| arrival.asset);
        batch
    };
    // The narrow form: an ordinary arrival for what a want the core adds
    // from the part of an answer creates (N) or returns to unavailable (E),
    // and for the entry held unavailable under no want that such a part
    // lists (Q). F, failed under its parent's own standing want, receives
    // nothing and stays failed. The part of R adds no want, R holding none:
    // D receives nothing. U is answered as any unanswered want is.
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let world = restored(&loader);
    assert_eq!(world.posted(&loader), with_the_answers(&[E, N, Q, U]));
    // A revision that lands while the world waits is held, then posted as a reload.
    let later = store.revision(&with((P, PARAMETERS, 3, &[])));
    loader.revision_in_place().expect("the revision is read");
    assert!(world.posted(&loader).is_empty());
    assert_eq!(loader.report().held_moves, 1);
    let available = |asset: AssetId| held(asset, PARAMETERS, Availability::Available(at_hand[&asset].hash), true);
    let after_the_first_tick = [available(E), held(F, PARAMETERS, Availability::Failed(REASON_ARTIFACT_GONE), true), available(N), available(P), available(Q), available(U)];
    world.extract(&[], &after_the_first_tick, true);
    assert_eq!(world.posted(&loader), vec![of(&later, P)]);
    store.place(&at_hand);
    // Not taken, the build's former default: what the saved table holds
    // under no want. E, failed under another want, is left out, and D is
    // posted, below an entry that holds no want.
    let former = Loader::start(&store.directory, Options { restore_answers_for_what_no_want_names: true, ..Options::default() }).expect("the capability starts");
    assert_eq!(restored(&former).posted(&former), with_the_answers(&[D, N, Q, U]));
    // Not taken, the wider form: every asset of a closure that the table
    // does not hold available, the entry failed under a standing want included.
    let wider = Loader::start(&store.directory, Options { restore_answers_complete_the_batch: true, ..Options::default() }).expect("the capability starts");
    assert_eq!(restored(&wider).posted(&wider), with_the_answers(&[D, E, F, N, Q, U]));
    // Not taken: the answers and nothing else, beside the unanswered want.
    let literal = Loader::start(&store.directory, Options { restore_answers_do_not_follow_the_closure: true, ..Options::default() }).expect("the capability starts");
    assert_eq!(restored(&literal).posted(&literal), with_the_answers(&[U]));
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
    assert_eq!(no_set.posted(&loader), vec![of(&later, A)]);
    // A want made meanwhile is answered inside the set, and one it cannot
    // answer there is a failed transition.
    under_set.extract(&[(P, PARAMETERS), (N, PARAMETERS)], &table, false);
    assert_eq!(under_set.posted(&loader), vec![Arrival::failed(N, REASON_UNKNOWN_ASSET), of(&set, P)]);
    no_set.extract(&[(P, PARAMETERS), (N, PARAMETERS)], &table, false);
    assert_eq!(no_set.posted(&loader), vec![of(&later, N), of(&later, P)]);
}

#[test]
fn several_worlds_each_receive_their_own_batches_whatever_the_delay() {
    let store = Loose::new();
    let domain = store.revision(&[(D, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, &[(D, PARAMETERS)]), (A, PARAMETERS, 1, &[])]);
    let wanted = |asset: AssetId| [held(asset, PARAMETERS, Availability::Unavailable, true)];
    let own_p = vec![of(&domain, D), of(&domain, P)];
    let own_a = vec![of(&domain, A)];
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
    let at = |asset: AssetId, root: AssetId| of(&domain, asset).for_root(root);
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
    let moved = |asset: AssetId, root: AssetId| of(&later, asset).for_root(root);
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
    assert_eq!(world.posted(&loader), vec![of(&domain, A)]);
    // The control of a mount and of a revision: one arrival per tick, in
    // identifier order, each posted when the host waits before a step.
    let slow = Loader::start(&store.directory, Options { moves_posted_one_per_tick: true, ..Options::default() }).expect("the capability starts");
    let table = [held(A, PARAMETERS, Availability::Available(domain[&A].hash), true), held(D, PARAMETERS, Availability::Available(domain[&D].hash), true)];
    let world = Attached::new(&slow, None, &table, &[], &[]);
    let later = store.revision(&[(A, PARAMETERS, 2, &[]), (D, PARAMETERS, 4, &[])]);
    slow.revision_in_place().expect("the revision is read");
    assert_eq!(world.posted(&slow), vec![of(&later, A)]);
    assert_eq!(world.posted(&slow), vec![of(&later, D)]);
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

#[test]
fn an_arrival_names_the_size_the_manifest_gives_and_a_failure_names_none() {
    let store = Loose::new();
    let domain = store.revision(&[(P, PARAMETERS, 1, &[(D, PARAMETERS)]), (D, PARAMETERS, 1, &[])]);
    let loader = Loader::start(&store.directory, Options::default()).expect("the capability starts");
    let world = Attached::new(&loader, None, &[], &[(P, PARAMETERS), (U, PARAMETERS)], &[]);
    let posted = world.posted(&loader);
    let sizes: Vec<(AssetId, u64)> = posted.iter().map(|arrival| (arrival.asset, arrival.size)).collect();
    assert_eq!(sizes, vec![(D, domain[&D].size), (P, domain[&P].size), (U, 0)], "the size of the canonical part, no prepared form existing");
    assert!(domain[&P].size > domain[&D].size && domain[&D].size == part(1, &[]).len() as u64);
    assert_eq!(posted[2].outcome, Outcome::Failure(REASON_UNKNOWN_ASSET));
}

#[test]
fn a_dependency_failed_for_its_type_is_loaded_under_the_manifests_type_below_a_parent_the_batch_brings() {
    let store = Loose::new();
    let domain = store.revision(&[(P, PARAMETERS, 1, &[(D, PARAMETERS)]), (D, PARAMETERS, 1, &[]), (R, PARAMETERS, 1, &[(P, PARAMETERS)])]);
    // A command wanted D under a wrong type and D failed for it. A want of
    // P, whose part lists D under the manifest's type, then leaves: the
    // core's want will give the entry that type before the arrival is applied.
    let failed = held(D, SECOND, Availability::Failed(REASON_ANOTHER_TYPE), true);
    let table = [failed, held(P, PARAMETERS, Availability::Unavailable, true)];
    let run = |options: Options, table: &[EntryView], left: AssetId| {
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let world = Attached::new(&loader, None, &[], &[], &[]);
        world.extract(&[(left, PARAMETERS)], table, false);
        world.posted(&loader)
    };
    assert_eq!(run(Options::default(), &table, P), vec![of(&domain, D), of(&domain, P)]);
    // Not taken: the dependency fails anew for its type, and the reading
    // taken on such an entry is undone by the batch that brings the want.
    let anew = Options { dependency_failed_for_its_type_fails_anew: true, ..Options::default() };
    assert_eq!(run(anew, &table, P), vec![Arrival::failed(D, REASON_ANOTHER_TYPE), of(&domain, P)]);
    // An entry that is not failed for its type is checked as any asset is.
    let unanswered = [held(D, SECOND, Availability::Unavailable, true), held(P, PARAMETERS, Availability::Unavailable, true)];
    assert_eq!(run(Options::default(), &unanswered, P), vec![Arrival::failed(D, REASON_ANOTHER_TYPE), of(&domain, P)]);
    // Below a parent the table holds available, which the batch does not
    // bring, the core adds no want: the dependency fails anew. The letter of
    // the exception, the other reading, loads it all the same.
    let in_force = [failed, held(P, PARAMETERS, Availability::Available(domain[&P].hash), true), held(R, PARAMETERS, Availability::Unavailable, true)];
    assert_eq!(run(Options::default(), &in_force, R), vec![Arrival::failed(D, REASON_ANOTHER_TYPE), of(&domain, R)]);
    let letter = Options { type_exception_below_a_parent_in_force: true, ..Options::default() };
    assert_eq!(run(letter, &in_force, R), vec![of(&domain, D), of(&domain, R)]);
}

#[test]
fn a_batch_is_complete_against_the_state_extracted_and_never_against_what_was_posted() {
    let store = Loose::new();
    let domain = store.revision(&[(P, PARAMETERS, 1, &[(D, PARAMETERS)]), (D, PARAMETERS, 1, &[]), (Q, PARAMETERS, 1, &[(D, PARAMETERS)])]);
    let run = |options: Options| {
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let world = Attached::new(&loader, None, &[], &[], &[]);
        // The want of P leaves, and its closure is posted.
        world.extract(&[(P, PARAMETERS)], &[held(P, PARAMETERS, Availability::Unavailable, true)], false);
        assert_eq!(world.posted(&loader), vec![of(&domain, D), of(&domain, P)]);
        // The world rejected the group over its budget: P is failed, and D,
        // which the rejected part alone listed, has no entry. A want of Q,
        // whose part lists D too, then leaves.
        let state = [held(P, PARAMETERS, Availability::Failed(prototype_core::table::REASON_OVER_BUDGET), true), held(Q, PARAMETERS, Availability::Unavailable, true)];
        world.extract(&[(Q, PARAMETERS)], &state, false);
        world.posted(&loader)
    };
    assert_eq!(run(Options::default()), vec![of(&domain, D), of(&domain, Q)], "the rejection is learned from the state: the batch holds the dependency again");
    // The control: the capability trusts what it posted, and leaves out an
    // asset the table does not hold. On a gated world the gate would then
    // stay closed for good.
    assert_eq!(run(Options { trusts_what_it_posted: true, ..Options::default() }), vec![of(&domain, Q)]);
}

#[test]
fn under_the_control_that_trusts_what_was_posted_an_asset_counts_as_held_until_it_is_posted_again() {
    let store = Loose::new();
    let domain = store.revision(&[(P, PARAMETERS, 1, &[(D, PARAMETERS)]), (D, PARAMETERS, 1, &[])]);
    let loader = Loader::start(&store.directory, Options { trusts_what_it_posted: true, ..Options::default() }).expect("the capability starts");
    let world = Attached::new(&loader, None, &[], &[], &[]);
    world.extract(&[(P, PARAMETERS)], &[held(P, PARAMETERS, Availability::Unavailable, true)], false);
    assert_eq!(world.posted(&loader), vec![of(&domain, D), of(&domain, P)]);
    // P leaves the manifest: its failure is posted, which ends the trust in
    // P. The trust in D stands.
    let in_force = [held(D, PARAMETERS, Availability::Available(domain[&D].hash), true), held(P, PARAMETERS, Availability::Available(domain[&P].hash), true)];
    world.extract(&[], &in_force, false);
    store.revision(&[(D, PARAMETERS, 1, &[])]);
    loader.revision_in_place().expect("the revision is read");
    assert_eq!(world.posted(&loader), vec![Arrival::failed(P, REASON_UNKNOWN_ASSET)]);
    // P failed and D left the table. A want added on P leaves: it is
    // answered, P being trusted no longer.
    world.extract(&[(P, PARAMETERS)], &[held(P, PARAMETERS, Availability::Unavailable, true)], false);
    assert_eq!(world.posted(&loader), vec![Arrival::failed(P, REASON_UNKNOWN_ASSET)]);
    // The manifest holds P again: its batch leaves D out, which the table
    // no longer holds and which was posted to a hash.
    store.place(&domain);
    loader.revision_in_place().expect("the revision is read");
    assert_eq!(world.posted(&loader), vec![of(&domain, P)]);
}

#[test]
fn the_batch_of_a_revision_is_built_against_the_last_extraction_and_under_a_control_against_the_one_before() {
    let run = |options: Options| {
        let store = Loose::new();
        let first = store.revision(&[(P, PARAMETERS, 1, &[(D, PARAMETERS)]), (D, PARAMETERS, 1, &[])]);
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let table = [held(D, PARAMETERS, Availability::Available(first[&D].hash), true), held(P, PARAMETERS, Availability::Available(first[&P].hash), true)];
        let world = Attached::new(&loader, None, &table, &[], &[]);
        // A revision whose part of P lists nothing.
        let second = store.revision(&[(P, PARAMETERS, 5, &[]), (D, PARAMETERS, 1, &[])]);
        loader.revision_in_place().expect("the revision is read");
        assert_eq!(world.posted(&loader), vec![of(&second, P)]);
        // The tick that applies it ends the want of D, and the tick after
        // takes D out of the table.
        let replaced = held(P, PARAMETERS, Availability::Available(second[&P].hash), true);
        world.extract(&[], &[held(D, PARAMETERS, Availability::Available(first[&D].hash), false), replaced], false);
        world.extract(&[], &[replaced], false);
        // A later revision lists D again.
        let third = store.revision(&[(P, PARAMETERS, 2, &[(D, PARAMETERS)]), (D, PARAMETERS, 1, &[])]);
        loader.revision_in_place().expect("the revision is read");
        (world.posted(&loader), third)
    };
    let (posted, third) = run(Options::default());
    assert_eq!(posted, vec![of(&third, D), of(&third, P)], "the fall is learned from the state: the batch holds the former dependency again");
    // The control: a view older than one tick still holds D available, and
    // the batch leaves it out.
    let (posted, third) = run(Options { moves_built_against_the_table_before_the_last: true, ..Options::default() });
    assert_eq!(posted, vec![of(&third, P)]);
}

impl Attached {
    /// A world whose composition carries an admission check for the type of
    /// parameters, with a bound.
    fn checked(loader: &Loader, entries: &[EntryView], bound: f32) -> Attached {
        let queue = Arc::new(ArrivalQueue::default());
        let checks = vec![(PARAMETERS, AdmissionCheck { bound })];
        let attachment = Attachment { queue: queue.clone(), set: None, entries: entries.to_vec(), unanswered: Vec::new(), restore: Vec::new(), standing: Vec::new(), checks, forms: Vec::new() };
        Attached { extract: loader.attach(attachment).expect("the world is attached"), queue }
    }
}

/// The bound of the checks below: the first parameter of a part of these
/// self-checks is its version, so a version over 5 stands above the bound.
const BOUND: f32 = 5.0;

#[test]
fn a_part_the_admission_check_of_a_world_refuses_is_not_posted_and_fails_a_first_load_alone() {
    // What three worlds receive of a revision of two assets, the new part
    // of P standing above the bound: one that carries the check and holds
    // both, one that holds both and carries none, and one that carries the
    // check and wants P after the revision. Then what the first receives of
    // a later revision whose part of P stands within the bound.
    let run = |options: Options| {
        let store = Loose::new();
        let before = store.revision(&[(A, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, &[])]);
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let table = [held(A, PARAMETERS, Availability::Available(before[&A].hash), true), held(P, PARAMETERS, Availability::Available(before[&P].hash), true)];
        let checked = Attached::checked(&loader, &table, BOUND);
        let unchecked = Attached::new(&loader, None, &table, &[], &[]);
        let late = Attached::checked(&loader, &[], BOUND);
        let second = store.revision(&[(A, PARAMETERS, 2, &[]), (P, PARAMETERS, 9, &[])]);
        loader.revision_in_place().expect("the revision is read");
        let (to_checked, to_unchecked) = (checked.posted(&loader), unchecked.posted(&loader));
        late.extract(&[(P, PARAMETERS)], &[held(P, PARAMETERS, Availability::Unavailable, true)], false);
        let to_late = late.posted(&loader);
        let withheld = loader.report().withheld;
        let third = store.revision(&[(A, PARAMETERS, 2, &[]), (P, PARAMETERS, 3, &[])]);
        loader.revision_in_place().expect("the revision is read");
        (to_checked, to_unchecked, to_late, withheld, checked.posted(&loader), second, third)
    };
    let (to_checked, to_unchecked, to_late, withheld, later, second, third) = run(Options::default());
    assert_eq!(to_checked, vec![of(&second, A)], "the batch is posted without the asset the world refuses, and the rest of the revision is applied");
    assert_eq!(to_unchecked, vec![of(&second, A), of(&second, P)], "the check is a world's: two worlds apply two batches from one revision");
    assert_eq!(to_late, vec![Arrival::failed(P, REASON_ADMISSION_REFUSED)], "a failure is posted for an asset the world does not hold available, a first load");
    let refused = |world: u32, cause: Cause| Withheld { world, asset: P, hash: second[&P].hash, cause };
    assert_eq!(withheld, vec![refused(0, Cause::RefusedReplacement), refused(2, Cause::RefusedFirstLoad)]);
    assert_eq!(later, vec![of(&third, P)]);
    // No check is run.
    let (to_checked, _, to_late, withheld, _, second, _) = run(Options { no_admission_check_run: true, ..Options::default() });
    assert_eq!((to_checked, to_late), (vec![of(&second, A), of(&second, P)], vec![of(&second, P)]));
    assert!(withheld.is_empty());
    // A part one world of the instance refuses is posted to no world.
    let (_, to_unchecked, _, withheld, _, second, _) = run(Options { a_part_one_world_refuses_is_posted_to_no_world: true, ..Options::default() });
    assert_eq!(to_unchecked, vec![of(&second, A)]);
    assert!(withheld.contains(&Withheld { world: 1, asset: P, hash: second[&P].hash, cause: Cause::RefusedByAnotherWorld }));
    // The batch is held back while the world refuses one of its assets, and
    // the next move owes the world both.
    let (to_checked, _, _, withheld, later, second, third) = run(Options { move_held_back_while_an_asset_is_refused: true, ..Options::default() });
    assert!(to_checked.is_empty());
    let held_back = |asset: AssetId| Withheld { world: 0, asset, hash: second[&asset].hash, cause: Cause::HeldBackFor(P) };
    assert_eq!(withheld[..2], [held_back(A), held_back(P)]);
    assert_eq!(later, vec![of(&third, A), of(&third, P)]);
    // A replacement the world refuses is posted to it as a failure.
    let (to_checked, _, _, withheld, _, second, _) = run(Options { refused_replacement_posted_as_a_failure: true, ..Options::default() });
    assert_eq!(to_checked, vec![of(&second, A), Arrival::failed(P, REASON_ADMISSION_REFUSED)]);
    assert_eq!(withheld[0], Withheld { world: 0, asset: P, hash: second[&P].hash, cause: Cause::RefusedAndPostedAsAFailure });
}

#[test]
fn an_asset_that_lists_a_refused_one_is_posted_and_the_closure_of_a_refused_part_is_not_followed() {
    // P lists D, and the world holds both. A revision changes both: the new
    // part of D stands above the bound, and the new part of P within it.
    let lists: &[Dependency] = &[(D, PARAMETERS)];
    let run = |options: Options| {
        let store = Loose::new();
        let before = store.revision(&[(D, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, lists)]);
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let table = [held(D, PARAMETERS, Availability::Available(before[&D].hash), true), held(P, PARAMETERS, Availability::Available(before[&P].hash), true)];
        let world = Attached::checked(&loader, &table, BOUND);
        let after = store.revision(&[(D, PARAMETERS, 9, &[]), (P, PARAMETERS, 2, lists)]);
        loader.revision_in_place().expect("the revision is read");
        (world.posted(&loader), loader.report().withheld, after)
    };
    let (posted, withheld, after) = run(Options::default());
    assert_eq!(posted, vec![of(&after, P)], "a part names its dependencies by identifier: the asset is posted, and reads the version in force");
    assert_eq!(withheld, vec![Withheld { world: 0, asset: D, hash: after[&D].hash, cause: Cause::RefusedReplacement }]);
    let (posted, withheld, after) = run(Options { dependent_of_a_refused_asset_withheld: true, ..Options::default() });
    assert!(posted.is_empty(), "the control: the asset is withheld with the one its part lists");
    assert!(withheld.contains(&Withheld { world: 0, asset: P, hash: after[&P].hash, cause: Cause::ListsARefusedAsset(D) }));
    // A changed part the world refuses that lists a dependency the table
    // does not hold: the batch is posted without the part, and holds no
    // arrival for the new dependency. Under the other reading it holds one,
    // which no want names.
    let with_a_new_dependency: &[Dependency] = &[(D, PARAMETERS), (N, PARAMETERS)];
    let run = |options: Options| {
        let store = Loose::new();
        let before = store.revision(&[(D, PARAMETERS, 1, &[]), (N, PARAMETERS, 1, &[]), (P, PARAMETERS, 1, lists)]);
        let loader = Loader::start(&store.directory, options).expect("the capability starts");
        let table = [held(D, PARAMETERS, Availability::Available(before[&D].hash), true), held(P, PARAMETERS, Availability::Available(before[&P].hash), true)];
        let world = Attached::checked(&loader, &table, BOUND);
        let after = store.revision(&[(D, PARAMETERS, 1, &[]), (N, PARAMETERS, 1, &[]), (P, PARAMETERS, 9, with_a_new_dependency)]);
        loader.revision_in_place().expect("the revision is read");
        (world.posted(&loader), after)
    };
    assert!(run(Options::default()).0.is_empty());
    let (posted, after) = run(Options { closure_of_a_refused_part_posted: true, ..Options::default() });
    assert_eq!(posted, vec![of(&after, N)]);
}
