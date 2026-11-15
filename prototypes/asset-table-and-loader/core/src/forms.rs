//! Prepared forms: what a capability may declare for an asset type, an
//! artifact derived from the canonical part by a function the capability
//! supplies. Throwaway, never product code.
//!
//! No asset type of the prototype has a prepared form of its own. The two
//! preparing functions here are the stand-ins a scenario sheet defines, each
//! for one of the two kinds the design text tells apart:
//!
//! - `stamp`, whose bytes differ per platform, so that its form is built
//!   once, by a preparing action of the pipeline, and is content: its hash
//!   stands in the domain manifest, in the arrival of its asset and in the
//!   entry, beside the hash of the canonical part;
//! - `grind`, which is the same computation on every cell, so that its form
//!   is a cache of the machine: the loading capability builds it where the
//!   machine's cache of forms does not hold it, and no manifest names it.
//!
//! The functions stand here, beside the asset types they are declared for,
//! because the pipeline runs one and the loading capability the other, and
//! neither knows the other.

use reference_workload::fnv1a;

/// The platform name the content of a family is prepared under, on whatever
/// machine runs the pipeline: the producer's.
pub const PRODUCER: &str = "producer";

/// What prepares a form: one of the two stand-ins, each at version 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Preparer {
    /// Built once: its output differs per platform.
    Stamp,
    /// The same computation on every cell: a cache of the machine.
    Grind,
}

impl Preparer {
    pub fn name(self) -> &'static str {
        match self {
            Preparer::Stamp => "stamp",
            Preparer::Grind => "grind",
        }
    }

    pub fn version(self) -> u32 {
        1
    }

    /// The name and the version under which a domain manifest lists a form
    /// built once, and under which a machine's cache of forms keys a form.
    pub fn label(self) -> String {
        format!("{}, version {}", self.name(), self.version())
    }

    /// Whether the form is built once, by a preparing action of the
    /// pipeline, and named by its hash in a world's state; the other kind is
    /// built by any machine to the same bytes.
    pub fn built_once(self) -> bool {
        self == Preparer::Stamp
    }
}

/// The local platform name of the machine that runs: its operating system
/// and its processor architecture, as its toolchain names them. It is never
/// the producer's.
pub fn local_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// `stamp` at version 1: the text `stamp 1`, one space, the platform name,
/// one zero byte, then the content hash of the part; where a size is stated
/// for the form, zero bytes are appended up to that size.
pub fn stamp(part: &[u8], platform: &str, size: Option<usize>) -> Vec<u8> {
    let mut form = b"stamp 1 ".to_vec();
    form.extend_from_slice(platform.as_bytes());
    form.push(0);
    form.extend_from_slice(&fnv1a(part).to_le_bytes());
    if let Some(size) = size
        && form.len() < size
    {
        form.resize(size, 0);
    }
    form
}

/// `grind` at version 1: as many bytes as the part, after four passes, each
/// replacing every byte by the sum, modulo 256, of the bytes up to and
/// including it in the output of the pass before, the first pass reading the
/// part.
pub fn grind(part: &[u8]) -> Vec<u8> {
    let mut form = part.to_vec();
    for _ in 0..4 {
        let mut sum = 0_u8;
        for byte in form.iter_mut() {
            sum = sum.wrapping_add(*byte);
            *byte = sum;
        }
    }
    form
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_names_its_platform_and_its_part_and_is_padded_to_the_size_stated() {
        let part = b"a canonical part";
        let built = stamp(part, PRODUCER, None);
        let mut stated = b"stamp 1 producer\0".to_vec();
        stated.extend_from_slice(&fnv1a(part).to_le_bytes());
        assert_eq!(built, stated);
        // Another platform, another form; another part, another form.
        assert_ne!(stamp(part, &local_platform(), None), built, "the local platform name is never the producer's");
        assert_ne!(stamp(b"another part", PRODUCER, None), built);
        let padded = stamp(part, PRODUCER, Some(4_000));
        assert_eq!(padded.len(), 4_000);
        assert!(padded.starts_with(&built) && padded[built.len()..].iter().all(|byte| *byte == 0));
        // A size under the length of the form cuts nothing.
        assert_eq!(stamp(part, PRODUCER, Some(3)), built);
        assert!(Preparer::Stamp.built_once() && !Preparer::Grind.built_once());
        assert_eq!(Preparer::Stamp.label(), "stamp, version 1");
    }

    #[test]
    fn a_grind_is_four_passes_of_running_sums_over_the_part() {
        // One pass of running sums over 1, 2, 3 gives 1, 3, 6; then 1, 4,
        // 10; then 1, 5, 15; then 1, 6, 21.
        assert_eq!(grind(&[1, 2, 3]), vec![1, 6, 21]);
        // 200, 100 gives 200, 44; then 200, 244; then 200, 188; then 200, 132.
        assert_eq!(grind(&[200, 100]), vec![200, 132], "the sums are taken modulo 256");
        assert_eq!(grind(&[]), Vec::<u8>::new());
        let part: Vec<u8> = (0..1_000_u32).map(|k| (k * 7) as u8).collect();
        assert_eq!(grind(&part).len(), part.len());
        assert_eq!(grind(&part), grind(&part), "the same computation gives the same bytes");
    }
}
