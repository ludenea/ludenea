//! The scenarios of one claim: a named content set holds under a moved
//! manifest. Throwaway, never product code.
//!
//! One instance holds a world created under a content set and a world that
//! names none; a second instance, whose manifest never moves, holds the same
//! world under the same set. A revision, the removal of an artifact, a mount
//! and an unmount move the first instance's manifest under them.

use std::collections::BTreeSet;
use std::sync::Arc;

use asset_loader::{Attachment, Loader, Options};
use prototype_core::table::{Policy, Record};
use prototype_core::world::{Links, World, WorldConfig, save_header};

use crate::oracle::{Expected, parse};
use crate::run::{Act, BASE, Claim, Driven, Family, Outcome, Settings, Variant, config, content, import, judge, late_extract, replay};

pub const NAME: &str = "a named content set holds under a moved manifest";
const MOVED: &str = "named-set/moved-manifest";
const RECORDING: &str = "named-set/recording-replays-under-a-set";
const SET: [&str; 5] = ["K1.1", "K2.1", "X2.1", "X3.1", "Y.1"];
const LATER: [&str; 4] = ["K1.2", "X2.2", "X3.2", "N.1"];
const PACKAGE: [&str; 2] = ["K2.9", "Y.9"];
const WANTS: [(u32, &str); 4] =
    [(9, "want X2 source:host required"), (11, "want Y source:host required"), (13, "want N source:host required"), (15, "want X3 source:host required")];

pub fn family(settings: &Settings) -> Family {
    let contents = [
        content("K1", 1),
        content("K1", 2),
        content("K2", 1),
        content("K2", 9),
        content("N", 1),
        content("X2", 1),
        content("X2", 2),
        content("X3", 1),
        content("X3", 2),
        content("Y", 1),
        content("Y", 9),
    ];
    Family::new(&["K1", "K2", "N", "X2", "X3", "Y"], &["K1", "K2", "X2", "Y"], &contents, &[], settings)
}

fn composed(settings: &Settings, family: &Family, workers: usize, set: Option<u64>) -> WorldConfig {
    let wants = [(family.asset("K1"), Policy::Required), (family.asset("K2"), Policy::Required)];
    let mut config = config(settings, workers, &family.names).wanting(BASE, &wants);
    config.content_set = set;
    config
}

/// A world attached to a loading capability, ready to be driven.
fn attached(loader: &Loader, config: WorldConfig) -> Driven {
    let (extract, slot) = late_extract();
    let mut links = Links::new(loader.residents());
    links.extract = Some(extract);
    let queue = links.queue.clone();
    let set = config.content_set;
    let world = World::new(config, Vec::new(), links);
    let attachment = Attachment { queue: queue.clone(), set, unanswered: world.unanswered(), named: world.named(), restore: Vec::new() };
    let _ = slot.set(loader.attach(attachment).expect("the world is attached"));
    Driven::new(world, queue)
}

/// The three worlds of the moved manifest, driven together to tick 22.
fn moved(settings: &Settings, family: &Family, workers: usize, options: Options) -> [(&'static str, Driven, Option<bool>); 3] {
    let directory = settings.directory(&format!("{MOVED}-{workers}"));
    let content_root = directory.join("content");
    family.write_sources(&content_root, &SET);
    let (store, first) = import(&content_root, &directory.join("store"));
    family.write_sources(&directory.join("reference-content"), &SET);
    let (reference_store, reference_first) = import(&directory.join("reference-content"), &directory.join("reference-store"));
    assert_eq!(first.domain_hash, reference_first.domain_hash, "the two instances start under one content set");
    family.write_sources(&directory.join("package-content"), &PACKAGE);
    let (package_store, package_revision) = import(&directory.join("package-content"), &directory.join("package-store"));
    let entries: BTreeSet<_> = package_revision.domain.keys().copied().collect();
    let package = asset_pipeline::package(&package_store, &package_revision.domain, &entries).expect("the package is built");

    let loader = Loader::start(&directory.join("store"), options).expect("the loading capability starts");
    let reference = Loader::start(&directory.join("reference-store"), Options::default()).expect("the second instance starts");
    let set = Some(first.domain_hash);
    let mut under_set = attached(&loader, composed(settings, family, workers, set));
    let mut no_set = attached(&loader, composed(settings, family, workers, None));
    let mut on_reference = attached(&reference, composed(settings, family, workers, set));

    let gone = family.hash("X3.1");
    let acts: Vec<(u32, Act)> = WANTS.iter().map(|(before, text)| (*before, Act::Post(text))).collect();
    for tick in 1..=22 {
        match tick {
            4 => {
                family.write_sources(&content_root, &LATER);
                import(&content_root, &directory.join("store"));
                loader.revision_in_place().expect("the later revision is read");
            }
            6 => {
                store.remove_object(gone).expect("the artifact is in the loose store");
                reference_store.remove_object(gone).expect("the artifact is in the second loose store");
            }
            7 => {
                loader.mount("package", package.clone(), true).expect("the package mounts");
            }
            19 => loader.unmount("package").expect("the package unmounts"),
            _ => {}
        }
        under_set.tick(tick, &acts, &family.names, &|| loader.settle());
        no_set.tick(tick, &acts, &family.names, &|| loader.settle());
        on_reference.tick(tick, &acts, &family.names, &|| reference.settle());
    }
    let other = if workers == 1 { 4 } else { 1 };
    let mut worlds = [("under-set", under_set, set, loader.residents()), ("no-set", no_set, None, loader.residents()), ("reference", on_reference, set, reference.residents())]
        .map(|(name, mut driven, set, residents)| {
            driven.finish();
            let replays = driven.observed.stuck.is_none().then(|| {
                let world = |records: Vec<Record>| World::new(composed(settings, family, other, set), records, Links::new(residents.clone()));
                replay(&driven.observed.log, 22, world) == driven.observed.state
            });
            (name, driven, replays)
        });
    // The refusals outside the domain are no part of a world's record.
    let _ = loader.report();
    worlds.sort_by_key(|(name, _, _)| *name != "under-set");
    worlds
}

/// A recording made in a world that names no set, over content that does
/// not move, replayed in a world created under the set's hash.
fn recording(settings: &Settings, family: &Family, workers: usize, set_under_the_hash: bool) -> (Driven, Driven, bool) {
    let directory = settings.directory(&format!("{RECORDING}-{workers}"));
    family.write_sources(&directory.join("content"), &SET);
    let (_, first) = import(&directory.join("content"), &directory.join("store"));
    let loader = Loader::start(&directory.join("store"), Options::default()).expect("the loading capability starts");
    let mut recorded = attached(&loader, composed(settings, family, workers, None));
    recorded.drive(10, &[(4, Act::Post("want X2 source:host optional"))], &family.names, &|| loader.settle());
    let mut config = composed(settings, family, if workers == 1 { 4 } else { 1 }, Some(first.domain_hash));
    config.set_under_the_hash = set_under_the_hash;
    let links = Links::new(loader.residents() as Arc<_>);
    let queue = links.queue.clone();
    let mut replayed = Driven::new(World::new(config, recorded.observed.log.clone(), links), queue);
    replayed.drive(10, &[], &family.names, &|| ());
    let header = save_header(&replayed.world.save()).expect("a save the world wrote has a header");
    (recorded, replayed, header.content_set == Some(first.domain_hash))
}

pub fn run(settings: &Settings, family: &Family, oracles: &str) -> Claim {
    let mut claim = Claim { name: NAME, ..Claim::default() };
    let stated = parse(oracles, &family.names).unwrap_or_else(|error| panic!("the oracle of the named set does not read: {error}"));
    let nothing = Expected::default();
    let expected = |scenario: &str, world: &str| stated.get(scenario).and_then(|worlds| worlds.get(world)).unwrap_or(&nothing).clone();
    for &workers in &settings.workers {
        let worlds = moved(settings, family, workers, Options::default());
        let reference_state = worlds.iter().find(|(name, _, _)| *name == "reference").map(|(_, driven, _)| driven.observed.state.clone()).unwrap_or_default();
        for (name, driven, replays) in &worlds {
            let mut differences = judge(settings, &family.names, &expected(MOVED, name), &driven.observed, None, 22);
            if *name == "under-set" && driven.observed.state != reference_state {
                differences.push("the state hashes of the world under the set are not those of the same world on the instance whose manifest never moved".to_string());
            }
            let mut outcome = Outcome::new(MOVED, name, workers, &driven.observed, differences);
            outcome.replays = *replays;
            claim.outcomes.push(outcome);
        }
        let (recorded, replayed, header_holds_the_set) = recording(settings, family, workers, false);
        let mut differences = judge(settings, &family.names, &expected(RECORDING, "world"), &recorded.observed, None, 10);
        differences.extend(judge(settings, &family.names, &expected(RECORDING, "world"), &replayed.observed, None, 10).into_iter().map(|text| format!("under the set: {text}")));
        if recorded.observed.state != replayed.observed.state {
            differences.push("the recording does not replay under the set to the same state hashes".to_string());
        }
        if !header_holds_the_set {
            differences.push("the header of a save of the world under the set does not hold the set's hash".to_string());
        }
        claim.outcomes.push(Outcome::new(RECORDING, "world", workers, &recorded.observed, differences));
    }
    let options = Options { current_manifest_for_sets: true, ..Options::default() };
    let worlds = moved(settings, family, 4, options);
    let (_, driven, _) = &worlds[0];
    let differences = judge(settings, &family.names, &expected(MOVED, "under-set"), &driven.observed, None, 22);
    claim.variants.push(Variant {
        scenario: MOVED.to_string(),
        name: "the instance's manifest in place of the set's",
        control: true,
        differs: !differences.is_empty(),
        first_difference: differences.first().cloned().unwrap_or_default(),
    });
    let (recorded, replayed, _) = recording(settings, family, 4, true);
    let differs = recorded.observed.state != replayed.observed.state;
    claim.variants.push(Variant {
        scenario: RECORDING.to_string(),
        name: "the set's hash under the state hash",
        control: true,
        differs,
        first_difference: if differs { "the recording does not replay under the set to the same state hashes".to_string() } else { String::new() },
    });
    claim
}
