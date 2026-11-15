//! The decoder at load of the stand-in block codec, under the host's
//! ceiling, and the mount of the package whose artifacts it expands.
//! Throwaway, never product code.
//!
//! A package from elsewhere stores each artifact in blocks of the stand-in
//! codec `runs`. Its mount states a ceiling: the most the decoder may hold
//! allocated to expand one artifact. The ceiling is the host's, a setting of
//! the mount, since the sizes a package declares in its own index, in its
//! slice of the manifest and in the headers of its blocks are the
//! declarations of the party the ceiling guards against; under the ceiling
//! the declared sizes are the tighter bound.
//!
//! What the decoder holds to expand one artifact is the buffer of the
//! artifact, of the size the index declares, and the run stream of one block
//! at a time. It reserves nothing before it has checked what it is about to
//! reserve against the ceiling, and it never writes past what a block or an
//! artifact declares:
//!
//! - an artifact whose declared size stands above the ceiling is refused
//!   before any block is read;
//! - a block that declares more than a block holds, or more than its
//!   artifact has left, or whose run stream is longer than its declared
//!   length could need, is refused before its stream is read;
//! - a run that would expand a block beyond what its header declares stops
//!   the expansion there.
//!
//! An artifact that cannot be expanded is a load that fails: a failed
//! transition for a first load, with a reason. The bytes that were expanded
//! are then verified against the hash that names them and validated, as the
//! bytes of any store are.
//!
//! The expansion runs on the file thread that reads the blocks: this
//! stand-in has no background pool for it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use content_store::Mounted;
use content_store::runs::{BLOCK, FOOTER, HEADER, Index, PAIR, read_footer, read_header, read_index, starts_as_a_package};
use prototype_core::table::{REASON_ARTIFACT_GONE, REASON_INVALID_PART, REASON_OVER_CEILING, Resident};

use crate::{Instance, Loader, Store};

/// What a harness is told when a thread of the capability starts to read,
/// to expand, to verify or to validate an artifact of a mount: it is called
/// on that thread, so that a measure of the thread's allocations can start
/// there.
pub type LoadWatch = Arc<dyn Fn() + Send + Sync>;

/// How a package of blocks is mounted. A package so mounted is one its host
/// does not trust.
#[derive(Clone)]
pub struct BlockedMount {
    /// Whether an entry of the package may override an entry already
    /// mounted under the same identifier: an untrusted package may not,
    /// unless its mount says so.
    pub may_override: bool,
    /// The host's ceiling: the most bytes the decoder may hold allocated to
    /// expand one artifact.
    pub ceiling: u64,
    pub watch: Option<LoadWatch>,
}

/// Why an artifact could not be expanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotExpanded {
    /// The package holds no artifact under the hash, or its file cannot be read.
    Gone,
    /// The artifact declares more than the host's ceiling lets the decoder
    /// hold. Nothing was reserved for it.
    OverTheCeiling,
    /// The blocks are not what they declare, in the words given.
    Invalid(&'static str),
}

impl NotExpanded {
    pub fn text(self) -> &'static str {
        match self {
            NotExpanded::Gone => "the package holds no bytes under the hash",
            NotExpanded::OverTheCeiling => "the artifact declares more than the host's ceiling lets the decoder hold",
            NotExpanded::Invalid(what) => what,
        }
    }
}

/// What one expansion did: the artifact, the size the index declares for
/// it, the most bytes the decoder held reserved to expand it, by its own
/// account, and how it ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub hash: u64,
    pub declared: u64,
    pub held: u64,
    pub not_expanded: Option<NotExpanded>,
}

/// Expands one artifact under the ceiling. `declared` is the size the index
/// declares for it, `blocks` the place of each of its blocks, and `read`
/// fills a buffer from a place of the package, or says that the place is
/// outside it. Returns the bytes, or why there are none, and in both cases
/// the most bytes the decoder held reserved.
pub fn expand(declared: u64, blocks: &[u64], ceiling: u64, read: &mut dyn FnMut(u64, &mut [u8]) -> bool) -> (Result<Vec<u8>, NotExpanded>, u64) {
    let mut held = 0;
    let expanded = expand_under(declared, blocks, ceiling, read, &mut held);
    (expanded, held)
}

fn expand_under(declared: u64, blocks: &[u64], ceiling: u64, read: &mut dyn FnMut(u64, &mut [u8]) -> bool, held: &mut u64) -> Result<Vec<u8>, NotExpanded> {
    // The declared size of the artifact against the ceiling, before any
    // block is read and before anything is reserved.
    if declared > ceiling {
        return Err(NotExpanded::OverTheCeiling);
    }
    if blocks.len() as u64 != declared.div_ceil(BLOCK as u64) {
        return Err(NotExpanded::Invalid("the index gives another count of blocks than the declared size asks for"));
    }
    let size = usize::try_from(declared).map_err(|_| NotExpanded::OverTheCeiling)?;
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve_exact(size).map_err(|_| NotExpanded::OverTheCeiling)?;
    *held = declared;
    let mut stream: Vec<u8> = Vec::new();
    for place in blocks {
        let mut header = [0_u8; HEADER];
        if !read(*place, &mut header) {
            return Err(NotExpanded::Invalid("a block stands outside the package"));
        }
        let (block_declared, stored) = read_header(&header);
        if block_declared == 0 || block_declared > BLOCK {
            return Err(NotExpanded::Invalid("a block declares no byte, or more than a block holds"));
        }
        if block_declared > size - out.len() {
            return Err(NotExpanded::Invalid("a block declares more than its artifact has left"));
        }
        // A pair expands to one byte at least: a run stream longer than
        // three times the declared length cannot be what it declares.
        if !stored.is_multiple_of(PAIR) || stored > block_declared * PAIR {
            return Err(NotExpanded::Invalid("a block stores a run stream that its declared length cannot need"));
        }
        // The buffer of the artifact and the run stream of this block.
        let needed = declared + stored as u64;
        if needed > ceiling {
            return Err(NotExpanded::OverTheCeiling);
        }
        if stream.capacity() < stored {
            // The former stream is released before the next is reserved:
            // two are never held.
            stream = Vec::new();
            stream.try_reserve_exact(stored).map_err(|_| NotExpanded::OverTheCeiling)?;
        }
        *held = (*held).max(needed);
        stream.clear();
        stream.resize(stored, 0);
        if !read(*place + HEADER as u64, &mut stream) {
            return Err(NotExpanded::Invalid("a block stands outside the package"));
        }
        let mut expanded = 0;
        for pair in stream.as_chunks::<PAIR>().0 {
            let count = usize::from(u16::from_le_bytes([pair[0], pair[1]]));
            if count == 0 {
                return Err(NotExpanded::Invalid("a run of no byte"));
            }
            if count > block_declared - expanded {
                return Err(NotExpanded::Invalid("a block expands beyond what it declares"));
            }
            // Inside what was reserved: the artifact has that much left.
            out.resize(out.len() + count, pair[2]);
            expanded += count;
        }
        if expanded != block_declared {
            return Err(NotExpanded::Invalid("a block expands to less than it declares"));
        }
    }
    if out.len() != size {
        return Err(NotExpanded::Invalid("the blocks expand to less than their artifact declares"));
    }
    Ok(out)
}

/// A mounted package of blocks: its index, read at the mount, and its file,
/// of which a load reads the blocks of one artifact.
pub(crate) struct Blocked {
    file: PathBuf,
    len: u64,
    pub(crate) index: Index,
    pub(crate) mount: BlockedMount,
    decoded: Mutex<Vec<Decoded>>,
}

impl Blocked {
    /// Reads the index of a package: what its producer declared, of which
    /// nothing is verified here.
    pub(crate) fn open(path: &Path, mount: BlockedMount) -> Result<Blocked, String> {
        let wrong = |error: std::io::Error| format!("{}: {error}", path.display());
        let mut file = File::open(path).map_err(wrong)?;
        let len = file.metadata().map_err(wrong)?.len();
        let mut head = [0_u8; 4];
        if len < (head.len() + FOOTER) as u64 {
            return Err(format!("{}: not a package of blocks", path.display()));
        }
        file.read_exact(&mut head).map_err(wrong)?;
        if !starts_as_a_package(&head) {
            return Err(format!("{}: not a package of blocks", path.display()));
        }
        let mut footer = [0_u8; FOOTER];
        file.seek(SeekFrom::Start(len - FOOTER as u64)).map_err(wrong)?;
        file.read_exact(&mut footer).map_err(wrong)?;
        let (index_at, index_len) = read_footer(&footer)?;
        if index_at.checked_add(index_len).is_none_or(|end| end > len - FOOTER as u64) {
            return Err(format!("{}: the index stands outside the package", path.display()));
        }
        let mut index = vec![0_u8; index_len as usize];
        file.seek(SeekFrom::Start(index_at)).map_err(wrong)?;
        file.read_exact(&mut index).map_err(wrong)?;
        Ok(Blocked { file: path.to_path_buf(), len, index: read_index(&index)?, mount, decoded: Mutex::new(Vec::new()) })
    }

    fn holds(&self, hash: u64) -> bool {
        self.index.artifacts.contains_key(&hash)
    }

    /// Reads the blocks of one artifact and expands them under the ceiling
    /// of the mount.
    fn expand(&self, hash: u64) -> Result<Vec<u8>, NotExpanded> {
        let place = self.index.artifacts.get(&hash).ok_or(NotExpanded::Gone)?;
        let mut file = None;
        let mut gone = false;
        let mut read = |at: u64, buffer: &mut [u8]| {
            if at.checked_add(buffer.len() as u64).is_none_or(|end| end > self.len) {
                return false;
            }
            // The file is opened at the first read of a block: an artifact
            // refused on its declared size reads nothing.
            if file.is_none() {
                file = File::open(&self.file).ok();
            }
            let filled = file.as_mut().is_some_and(|file| file.seek(SeekFrom::Start(at)).is_ok() && file.read_exact(buffer).is_ok());
            gone |= !filled;
            filled
        };
        let (expanded, held) = expand(place.declared, &place.blocks, self.mount.ceiling, &mut read);
        // A file that could not be read is no fault of what it declares.
        let expanded = if gone { Err(NotExpanded::Gone) } else { expanded };
        let decoded = Decoded { hash, declared: place.declared, held, not_expanded: expanded.as_ref().err().copied() };
        self.decoded.lock().expect("the account is never poisoned").push(decoded);
        expanded
    }

    fn watched(&self) {
        if let Some(watch) = &self.mount.watch {
            watch();
        }
    }
}

/// The package answers no plain read: its artifacts are had through the
/// decoder alone, so that none is read past the ceiling of its mount.
impl Store for Blocked {
    fn read(&self, _hash: u64) -> Option<Vec<u8>> {
        None
    }
}

/// One expansion as the file threads run it: the blocks are read and
/// expanded on the thread that reads, which keeps why there are no bytes
/// where there are none.
struct Expanding {
    package: Arc<Blocked>,
    refused: Mutex<Option<NotExpanded>>,
}

impl Store for Expanding {
    fn read(&self, hash: u64) -> Option<Vec<u8>> {
        self.package.watched();
        match self.package.expand(hash) {
            Ok(bytes) => Some(bytes),
            Err(refused) => {
                *self.refused.lock().expect("the reason is never poisoned") = Some(refused);
                None
            }
        }
    }
}

impl Instance {
    /// The part a mounted package of blocks holds under a hash that no
    /// store holds whole: its blocks read and expanded under the ceiling of
    /// the mount, then verified against the hash and validated. None where
    /// no such package holds an artifact under the hash.
    pub(crate) fn expanded(&mut self, hash: u64) -> Option<Result<Arc<Resident>, u32>> {
        let package = self.decoders.iter().map(|(_, package)| package).find(|package| package.holds(hash)).cloned()?;
        // This thread verifies and validates what the file thread expands.
        package.watched();
        let expanding = Arc::new(Expanding { package, refused: Mutex::new(None) });
        let store: Arc<dyn Store> = expanding.clone();
        let bytes = self.files.read(&store, hash);
        let refused = expanding.refused.lock().expect("the reason is never poisoned").take();
        Some(match (bytes, refused) {
            (Some(bytes), _) => {
                self.report.reads += 1;
                self.residents.insert(hash, bytes).map_err(|_| REASON_INVALID_PART)
            }
            (None, Some(NotExpanded::OverTheCeiling)) if !self.options.over_the_ceiling_fails_as_an_invalid_part => Err(REASON_OVER_CEILING),
            (None, Some(NotExpanded::Gone) | None) => Err(REASON_ARTIFACT_GONE),
            (None, Some(_)) => Err(REASON_INVALID_PART),
        })
    }
}

impl Loader {
    /// Mounts a package of blocks after the packages already mounted, as a
    /// package its host does not trust: every artifact read from it is
    /// expanded under the ceiling of the mount, verified by its hash and
    /// validated. A mount while worlds exist is a revision.
    pub fn mount_blocked(&self, name: &str, file: &Path, mount: BlockedMount) -> Result<Mounted, String> {
        let (name, file) = (name.to_string(), file.to_path_buf());
        self.call(move |instance| {
            let package = Arc::new(Blocked::open(&file, mount)?);
            instance.mounts.push((name.clone(), package.index.domain.clone(), package.mount.may_override));
            instance.stores.push(package.clone());
            instance.decoders.push((name, package));
            let mounted = instance.merge().pop().expect("one result per mount");
            instance.moved();
            Ok(mounted)
        })
    }

    /// What the decoder did for each artifact it was asked to expand from a
    /// mounted package of blocks, in order: its own account of what it held.
    pub fn decoded(&self, name: &str) -> Vec<Decoded> {
        let name = name.to_string();
        self.call(move |instance| {
            let package = instance.decoders.iter().find(|(mounted, _)| *mounted == name).map(|(_, package)| package);
            package.map_or_else(Vec::new, |package| package.decoded.lock().expect("the account is never poisoned").clone())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use content_store::runs::{Stored, block, build};
    use content_store::{AssetId, Domain, DomainEntry, content_hash, encode_domain, encode_root, hex};
    use prototype_core::table::AssetType::Parameters;
    use prototype_core::table::{Arrival, ArrivalQueue, Availability, EntryView, RepulsionTable, Residents, part_bytes};

    use super::*;
    use crate::{Attachment, Options};

    const CEILING: u64 = 1_048_576;

    /// A package held in memory: what `expand` reads from.
    struct Held(Vec<u8>);

    impl Held {
        fn of(artifacts: &[Stored]) -> Held {
            Held(build(&Domain::new(), artifacts))
        }

        fn index(&self) -> Index {
            let footer: &[u8; FOOTER] = self.0[self.0.len() - FOOTER..].try_into().expect("a footer");
            let (at, len) = read_footer(footer).expect("the footer of a package");
            read_index(&self.0[at as usize..(at + len) as usize]).expect("the index of a package")
        }

        /// Expands one artifact of the package, and counts the bytes read.
        fn expand(&self, hash: u64, ceiling: u64) -> (Result<Vec<u8>, NotExpanded>, u64, usize) {
            let place = &self.index().artifacts[&hash];
            let mut bytes_read = 0;
            let mut read = |at: u64, buffer: &mut [u8]| {
                let Some(from) = self.0.get(at as usize..at as usize + buffer.len()) else { return false };
                buffer.copy_from_slice(from);
                bytes_read += buffer.len();
                true
            };
            let (expanded, held) = expand(place.declared, &place.blocks, ceiling, &mut read);
            (expanded, held, bytes_read)
        }
    }

    /// The run stream of the sheet: 128 pairs, each of the count 65 535 and
    /// of the byte zero, 384 stored bytes that expand to 8 388 480 bytes.
    fn runs_of_zeros() -> Vec<u8> {
        (0..128).flat_map(|_| [0xff, 0xff, 0]).collect()
    }

    #[test]
    fn what_the_honest_writer_stores_expands_to_the_bytes_it_was_given() {
        // Bytes that do not run, runs longer than a pair counts, an artifact
        // of several blocks whose last one is short, and one of no byte.
        let plain: Vec<u8> = (0..BLOCK as u32).map(|at| (at.wrapping_mul(31) >> (at % 4 * 8)) as u8).collect();
        let zeros = vec![0_u8; BLOCK * 3];
        let mixed: Vec<u8> = (0..BLOCK as u32 * 2 + 77).map(|at| (at / 1_000) as u8).collect();
        for bytes in [&plain, &zeros, &mixed, &Vec::new()] {
            let stored = Stored::honest(bytes);
            let held = Held::of(std::slice::from_ref(&stored));
            let (expanded, by_its_account, _) = held.expand(stored.hash, CEILING);
            let expanded = expanded.expect("an honest artifact under the ceiling expands");
            assert_eq!(content_hash(&expanded), stored.hash);
            assert_eq!(&expanded, bytes);
            assert_eq!(expanded.capacity(), bytes.len(), "the buffer of the artifact is reserved once, at its declared size");
            let longest_stream = stored.blocks.iter().map(|stored| stored.len() - HEADER).max().unwrap_or(0);
            assert_eq!(by_its_account, (bytes.len() + longest_stream) as u64, "the artifact and the run stream of one block");
            assert!(by_its_account <= CEILING);
        }
    }

    #[test]
    fn a_payload_that_expands_beyond_its_declaration_is_stopped_at_what_it_declares() {
        // The index and the header of the one block declare 4 096 bytes.
        let crafted = Stored { hash: 0x59, declared: 4_096, blocks: vec![block(4_096, &runs_of_zeros())] };
        let held = Held::of(std::slice::from_ref(&crafted));
        let (expanded, by_its_account, _) = held.expand(crafted.hash, CEILING);
        assert_eq!(expanded, Err(NotExpanded::Invalid("a block expands beyond what it declares")));
        assert_eq!(by_its_account, 4_096 + 384, "the declared sizes are the tighter bound: nothing near what the stream expands to is held");
        // A stream that expands to less than its header declares, and a run
        // of no byte, are no better.
        let short = Stored { hash: 0x5a, declared: 10, blocks: vec![block(10, &[4, 0, 7])] };
        assert_eq!(Held::of(std::slice::from_ref(&short)).expand(short.hash, CEILING).0, Err(NotExpanded::Invalid("a block expands to less than it declares")));
        let empty_run = Stored { hash: 0x5b, declared: 4, blocks: vec![block(4, &[0, 0, 7, 4, 0, 7])] };
        assert_eq!(Held::of(std::slice::from_ref(&empty_run)).expand(empty_run.hash, CEILING).0, Err(NotExpanded::Invalid("a run of no byte")));
    }

    #[test]
    fn a_declared_size_above_the_ceiling_is_refused_before_any_block_is_read() {
        // An honest artifact of four times the ceiling, each of its 64
        // blocks honestly declaring 65 536 bytes.
        let bytes = vec![0_u8; 4 * CEILING as usize];
        let stored = Stored::honest(&bytes);
        assert_eq!(stored.blocks.len(), 64);
        let held = Held::of(std::slice::from_ref(&stored));
        let (expanded, by_its_account, bytes_read) = held.expand(stored.hash, CEILING);
        assert_eq!(expanded, Err(NotExpanded::OverTheCeiling));
        assert_eq!((by_its_account, bytes_read), (0, 0), "nothing is reserved and nothing is read");
        // The same artifact expands under a ceiling that allows it.
        assert_eq!(held.expand(stored.hash, 8 * CEILING).0.map(|expanded| expanded.len()), Ok(bytes.len()));
        // The ceiling counts the run stream of a block with the artifact: an
        // artifact that fits alone and not with its stream is refused.
        let plain: Vec<u8> = (0..1_000_u32).map(|at| at as u8).collect();
        let tight = Stored::honest(&plain);
        let stream = tight.blocks[0].len() - HEADER;
        let held = Held::of(std::slice::from_ref(&tight));
        assert_eq!(held.expand(tight.hash, (plain.len() + stream) as u64).0.map(|expanded| expanded.len()), Ok(plain.len()));
        assert_eq!(held.expand(tight.hash, (plain.len() + stream) as u64 - 1).0, Err(NotExpanded::OverTheCeiling));
        assert_eq!(held.expand(tight.hash, plain.len() as u64 - 1).0, Err(NotExpanded::OverTheCeiling));
    }

    #[test]
    fn a_block_that_is_not_what_the_index_and_its_header_declare_is_refused_before_its_stream_is_read() {
        let refused = |crafted: Stored| {
            let held = Held::of(std::slice::from_ref(&crafted));
            let (expanded, by_its_account, bytes_read) = held.expand(crafted.hash, CEILING);
            (expanded.expect_err("a crafted block is refused"), by_its_account, bytes_read)
        };
        // A header that declares more than a block holds, or no byte.
        let (why, _, bytes_read) = refused(Stored { hash: 1, declared: 100, blocks: vec![block(70_000, &[1, 0, 7])] });
        assert_eq!((why, bytes_read), (NotExpanded::Invalid("a block declares no byte, or more than a block holds"), HEADER));
        assert_eq!(refused(Stored { hash: 2, declared: 100, blocks: vec![block(0, &[])] }).0, NotExpanded::Invalid("a block declares no byte, or more than a block holds"));
        // A header that declares more than its artifact has left.
        assert_eq!(refused(Stored { hash: 3, declared: 100, blocks: vec![block(101, &[101, 0, 7])] }).0, NotExpanded::Invalid("a block declares more than its artifact has left"));
        // A run stream longer than its declared length could need, or that
        // is no sequence of pairs: its length is the producer's word, and
        // nothing of that length is reserved.
        let long: Vec<u8> = (0..31).flat_map(|_| [1, 0, 7]).collect();
        let (why, by_its_account, bytes_read) = refused(Stored { hash: 4, declared: 10, blocks: vec![block(10, &long)] });
        assert_eq!((why, by_its_account, bytes_read), (NotExpanded::Invalid("a block stores a run stream that its declared length cannot need"), 10, HEADER));
        assert_eq!(refused(Stored { hash: 5, declared: 10, blocks: vec![block(10, &[10, 0])] }).0, NotExpanded::Invalid("a block stores a run stream that its declared length cannot need"));
        // An index that gives another count of blocks than the size asks for.
        assert_eq!(refused(Stored { hash: 6, declared: 10, blocks: Vec::new() }).0, NotExpanded::Invalid("the index gives another count of blocks than the declared size asks for"));
        // A place outside the package, and a header whose stream runs past
        // the end of the file.
        let mut read_nothing = |_: u64, _: &mut [u8]| false;
        assert_eq!(expand(10, &[1 << 40], CEILING, &mut read_nothing).0, Err(NotExpanded::Invalid("a block stands outside the package")));
        let mut read_the_header_alone = |at: u64, buffer: &mut [u8]| {
            if at != 0 {
                return false;
            }
            buffer.copy_from_slice(&block(10, &[10, 0, 7])[..HEADER]);
            true
        };
        assert_eq!(expand(10, &[0], CEILING, &mut read_the_header_alone).0, Err(NotExpanded::Invalid("a block stands outside the package")));
    }

    /// A loose store with an empty revision in place, in a directory of its own.
    fn empty_store(tag: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("asset-loader-decoder-check-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("objects")).expect("a scratch directory is created");
        let manifest = encode_domain(&Domain::new());
        let hash = content_hash(&manifest);
        fs::write(directory.join("objects").join(hex(hash)), &manifest).expect("the manifest is written");
        fs::write(directory.join("root"), encode_root(hash)).expect("the root document is written");
        directory
    }

    #[test]
    fn a_mounted_package_of_blocks_is_loaded_through_the_decoder_and_leaves_with_its_mount() {
        const SMALL: AssetId = 0x0a;
        const LARGE: AssetId = 0x0b;
        let directory = empty_store("mount");
        // Two assets: an honest part that the ceiling of the mount allows
        // with the run stream of its one block, and an honest part whose
        // declared size stands above it.
        let part = |salt: u32, payload: usize| part_bytes(RepulsionTable { strength: 30.0, radius: 8.0 }, &[], payload, salt);
        let (small, large) = (part(1, 100), part(2, 5_000));
        let entry = |bytes: &[u8]| DomainEntry { asset_type: Parameters.stable_name().to_string(), hash: content_hash(bytes), size: bytes.len() as u64, dependencies: Vec::new(), forms: Vec::new() };
        let slice: Domain = [(SMALL, entry(&small)), (LARGE, entry(&large))].into_iter().collect();
        let file = directory.join("elsewhere.package");
        fs::write(&file, build(&slice, &[Stored::honest(&small), Stored::honest(&large)])).expect("the package is written");
        let loader = Loader::start(&directory, Options::default()).expect("the loading capability starts");
        let mount = BlockedMount { may_override: false, ceiling: 1_000, watch: None };
        // A file that is no package of blocks does not mount.
        fs::write(directory.join("other"), b"whatever a host is handed, of any length at all").expect("a file is written");
        assert!(loader.mount_blocked("other", &directory.join("other"), mount.clone()).is_err());
        assert!(loader.mount_blocked("absent", &directory.join("absent"), mount.clone()).is_err());
        let mounted = loader.mount_blocked("elsewhere", &file, mount.clone()).expect("the package mounts");
        assert_eq!((mounted.added, mounted.refused), (vec![SMALL, LARGE], Vec::new()));
        // A world that wants both: one batch, an arrival to a hash that
        // names the size the manifest gives, and a failed transition.
        let queue = Arc::new(ArrivalQueue::default());
        let unanswered = vec![(SMALL, Parameters), (LARGE, Parameters)];
        let entries = unanswered.iter().map(|(asset, asset_type)| EntryView { asset: *asset, asset_type: *asset_type, availability: Availability::Unavailable, wanted: true }).collect();
        let extract = loader.attach(Attachment { queue: queue.clone(), set: None, entries, unanswered, restore: Vec::new(), standing: Vec::new(), checks: Vec::new() }).expect("the world is attached");
        loader.settle();
        let mut posted = BTreeMap::new();
        queue.drain_into(&mut posted);
        assert_eq!(posted.len(), 2);
        assert_eq!(posted[&SMALL], Arrival::at(SMALL, content_hash(&small)).sized(small.len() as u64));
        assert_eq!(posted[&LARGE], Arrival::failed(LARGE, REASON_OVER_CEILING));
        assert!(loader.residents().get(content_hash(&small)).is_some(), "what was expanded is verified, validated and resident");
        assert!(loader.residents().get(content_hash(&large)).is_none());
        let account = loader.decoded("elsewhere");
        assert_eq!(account.len(), 2);
        assert!(account.iter().all(|decoded| decoded.held <= 1_000), "by its own account the decoder held to the ceiling: {account:?}");
        let of_the_large = account.iter().find(|decoded| decoded.hash == content_hash(&large)).expect("the decoder was asked for it");
        assert_eq!((of_the_large.declared, of_the_large.held, of_the_large.not_expanded), (large.len() as u64, 0, Some(NotExpanded::OverTheCeiling)));
        assert_eq!(loader.report().reads, 2, "the manifest of the revision, and the one artifact that was expanded");
        // The same package mounted again without leave to override: its
        // entries are refused by name.
        assert_eq!(loader.mount_blocked("again", &file, mount).expect("the package mounts").refused, vec![SMALL, LARGE]);
        // A package leaves with its mount, and its decoder with it.
        loader.unmount("again").expect("the package unmounts");
        loader.unmount("elsewhere").expect("the package unmounts");
        assert!(loader.decoded("elsewhere").is_empty() && loader.manifest(None).is_empty());
        drop(extract);
        drop(loader);
        let _ = fs::remove_dir_all(&directory);
    }
}
