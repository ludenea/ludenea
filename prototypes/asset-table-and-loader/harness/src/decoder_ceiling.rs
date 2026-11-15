//! A decoder at load under the host's ceiling: a row judged on a ceiling
//! and on the kind of a transition, with no oracle and no control.
//! Throwaway, never product code.
//!
//! The family `decoder-ceiling` of the sheet of the stand-in stage. One
//! world with an empty table and a live loading capability over a loose
//! store that holds nothing it wants. Between tick 1 and tick 2 a package
//! from elsewhere is mounted as untrusted, without leave to override, under
//! a ceiling of 1 048 576 bytes; its artifacts are stored in blocks of the
//! stand-in codec `runs`. It holds three assets, each wanted as required in
//! turn:
//!
//! - `W`, an honest part of 65 536 bytes, declared so everywhere: a load the
//!   ceiling allows;
//! - `Y`, whose manifest slice, index and block header each declare 4 096
//!   bytes, and whose run stream of 384 bytes expands to 8 388 480 bytes: the
//!   payload that expands beyond what it declares;
//! - `Z`, an honest part of 4 194 304 bytes, four times the ceiling, declared
//!   so everywhere: the declared size above the ceiling.
//!
//! For each load, a counting allocator scoped to the threads that read,
//! expand, verify and validate the artifact measures the largest single
//! allocation and the largest count of bytes held at one moment, and the
//! harness reads the transition the world receives in the record of the tick
//! after the want. The row is refuted by an allocation above the ceiling, or
//! by a load of `Y` or of `Z` that is not a failed transition.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use asset_loader::{Attachment, BlockedMount, Loader, Options};
use asset_pipeline::Source;
use content_store::runs::{self, Stored};
use content_store::{Domain, DomainEntry, content_hash};
use prototype_core::table::AssetType::Parameters;
use prototype_core::table::{AssetId, Availability, Command, Maker, Outcome, Owner, Part, Policy, RepulsionTable, decode_log, encode_log, part_bytes, part_len, reason};
use prototype_core::world::{Extraction, Links, Mode, Size, StepResult, Wait, World, WorldConfig};
use reference_workload::{Json, SplitMix64};

use crate::magnitude::{arguments_or_exit, finish, head, profile_and_size};
use crate::run::import;
use crate::scoped_allocator::{self, Measured};

/// The host's ceiling, a setting of the mount: the most bytes the decoder
/// may hold allocated to expand one artifact.
pub const CEILING: u64 = 1_048_576;
/// The sizes the three assets declare, in bytes.
const SIZE_OF_W: usize = 65_536;
const SIZE_OF_Y: usize = 4_096;
const SIZE_OF_Z: usize = 4 * CEILING as usize;
/// The run stream of the one block of `Y`: 128 pairs, each of the count
/// 65 535 and of the byte zero.
const RUNS_OF_Y: usize = 128;
const HOST: Owner = Owner::World(Maker::Source(3));
const MOUNT: &str = "from-elsewhere";
/// The package is mounted between tick 1 and tick 2, and the world is
/// stepped to tick 10.
const MOUNTED_BEFORE_TICK: u32 = 2;
const LAST_TICK: u32 = 10;
/// The three cases: the name, the asset, and the tick whose record wants it.
const CASES: [(&str, &str, u32); 3] = [("within", "W", 3), ("expands-beyond-its-declaration", "Y", 5), ("declared-above-the-ceiling", "Z", 7)];
/// What the decoder holds and where it stops, in words, for the report.
const THE_DECODER: &str = "to expand one artifact the decoder holds the buffer of the artifact, of the size the index declares, and the run stream of one block at a time; \
     it checks each against the ceiling before it reserves it, refuses before any read an artifact whose declared size stands above the ceiling, \
     refuses before reading its stream a block whose header declares more than a block holds, more than its artifact has left, or a stream longer than its declared length can need, \
     and stops a run that would expand a block beyond what its header declares; the bytes it expands are then verified against their hash and validated";

/// The content of the family: the three assets and the package that holds them.
struct Content {
    /// `W`, `Y` and `Z`, in the order of the cases.
    assets: [AssetId; 3],
    /// The size each declares in the slice of the manifest and in the index.
    declared: [u64; 3],
    /// The hash the slice of the manifest names for each.
    hashes: [u64; 3],
    package: Vec<u8>,
}

/// Makes the content. `W` is imported by the stand-in pipeline, in a store
/// of the producer's own, and stored honestly; `Y` is crafted by hand under
/// the hash of an honest part of the size it declares, which the package
/// does not hold; `Z` is an honest part padded with zero bytes, stored
/// honestly.
fn content(directory: &Path) -> Content {
    let mut stream = SplitMix64::new(0x6465_636f_6465_7200);
    let mut draw = || (u128::from(stream.next()) << 64) | u128::from(stream.next() | 1);
    let assets = [draw(), draw(), draw()];
    let source = |asset: AssetId, salt: u32, size: usize| Source { asset, asset_type: Parameters, strength: 24.0 + salt as f32, radius: 8.0, salt, size: Some(size), manifest_size: None, dependencies: Vec::new() };
    let parameters = || Parameters.stable_name().to_string();

    // W: an honest part padded by the stand-in pipeline.
    let producer = directory.join("producer");
    fs::create_dir_all(producer.join("content")).expect("the producer's content root is created");
    fs::write(producer.join("content").join("W.asset"), source(assets[0], 1, SIZE_OF_W).text()).expect("the source of W is written");
    let (cache, _) = import(&producer.join("content"), &producer.join("store"));
    let of_w = cache.revision().expect("the producer's revision is in place").domain[&assets[0]].clone();
    let w = cache.object(of_w.hash).expect("the producer's store holds the part of W");
    assert_eq!((w.len(), of_w.size), (SIZE_OF_W, SIZE_OF_W as u64), "W is declared as it is");

    // Y: the slice, the index and the header of its one block each declare
    // 4 096 bytes, under the hash of an honest part of that size.
    let honest_y = source(assets[1], 2, SIZE_OF_Y).part();
    let of_y = DomainEntry { asset_type: parameters(), hash: content_hash(&honest_y), size: SIZE_OF_Y as u64, dependencies: Vec::new(), forms: Vec::new() };
    let runs_of_zeros: Vec<u8> = (0..RUNS_OF_Y).flat_map(|_| [0xff, 0xff, 0]).collect();
    let stored_y = Stored { hash: of_y.hash, declared: of_y.size, blocks: vec![runs::block(SIZE_OF_Y as u32, &runs_of_zeros)] };

    // Z: an honest part padded with zero bytes to four times the ceiling.
    let listed_nothing = part_len(&[], 0);
    let mut z = part_bytes(RepulsionTable { strength: 27.0, radius: 8.0 }, &[], SIZE_OF_Z - listed_nothing, 3);
    z[listed_nothing..].fill(0);
    assert!(Part::decode(&z).is_ok(), "Z is a valid part");
    let of_z = DomainEntry { asset_type: parameters(), hash: content_hash(&z), size: z.len() as u64, dependencies: Vec::new(), forms: Vec::new() };
    let stored_z = Stored::honest(&z);
    assert_eq!(stored_z.blocks.len(), SIZE_OF_Z / runs::BLOCK, "each block of Z honestly declares a whole block");

    let declared = [of_w.size, of_y.size, of_z.size];
    let hashes = [of_w.hash, of_y.hash, of_z.hash];
    let slice: Domain = assets.into_iter().zip([of_w, of_y, of_z]).collect();
    let package = runs::build(&slice, &[Stored::honest(&w), stored_y, stored_z]);
    Content { assets, declared, hashes, package }
}

/// One load: a case, at one worker count.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: &'static str,
    pub asset: &'static str,
    pub workers: usize,
    /// The size the slice of the manifest and the index declare.
    pub declared: u64,
    /// What the counting allocator measured on the threads that read,
    /// expand, verify and validate the artifact, from the first read of a
    /// block to the posting of the arrival.
    pub measured: Measured,
    /// The most bytes the decoder held reserved, by its own account, and
    /// how it ended, in its words.
    pub decoder_held: u64,
    pub decoder_says: String,
    /// The transition the world receives for the asset in the record of
    /// the tick after its want: to a hash, failed with a reason, or none.
    pub transition: Option<Outcome>,
    /// Whether the step of that tick names the asset as a required asset
    /// that failed, and whether the world then steps.
    pub step_names_the_asset: bool,
    pub world_steps: bool,
    /// Whether the table holds the asset available at the last tick.
    pub available_at_the_end: bool,
}

impl Case {
    pub fn failed(&self) -> bool {
        matches!(self.transition, Some(Outcome::Failure(_)))
    }

    /// No allocation above the ceiling: neither one block nor what was held
    /// at one moment, every block being accounted for.
    pub fn under_the_ceiling(&self) -> bool {
        self.measured.complete && self.measured.largest as u64 <= CEILING && self.measured.held_at_most as u64 <= CEILING
    }

    /// The reason the build names for a failed transition: its short name
    /// and what it says.
    pub fn reason(&self) -> Option<(&'static str, &'static str)> {
        match self.transition {
            Some(Outcome::Failure(code)) => Some(reason(code).map_or(("unknown", "a reason the list does not hold"), |reason| (reason.name, reason.text))),
            _ => None,
        }
    }

    fn transition_in_words(&self) -> &'static str {
        match self.transition {
            Some(Outcome::Hash(_)) => "to a hash",
            Some(Outcome::Failure(_)) => "failed",
            None => "none",
        }
    }
}

/// What one world gave.
pub struct Played {
    pub cases: Vec<Case>,
    /// Whether the package mounted as stated, the world ran to its last
    /// tick, and the load the ceiling allows put its asset in force.
    pub as_stated: bool,
    /// Whether the input log, read back from its bytes, replays the run at
    /// another worker count to the same state hashes.
    pub replays: bool,
}

type Extract = Arc<dyn Fn(Extraction) + Send + Sync>;

/// The extraction of a world that is attached to its loading capability
/// after it is made.
fn late_extract() -> (Extract, Arc<OnceLock<Extract>>) {
    let slot: Arc<OnceLock<Extract>> = Arc::new(OnceLock::new());
    let filled = slot.clone();
    let extract = move |extraction: Extraction| {
        if let Some(extract) = filled.get() {
            extract(extraction);
        }
    };
    (Arc::new(extract), slot)
}

/// Plays the family once: one world, the package mounted between tick 1 and
/// tick 2, the three wants, ten ticks. The host waits for the loading
/// capability before each step.
pub fn play(directory: &Path, size: Size, workers: usize, options: Options) -> Played {
    let _ = fs::remove_dir_all(directory);
    fs::create_dir_all(directory.join("content")).expect("a scratch directory is created");
    let content = content(directory);
    let package = directory.join("from-elsewhere.package");
    fs::write(&package, &content.package).expect("the package is written");
    // The loose store, which its host trusts: an empty revision, so that it
    // holds nothing the scenario wants.
    import(&directory.join("content"), &directory.join("store"));
    let loader = Loader::start(&directory.join("store"), options).expect("the loading capability starts");

    let (extract, slot) = late_extract();
    let mut links = Links::new(loader.residents());
    links.extract = Some(extract);
    let queue = links.queue.clone();
    let config = WorldConfig::new(size, workers, Mode::Identifiers, vec![content.assets[0]]);
    let mut world = World::new(config.clone(), Vec::new(), links);
    let attachment = Attachment { queue, set: None, entries: world.entries(), unanswered: world.unanswered(), restore: Vec::new(), standing: Vec::new(), checks: Vec::new() };
    let _ = slot.set(loader.attach(attachment).expect("the world is attached"));
    let mut as_stated = world.entries().is_empty();

    let mut measured: [Option<Measured>; 3] = [None; 3];
    let mut steps: Vec<StepResult> = Vec::new();
    for tick in 1..=LAST_TICK {
        if tick == MOUNTED_BEFORE_TICK {
            let mount = BlockedMount { may_override: false, ceiling: CEILING, watch: Some(Arc::new(scoped_allocator::enter)) };
            let mounted = loader.mount_blocked(MOUNT, &package, mount).expect("the package mounts");
            let mut added = mounted.added.clone();
            added.sort_unstable();
            let mut stated = content.assets.to_vec();
            stated.sort_unstable();
            as_stated &= added == stated && mounted.overridden.is_empty() && mounted.refused.is_empty();
        }
        // A want of this tick leaves the world at its end: the load then
        // runs on the threads of the capability, which enter the scope
        // opened here when they turn to the artifact.
        let wanted = CASES.iter().position(|(_, _, at)| *at == tick);
        let scope = wanted.map(|case| {
            world.post(Command::Want { asset: content.assets[case], asset_type: Parameters, owner: HOST, policy: Policy::Required });
            scoped_allocator::open()
        });
        loader.settle();
        steps.push(world.step(1, Wait::No).expect("the world does not fail"));
        if let (Some(case), Some(scope)) = (wanted, scope) {
            // The arrival is posted once the capability has nothing left to post.
            loader.settle();
            measured[case] = Some(scope.close());
        }
    }
    as_stated &= world.tick() == LAST_TICK;

    let decoded = loader.decoded(MOUNT);
    let mut cases = Vec::new();
    for (case, (name, asset, wanted_at)) in CASES.into_iter().enumerate() {
        let id = content.assets[case];
        let after = wanted_at + 1;
        let transition = world.log().iter().find(|record| record.tick == after).and_then(|record| record.arrivals.iter().find(|arrival| arrival.asset == id)).map(|arrival| arrival.outcome);
        let step = &steps[after as usize - 1];
        let account = decoded.iter().find(|decoded| decoded.hash == content.hashes[case]);
        cases.push(Case {
            name,
            asset,
            workers,
            declared: content.declared[case],
            measured: measured[case].expect("each case is measured"),
            decoder_held: account.map_or(0, |decoded| decoded.held),
            decoder_says: match account {
                None => "the decoder was not asked for it".to_string(),
                Some(decoded) => decoded.not_expanded.map_or("expanded", |why| why.text()).to_string(),
            },
            transition,
            step_names_the_asset: step.failed_required.iter().any(|(failed, _)| *failed == id),
            world_steps: step.ticks_ran == 1,
            available_at_the_end: matches!(world.entry(id), Some((Availability::Available(_), _))),
        });
    }
    // The load the ceiling allows puts its asset in force, at the hash and
    // of the size the package declares.
    as_stated &= cases[0].transition == Some(Outcome::Hash(content.hashes[0])) && cases[0].available_at_the_end && world.size(content.assets[0]) == Some(content.declared[0]);

    // A replay of the run: its input log, read back from its bytes, at
    // another worker count, the parts read from the same resident store.
    let records = decode_log(&encode_log(world.log())).expect("a log the world wrote decodes");
    let mut replayed = World::new(WorldConfig { workers: if workers == 1 { 4 } else { 1 }, ..config }, records, Links::new(loader.residents()));
    let replays = replayed.step(LAST_TICK, Wait::No).is_ok_and(|result| result.ticks_ran == LAST_TICK) && replayed.state_hashes() == world.state_hashes();
    drop(loader);
    let _ = fs::remove_dir_all(directory);
    Played { cases, as_stated, replays }
}

/// What the family gave over its repetitions.
pub struct DecoderCeiling {
    /// Per repetition, the cases of each worker count.
    pub repetitions: Vec<Vec<Case>>,
    pub as_stated: bool,
    pub replays: bool,
}

impl DecoderCeiling {
    fn cases(&self) -> impl Iterator<Item = &Case> {
        self.repetitions.iter().flatten()
    }

    /// The first half of the refutation: an allocation above the ceiling,
    /// in any case.
    pub fn an_allocation_above_the_ceiling(&self) -> bool {
        self.cases().any(|case| !case.under_the_ceiling())
    }

    /// The second half: a load of `Y` or of `Z` that is not a failed
    /// transition.
    pub fn a_refused_load_that_is_not_a_failed_transition(&self) -> bool {
        self.cases().any(|case| case.name != "within" && !case.failed())
    }

    /// Whether the row holds on what was measured.
    pub fn holds(&self) -> bool {
        !self.an_allocation_above_the_ceiling() && !self.a_refused_load_that_is_not_a_failed_transition() && self.as_stated && self.replays
    }
}

/// Runs the family: `repetitions` times, at each worker count.
pub fn run(repetitions: usize, worker_counts: &[usize], size: Size, scratch: &Path) -> DecoderCeiling {
    let mut out = DecoderCeiling { repetitions: Vec::new(), as_stated: true, replays: true };
    for repetition in 0..repetitions {
        let mut cases = Vec::new();
        for &workers in worker_counts {
            let played = play(&scratch.join(format!("repetition-{repetition}-{workers}-workers")), size, workers, Options::default());
            out.as_stated &= played.as_stated;
            out.replays &= played.replays;
            cases.extend(played.cases);
        }
        out.repetitions.push(cases);
    }
    out
}

/// Writes what the family gave: the facts, the summary and the full log.
pub fn report(row: &DecoderCeiling, repetitions: usize, small: bool) -> (Json, String, String) {
    let (mut out, mut summary, mut log) = (Json::new(), String::new(), String::new());
    out.object_start();
    head(&mut out, "decoder-ceiling", repetitions, small);
    out.u64("ceiling_bytes", CEILING);
    out.string("the_decoder", THE_DECODER);
    out.bool("an_allocation_above_the_ceiling", row.an_allocation_above_the_ceiling());
    out.bool("a_load_of_y_or_of_z_that_is_not_a_failed_transition", row.a_refused_load_that_is_not_a_failed_transition());
    out.bool("the_world_and_the_mount_as_stated", row.as_stated);
    out.bool("every_input_log_replays", row.replays);
    out.bool("holds", row.holds());
    let _ = writeln!(log, "== a decoder at load under the host's ceiling of {CEILING} bytes");
    let _ = writeln!(log, "   {THE_DECODER}");
    out.key("cases");
    out.array_start();
    for (repetition, cases) in row.repetitions.iter().enumerate() {
        for case in cases {
            let (reason, says) = case.reason().unwrap_or(("none", "the load did not fail"));
            out.object_start();
            out.u64("repetition", repetition as u64 + 1);
            out.u64("workers", case.workers as u64);
            out.string("case", &format!("decoder-ceiling/{}", case.name));
            out.string("asset", case.asset);
            out.u64("declared_bytes", case.declared);
            out.u64("largest_single_allocation_bytes", case.measured.largest as u64);
            out.u64("largest_bytes_held_at_one_moment", case.measured.held_at_most as u64);
            out.bool("every_allocation_accounted_for", case.measured.complete);
            out.u64("ceiling_bytes", CEILING);
            out.bool("under_the_ceiling", case.under_the_ceiling());
            out.u64("bytes_the_decoder_held_by_its_own_account", case.decoder_held);
            out.string("the_decoder_says", &case.decoder_says);
            out.string("transition", case.transition_in_words());
            out.string("reason", reason);
            out.string("reason_in_words", says);
            out.bool("the_step_names_the_asset", case.step_names_the_asset);
            out.bool("the_world_then_steps", case.world_steps);
            out.bool("available_at_the_last_tick", case.available_at_the_end);
            out.object_end();
            let _ = writeln!(
                log,
                "  repetition {} {} workers decoder-ceiling/{:<31} {} declares {:>7} bytes: largest single allocation {:>7} bytes, largest held at one moment {:>7} bytes, ceiling {CEILING} (under it: {}; every allocation accounted for: {}); \
                 by the decoder's own account {:>7} bytes held, and it says: {}; transition {}, reason {reason} ({says}); the step names the asset: {}; the world then steps: {}; available at the last tick: {}",
                repetition + 1,
                case.workers,
                case.name,
                case.asset,
                case.declared,
                case.measured.largest,
                case.measured.held_at_most,
                case.under_the_ceiling(),
                case.measured.complete,
                case.decoder_held,
                case.decoder_says,
                case.transition_in_words(),
                case.step_names_the_asset,
                case.world_steps,
                case.available_at_the_end
            );
        }
    }
    out.array_end();
    out.object_end();

    let _ = writeln!(
        summary,
        "a decoder at load under the host's ceiling of {CEILING} bytes, {}: what is read is counts of bytes and the kind of a transition, which other work on the machine does not move",
        profile_and_size(repetitions, small)
    );
    // Per case, over every repetition and worker count: the largest of the
    // two counts, and the transitions and reasons it gave.
    for (name, asset, _) in CASES {
        let cases: Vec<&Case> = row.cases().filter(|case| case.name == name).collect();
        let Some(first) = cases.first() else { continue };
        let largest = cases.iter().map(|case| case.measured.largest).max().unwrap_or(0);
        let held = cases.iter().map(|case| case.measured.held_at_most).max().unwrap_or(0);
        let mut gave: Vec<String> = cases.iter().map(|case| format!("{}{}", case.transition_in_words(), case.reason().map_or_else(String::new, |(reason, _)| format!(" for the reason {reason}")))).collect();
        gave.sort();
        gave.dedup();
        let _ = writeln!(
            summary,
            "  decoder-ceiling/{name} ({asset}, {} bytes declared): largest single allocation {largest} bytes, largest held at one moment {held} bytes, ceiling {CEILING}; transition {}; the step names the asset: {}; the world then steps: {}",
            first.declared,
            gave.join(" or "),
            cases.iter().all(|case| case.step_names_the_asset),
            cases.iter().all(|case| case.world_steps)
        );
    }
    let _ = writeln!(
        summary,
        "an allocation above the ceiling: {}; a load of Y or of Z that is not a failed transition: {}; the world and the mount as stated: {}; every input log replays: {}",
        row.an_allocation_above_the_ceiling(),
        row.a_refused_load_that_is_not_a_failed_transition(),
        row.as_stated,
        row.replays
    );
    (out, summary, log)
}

/// The mode `--decoder-ceiling [repetitions] [small]`.
pub fn mode(arguments: &[String]) {
    let (repetitions, small) = arguments_or_exit("--decoder-ceiling", arguments);
    let scratch = std::env::temp_dir().join(format!("asset-table-and-loader-decoder-ceiling-{}", std::process::id()));
    let row = run(repetitions, &[1, 4, 8], if small { Size::Small } else { Size::Default }, &scratch);
    let _ = fs::remove_dir_all(&scratch);
    let (facts, summary, log) = report(&row, repetitions, small);
    finish("decoder-ceiling", facts, &summary, &log, &format!("holds: {}", row.holds()));
}

#[cfg(test)]
mod tests {
    use prototype_core::table::{REASON_INVALID_PART, REASON_OVER_CEILING};

    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("asset-table-and-loader-decoder-ceiling-test-{tag}-{}", std::process::id()))
    }

    /// The mode at its small size: the load the ceiling allows puts its
    /// asset in force; the payload that expands beyond what it declares and
    /// the declared size above the ceiling are failed transitions that the
    /// step names, after which the world steps; no thread that reads,
    /// expands, verifies or validates holds more than the ceiling; and the
    /// input log replays.
    #[test]
    fn the_mode_runs_at_its_small_size() {
        let scratch = scratch("mode");
        let row = run(1, &[1, 4], Size::Small, &scratch);
        let _ = fs::remove_dir_all(&scratch);
        assert!(row.as_stated && row.replays && row.holds());
        let cases = &row.repetitions[0];
        assert_eq!(cases.len(), 2 * CASES.len());
        for case in cases {
            assert!(case.under_the_ceiling() && case.world_steps, "{case:?}");
            match case.name {
                "within" => {
                    assert!(matches!(case.transition, Some(Outcome::Hash(_))) && case.available_at_the_end && !case.step_names_the_asset);
                    // The artifact and the run stream of its one block are held together.
                    assert!(case.measured.held_at_most >= SIZE_OF_W && case.decoder_held >= SIZE_OF_W as u64, "{case:?}");
                    assert_eq!(case.decoder_says, "expanded");
                }
                "expands-beyond-its-declaration" => {
                    assert_eq!(case.transition, Some(Outcome::Failure(REASON_INVALID_PART)));
                    assert!(case.step_names_the_asset && !case.available_at_the_end);
                    assert_eq!(case.decoder_held, (SIZE_OF_Y + RUNS_OF_Y * runs::PAIR) as u64, "the declared sizes are the tighter bound");
                    assert!(case.measured.largest < runs::BLOCK, "nothing near the 8 388 480 bytes the stream expands to: {case:?}");
                    assert_eq!(case.decoder_says, "a block expands beyond what it declares");
                }
                _ => {
                    assert_eq!(case.transition, Some(Outcome::Failure(REASON_OVER_CEILING)));
                    assert!(case.step_names_the_asset && !case.available_at_the_end);
                    assert_eq!(case.decoder_held, 0, "nothing is reserved for a declared size above the ceiling");
                    assert!(case.measured.largest < runs::BLOCK, "{case:?}");
                }
            }
        }
        let (facts, summary, log) = report(&row, 1, true);
        let facts = facts.finish();
        assert!(facts.contains("\"mode\":\"decoder-ceiling\"") && facts.contains("largest_bytes_held_at_one_moment") && facts.contains("\"reason\":\"over-ceiling\""));
        assert!(summary.contains("an allocation above the ceiling: false") && summary.contains("does not move"));
        assert_eq!(log.lines().count(), 2 + cases.len());
    }

    /// The other reading of the reason of a declared size above the
    /// ceiling: the nearest reason the design text lists.
    #[test]
    fn under_the_other_reading_a_size_above_the_ceiling_fails_as_an_invalid_part() {
        let scratch = scratch("reading");
        let played = play(&scratch, Size::Small, 4, Options { over_the_ceiling_fails_as_an_invalid_part: true, ..Options::default() });
        assert!(played.as_stated && played.replays);
        let reasons: Vec<Option<Outcome>> = played.cases.iter().map(|case| case.transition).collect();
        assert!(matches!(reasons[0], Some(Outcome::Hash(_))));
        assert_eq!(reasons[1..], [Some(Outcome::Failure(REASON_INVALID_PART)), Some(Outcome::Failure(REASON_INVALID_PART))]);
    }

    /// The content is the one the sheet states.
    #[test]
    fn the_package_is_the_one_the_sheet_states() {
        let scratch = scratch("content");
        let content = content(&scratch);
        let _ = fs::remove_dir_all(&scratch);
        assert_eq!(content.declared, [65_536, 4_096, 4_194_304]);
        let footer: &[u8; runs::FOOTER] = content.package[content.package.len() - runs::FOOTER..].try_into().expect("a footer");
        let (at, len) = runs::read_footer(footer).expect("the footer of a package");
        let index = runs::read_index(&content.package[at as usize..(at + len) as usize]).expect("the index of a package");
        assert_eq!(index.domain.len(), 3);
        for (case, asset) in content.assets.iter().enumerate() {
            let entry = &index.domain[asset];
            assert_eq!((entry.hash, entry.size, entry.dependencies.len()), (content.hashes[case], content.declared[case], 0));
            assert_eq!(index.artifacts[&entry.hash].declared, content.declared[case], "the index declares what the slice declares");
        }
        // Y: one block whose header declares 4 096 bytes over 384 stored
        // bytes; Z: 64 blocks.
        let of_y = &index.artifacts[&content.hashes[1]];
        let header: &[u8; runs::HEADER] = content.package[of_y.blocks[0] as usize..][..runs::HEADER].try_into().expect("a header");
        assert_eq!((of_y.blocks.len(), runs::read_header(header)), (1, (4_096, 384)));
        assert_eq!(index.artifacts[&content.hashes[2]].blocks.len(), 64);
    }
}
