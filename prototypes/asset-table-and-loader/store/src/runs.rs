//! The stand-in block codec `runs`, and the stand-in package whose artifacts
//! it stores in blocks. Throwaway, never product code.
//!
//! The codec is the prototype's own. It exists so that a stored payload can
//! expand beyond what it declares: nothing of it is a format the engine
//! would ship. An artifact is cut into blocks of at most 65 536 expanded
//! bytes. A block is, in this order: its declared length, the count of bytes
//! it expands to, as a 32-bit little-endian integer; its stored length, the
//! count of bytes of the run stream that follows, as a 32-bit little-endian
//! integer; then the run stream, a sequence of pairs, each a count from 1 to
//! 65 535 as a 16-bit little-endian integer followed by one byte, which
//! expands to that many copies of that byte.
//!
//! This module holds the format and its honest writer, which the producer of
//! a package runs. The decoder is the loading capability's: it expands a
//! block at load, under a ceiling that is the host's.
//!
//! The package is one file: the blocks; an index that holds, for each of its
//! assets, the slice of the domain manifest, and, for each artifact, its
//! hash, its declared size and the place of each of its blocks; and a footer
//! that says where the index stands. Its bytes are a function of its inputs:
//! the assets are in the order of their identifiers, the artifacts in the
//! order of their hashes, and the index is one block at the end of the file.
//! It holds no presentation manifest.

use std::collections::BTreeMap;

use crate::{Domain, DomainEntry, Reader, content_hash, put_dependencies, put_forms, put_text, read_dependencies, read_forms};

/// The most bytes one block expands to.
pub const BLOCK: usize = 65_536;
/// The length of the header of a block: its declared length, then its stored length.
pub const HEADER: usize = 8;
/// The length of one pair of a run stream: a count, then a byte.
pub const PAIR: usize = 3;
/// The largest count one pair holds.
pub const LONGEST_RUN: usize = 65_535;
/// The length of the footer of a package: where its index stands, how long
/// it is, and the mark of the format.
pub const FOOTER: usize = 20;

const MAGIC: &[u8; 4] = b"LGRB";

/// One block as it stands in a package: a header that declares `declared`
/// expanded bytes, then the run stream given. Nothing checks that the stream
/// expands to what the header declares: a package from elsewhere is whatever
/// its producer wrote.
pub fn block(declared: u32, stream: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + stream.len());
    out.extend_from_slice(&declared.to_le_bytes());
    out.extend_from_slice(&(stream.len() as u32).to_le_bytes());
    out.extend_from_slice(stream);
    out
}

/// The run stream the honest writer emits for an input: the runs of equal
/// bytes of the input, in order, a run longer than a pair can count cut into
/// several pairs.
pub fn runs_of(input: &[u8]) -> Vec<u8> {
    let mut stream = Vec::new();
    let mut at = 0;
    while at < input.len() {
        let byte = input[at];
        let run = input[at..].iter().take(LONGEST_RUN).take_while(|other| **other == byte).count();
        stream.extend_from_slice(&(run as u16).to_le_bytes());
        stream.push(byte);
        at += run;
    }
    stream
}

/// The header of a block: the count of bytes it declares it expands to, and
/// the count of bytes of its run stream.
pub fn read_header(header: &[u8; HEADER]) -> (usize, usize) {
    let word = |at: usize| u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]) as usize;
    (word(0), word(4))
}

/// An artifact as a package stores it: the hash that names it, the size it
/// declares, and its blocks, each a header and a run stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub hash: u64,
    pub declared: u64,
    pub blocks: Vec<Vec<u8>>,
}

impl Stored {
    /// What the honest writer stores for an artifact: its bytes cut into
    /// blocks of at most 65 536 bytes, each declaring the length its runs
    /// expand to, under the hash of the bytes and their length.
    pub fn honest(bytes: &[u8]) -> Stored {
        let blocks = bytes.chunks(BLOCK).map(|input| block(input.len() as u32, &runs_of(input))).collect();
        Stored { hash: content_hash(bytes), declared: bytes.len() as u64, blocks }
    }
}

/// Builds a package of the assets of a slice of the domain manifest and of
/// the artifacts given, each stored in blocks.
pub fn build(slice: &Domain, artifacts: &[Stored]) -> Vec<u8> {
    let mut file = MAGIC.to_vec();
    let mut index = Vec::new();
    index.extend_from_slice(&(slice.len() as u32).to_le_bytes());
    for (id, entry) in slice {
        index.extend_from_slice(&id.to_le_bytes());
        put_text(&mut index, &entry.asset_type);
        put_dependencies(&mut index, &entry.dependencies);
        put_forms(&mut index, &entry.forms);
        index.extend_from_slice(&entry.hash.to_le_bytes());
        index.extend_from_slice(&entry.size.to_le_bytes());
    }
    let by_hash: BTreeMap<u64, &Stored> = artifacts.iter().map(|artifact| (artifact.hash, artifact)).collect();
    index.extend_from_slice(&(by_hash.len() as u32).to_le_bytes());
    for (hash, artifact) in by_hash {
        index.extend_from_slice(&hash.to_le_bytes());
        index.extend_from_slice(&artifact.declared.to_le_bytes());
        index.extend_from_slice(&(artifact.blocks.len() as u32).to_le_bytes());
        for stored in &artifact.blocks {
            index.extend_from_slice(&(file.len() as u64).to_le_bytes());
            file.extend_from_slice(stored);
        }
    }
    let index_at = file.len() as u64;
    file.extend_from_slice(&index);
    file.extend_from_slice(&index_at.to_le_bytes());
    file.extend_from_slice(&(index.len() as u64).to_le_bytes());
    file.extend_from_slice(MAGIC);
    file
}

/// Where an artifact stands in a package: the size the index declares for
/// it, and the place of each of its blocks, counted from the start of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub declared: u64,
    pub blocks: Vec<u64>,
}

/// The index of a package as read back: its slice of the domain manifest,
/// and where each artifact stands. It is what its producer declared, and
/// nothing of it is verified by reading it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub domain: Domain,
    pub artifacts: BTreeMap<u64, Place>,
}

/// The footer of a package: where its index stands and how long it is.
pub fn read_footer(footer: &[u8; FOOTER]) -> Result<(u64, u64), String> {
    if &footer[FOOTER - 4..] != MAGIC {
        return Err("not a package of blocks".to_string());
    }
    let mut reader = Reader::new(&footer[..FOOTER - 4]);
    Ok((reader.u64()?, reader.u64()?))
}

/// Whether a file starts as a package of blocks does.
pub fn starts_as_a_package(head: &[u8]) -> bool {
    head.len() >= MAGIC.len() && &head[..MAGIC.len()] == MAGIC
}

/// The index of a package, from its bytes.
pub fn read_index(index: &[u8]) -> Result<Index, String> {
    let mut reader = Reader::new(index);
    let mut read = Index::default();
    for _ in 0..reader.u32()? {
        let id = reader.id()?;
        let asset_type = reader.text()?;
        let dependencies = read_dependencies(&mut reader)?;
        let forms = read_forms(&mut reader)?;
        let (hash, size) = (reader.u64()?, reader.u64()?);
        read.domain.insert(id, DomainEntry { asset_type, hash, size, dependencies, forms });
    }
    for _ in 0..reader.u32()? {
        let (hash, declared) = (reader.u64()?, reader.u64()?);
        let blocks = (0..reader.u32()?).map(|_| reader.u64()).collect::<Result<Vec<u64>, String>>()?;
        read.artifacts.insert(hash, Place { declared, blocks });
    }
    reader.end()?;
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMETERS: &str = "prototype/parameters";

    /// Expands a run stream with no bound: what the honest writer wrote,
    /// read back by the self-checks of the writer alone.
    fn expanded(stream: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for pair in stream.as_chunks::<PAIR>().0 {
            out.resize(out.len() + usize::from(u16::from_le_bytes([pair[0], pair[1]])), pair[2]);
        }
        out
    }

    #[test]
    fn the_honest_writer_emits_the_runs_of_equal_bytes_in_order() {
        assert_eq!(runs_of(b""), Vec::<u8>::new());
        assert_eq!(runs_of(b"aaab"), vec![3, 0, b'a', 1, 0, b'b']);
        assert_eq!(runs_of(b"abba"), vec![1, 0, b'a', 2, 0, b'b', 1, 0, b'a']);
        // A run longer than a pair can count is cut: 65 536 equal bytes are
        // a pair of 65 535 and a pair of one.
        let zeros = vec![0_u8; BLOCK];
        assert_eq!(runs_of(&zeros), vec![0xff, 0xff, 0, 1, 0, 0]);
        let mixed: Vec<u8> = (0..70_000_u32).map(|at| (at / 300) as u8).collect();
        let stream = runs_of(&mixed);
        assert_eq!(stream.len() % PAIR, 0);
        assert!(stream.as_chunks::<PAIR>().0.iter().all(|pair| u16::from_le_bytes([pair[0], pair[1]]) >= 1), "no pair counts no byte");
        assert_eq!(expanded(&stream), mixed);
    }

    #[test]
    fn an_honest_artifact_is_cut_into_blocks_that_declare_what_they_expand_to() {
        let bytes: Vec<u8> = (0..BLOCK as u32 * 2 + 10).map(|at| (at % 7) as u8).collect();
        let stored = Stored::honest(&bytes);
        assert_eq!((stored.hash, stored.declared), (content_hash(&bytes), bytes.len() as u64));
        assert_eq!(stored.blocks.len(), 3);
        let mut read_back = Vec::new();
        for (stored, input) in stored.blocks.iter().zip(bytes.chunks(BLOCK)) {
            let (declared, stored_len) = read_header(stored[..HEADER].try_into().expect("a header"));
            assert_eq!((declared, stored_len), (input.len(), stored.len() - HEADER));
            assert!(declared <= BLOCK);
            read_back.extend(expanded(&stored[HEADER..]));
        }
        assert_eq!(read_back, bytes);
        assert!(Stored::honest(b"").blocks.is_empty(), "an artifact of no byte has no block");
    }

    fn entry(bytes: &[u8]) -> DomainEntry {
        DomainEntry { asset_type: PARAMETERS.to_string(), hash: content_hash(bytes), size: bytes.len() as u64, dependencies: Vec::new(), forms: Vec::new() }
    }

    #[test]
    fn a_package_of_blocks_is_a_function_of_its_inputs_and_its_index_reads_back() {
        let (seven, nine) = (vec![7_u8; 100_000], b"nine".to_vec());
        let mut slice = Domain::new();
        slice.insert(7, entry(&seven));
        slice.insert(9, entry(&nine));
        let artifacts = [Stored::honest(&seven), Stored::honest(&nine)];
        let file = build(&slice, &artifacts);
        assert!(starts_as_a_package(&file));
        let reversed = [artifacts[1].clone(), artifacts[0].clone()];
        assert_eq!(build(&slice, &reversed), file, "the order in which the artifacts are given changes no byte");
        let footer: &[u8; FOOTER] = file[file.len() - FOOTER..].try_into().expect("a footer");
        let (index_at, index_len) = read_footer(footer).expect("the footer of a package");
        assert_eq!(index_at + index_len + FOOTER as u64, file.len() as u64, "the index is one block at the end of the file");
        let index = read_index(&file[index_at as usize..(index_at + index_len) as usize]).expect("the index reads back");
        assert_eq!(index.domain, slice);
        let place = &index.artifacts[&content_hash(&seven)];
        assert_eq!((place.declared, place.blocks.len()), (100_000, 2));
        // Each place is where a block starts: its header is there.
        for (at, stored) in place.blocks.iter().zip(&artifacts[0].blocks) {
            assert_eq!(&file[*at as usize..*at as usize + stored.len()], &stored[..]);
        }
        // A cut index and a file that is no package are refused.
        assert!(read_index(&file[index_at as usize..(index_at + index_len) as usize - 1]).is_err());
        let mut other = *footer;
        other[FOOTER - 1] ^= 1;
        assert!(read_footer(&other).is_err());
        assert!(!starts_as_a_package(b"LGPK"));
    }

    #[test]
    fn a_block_states_whatever_its_producer_wrote() {
        // A header that declares 4 096 bytes over 128 runs of 65 535 zeros:
        // 384 stored bytes that expand to 8 388 480 bytes.
        let stream: Vec<u8> = (0..128).flat_map(|_| [0xff, 0xff, 0]).collect();
        let crafted = block(4_096, &stream);
        assert_eq!(read_header(crafted[..HEADER].try_into().expect("a header")), (4_096, 384));
        assert_eq!(expanded(&crafted[HEADER..]).len(), 8_388_480);
    }
}
