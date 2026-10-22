//! The golden replay of the arena (`golden/arena.replay`), recorded in release by
//! `record-replay`; these tests run in the debug profile, so passing them also proves that
//! debug and release builds agree.

use std::ffi::OsString;
use std::path::PathBuf;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/arena.replay");

/// Where a run writes the snapshots around a divergence: `LUDENEA_DUMP_DIR` when it is set
/// and not empty (CI uploads it), else the temporary directory; a folder per thread count.
fn dump_dir(threads: &str) -> String {
    std::env::var_os("LUDENEA_DUMP_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join(format!("arena-replay-{threads}-threads"))
        .into_os_string()
        .into_string()
        .unwrap_or_else(|dir: OsString| panic!("the dump folder {dir:?} is not UTF-8"))
}

#[test]
fn arena_replay_verifies_at_one_two_and_eight_threads() {
    for threads in ["1", "2", "8"] {
        let dump_dir = dump_dir(threads);
        let arguments = [
            "arena",
            "verify",
            GOLDEN,
            "--threads",
            threads,
            "--dump-dir",
            &dump_dir,
        ];
        let mut said = Vec::new();
        let code = ludenea::cli::run(arena::app, &arguments, &mut said);
        let said = String::from_utf8_lossy(&said);
        assert_eq!(code, 0, "with {threads} threads: {said}");
        assert!(said.contains("601 hashes match"), "{said}");
    }
}
