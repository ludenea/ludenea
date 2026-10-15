//! Compares the golden hashes reported by every CI target with the committed
//! reference.

use std::collections::BTreeMap;

/// One message per target whose hash, keyed by target name, differs from
/// `reference`.
pub fn problems(reference: &str, hashes: &BTreeMap<String, String>) -> Vec<String> {
    if hashes.is_empty() {
        return vec!["no golden hash to compare".to_owned()];
    }
    hashes
        .iter()
        .filter(|(_, hash)| *hash != reference)
        .map(|(target, hash)| {
            format!("{target}: golden hash {hash} differs from the reference {reference}")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashes(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(target, hash)| ((*target).to_owned(), (*hash).to_owned()))
            .collect()
    }

    #[test]
    fn targets_matching_the_reference_pass() {
        let hashes = hashes(&[
            ("x86_64-unknown-linux-gnu", "abc"),
            ("wasm32-wasip1", "abc"),
        ]);

        assert_eq!(problems("abc", &hashes), Vec::<String>::new());
    }

    #[test]
    fn a_diverging_target_is_named_with_both_hashes() {
        let hashes = hashes(&[
            ("x86_64-unknown-linux-gnu", "abc"),
            ("aarch64-apple-darwin", "def"),
        ]);

        assert_eq!(
            problems("abc", &hashes),
            ["aarch64-apple-darwin: golden hash def differs from the reference abc"]
        );
    }

    #[test]
    fn no_hash_at_all_is_a_problem() {
        assert_eq!(
            problems("abc", &BTreeMap::new()),
            ["no golden hash to compare"]
        );
    }
}
