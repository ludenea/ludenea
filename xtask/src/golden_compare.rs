//! Compares the golden hashes reported by every CI target with the committed
//! reference.

use std::collections::BTreeMap;

/// One message per problem found in the `reports` of the `expected` targets.
/// A report, keyed by target name, holds one hash per line, such as the hash
/// of the debug build and the hash of the release build. A missing target is
/// a problem: its build failed before reporting anything.
pub fn problems(
    reference: &str,
    expected: &[String],
    reports: &BTreeMap<String, String>,
) -> Vec<String> {
    let missing = expected
        .iter()
        .filter(|target| !reports.contains_key(*target))
        .map(|target| format!("{target}: no golden hash reported"));
    let unexpected = reports
        .keys()
        .filter(|target| !expected.contains(target))
        .map(|target| format!("{target}: not an expected target"));
    let diverging = reports.iter().flat_map(|(target, report)| {
        let hashes: Vec<&str> = report
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if hashes.is_empty() {
            return vec![format!("{target}: the report holds no golden hash")];
        }
        hashes
            .into_iter()
            .filter(|hash| *hash != reference)
            .map(|hash| {
                format!("{target}: golden hash {hash} differs from the reference {reference}")
            })
            .collect()
    });
    missing.chain(unexpected).chain(diverging).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn reports(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(target, report)| ((*target).to_owned(), (*report).to_owned()))
            .collect()
    }

    #[test]
    fn targets_matching_the_reference_pass() {
        let reports = reports(&[
            ("x86_64-unknown-linux-gnu", "abc\nabc\n"),
            ("wasm32-wasip1", "abc"),
        ]);

        assert_eq!(
            problems(
                "abc",
                &targets(&["x86_64-unknown-linux-gnu", "wasm32-wasip1"]),
                &reports
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_diverging_hash_is_named_with_its_target_and_the_reference() {
        let reports = reports(&[
            ("x86_64-unknown-linux-gnu", "abc"),
            ("aarch64-apple-darwin", "abc\ndef"),
        ]);

        assert_eq!(
            problems(
                "abc",
                &targets(&["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"]),
                &reports
            ),
            ["aarch64-apple-darwin: golden hash def differs from the reference abc"]
        );
    }

    #[test]
    fn a_target_without_report_is_a_problem() {
        let reports = reports(&[("x86_64-unknown-linux-gnu", "abc")]);

        assert_eq!(
            problems(
                "abc",
                &targets(&["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]),
                &reports
            ),
            ["x86_64-pc-windows-msvc: no golden hash reported"]
        );
    }

    #[test]
    fn a_report_from_an_unexpected_target_is_a_problem() {
        let reports = reports(&[
            ("x86_64-unknown-linux-gnu", "abc"),
            ("riscv64gc-unknown-linux-gnu", "abc"),
        ]);

        assert_eq!(
            problems("abc", &targets(&["x86_64-unknown-linux-gnu"]), &reports),
            ["riscv64gc-unknown-linux-gnu: not an expected target"]
        );
    }

    #[test]
    fn an_empty_report_is_a_problem() {
        let reports = reports(&[("wasm32-wasip1", "\n")]);

        assert_eq!(
            problems("abc", &targets(&["wasm32-wasip1"]), &reports),
            ["wasm32-wasip1: the report holds no golden hash"]
        );
    }
}
