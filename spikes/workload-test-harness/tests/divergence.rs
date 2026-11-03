//! The reference workload as sixteen tests of the standard harness, for the
//! game testing research. Throwaway, never product code.
//!
//! Every test restores two worlds from the shared fixture at two worker
//! counts and compares their per-tick hashes to the end of the run. Eight
//! tests expect no divergence and the sequential reference's hashes from both
//! worlds; seven inject a worker-count-dependent energy reduction in the
//! second world from a chosen tick and expect the divergence report to name
//! that tick and the energy as the first divergent group, with the two
//! snapshots of that tick different; the sixteenth injects a panic in a
//! system of the second world when the orchestrator arms it, and fails alone
//! with the failure's context in its message. Each test writes its report
//! where the orchestrator asks it to.

use workload_test_harness::{Case, GROUPS, Size, run_case, settings, shared};

fn expect_no_divergence(case: Case) {
    let report = run_case(&case);
    report.write();
    assert!(report.failure.is_none(), "{}: world B failed: {:?}", case.name, report.failure);
    assert!(report.divergence.is_none(), "{}: first divergent tick {:?}", case.name, report.divergence.as_ref().map(|d| d.first_divergent_tick));
    assert!(report.a_matches_reference(), "{}: world A left the sequential reference", case.name);
    assert!(report.b_matches_reference(), "{}: world B left the sequential reference", case.name);
    assert_eq!(report.run_hash_a(), shared().reference.run_hash);
}

fn expect_divergence(case: Case) {
    let report = run_case(&case);
    report.write();
    assert!(report.failure.is_none(), "{}: world B failed: {:?}", case.name, report.failure);
    let injected = report.injection_tick.expect("the case injects a divergence");
    assert!(report.a_matches_reference(), "{}: world A left the sequential reference", case.name);
    let Some(divergence) = report.divergence.as_ref() else {
        // The free-running reduction regroups the chunks of the motion pass
        // by the worker that ran them. The small size has four chunks of a
        // pass short enough for one worker to run them all before the others
        // wake, which is the reference's grouping: at that size a missing
        // divergence is the construct's, not the test's, and the self-check
        // accepts it. The measured run (10 000 entities, 40 chunks) demands it.
        assert_eq!(settings().size, Size::Small, "{}: no divergence found after tick {injected}", case.name);
        return;
    };
    assert!(divergence.first_divergent_tick >= injected, "{}: diverged at {} before the injected tick {injected}", case.name, divergence.first_divergent_tick);
    assert_eq!(divergence.snapshot_tick, divergence.first_divergent_tick, "{}: the snapshots are taken at the divergent tick", case.name);
    assert_eq!(divergence.first_divergent_group, Some(GROUPS[2]), "{}: the energy diverges first", case.name);
    assert_eq!(divergence.groups_differing, vec!["energy"], "{}: only the energy differs at that tick", case.name);
    assert!(divergence.first_differing_byte.is_some(), "{}: the two snapshots differ", case.name);
    assert_eq!(divergence.snapshot_bytes_a, divergence.snapshot_bytes_b);
}

#[test]
fn no_injection_1_vs_4_workers() {
    expect_no_divergence(Case::new("no_injection_1_vs_4_workers", 1, 4));
}

#[test]
fn no_injection_1_vs_2_workers() {
    expect_no_divergence(Case::new("no_injection_1_vs_2_workers", 1, 2));
}

#[test]
fn no_injection_2_vs_4_workers() {
    expect_no_divergence(Case::new("no_injection_2_vs_4_workers", 2, 4));
}

#[test]
fn no_injection_4_vs_4_workers() {
    expect_no_divergence(Case::new("no_injection_4_vs_4_workers", 4, 4));
}

#[test]
fn no_injection_1_vs_3_workers() {
    expect_no_divergence(Case::new("no_injection_1_vs_3_workers", 1, 3));
}

#[test]
fn no_injection_2_vs_3_workers() {
    expect_no_divergence(Case::new("no_injection_2_vs_3_workers", 2, 3));
}

#[test]
fn no_injection_3_vs_4_workers() {
    expect_no_divergence(Case::new("no_injection_3_vs_4_workers", 3, 4));
}

#[test]
fn no_injection_1_vs_1_workers() {
    expect_no_divergence(Case::new("no_injection_1_vs_1_workers", 1, 1));
}

#[test]
fn injection_at_500_1_vs_4_workers() {
    expect_divergence(Case::new("injection_at_500_1_vs_4_workers", 1, 4).diverging_from(500));
}

#[test]
fn injection_at_250_2_vs_4_workers() {
    expect_divergence(Case::new("injection_at_250_2_vs_4_workers", 2, 4).diverging_from(250));
}

#[test]
fn injection_at_750_1_vs_2_workers() {
    expect_divergence(Case::new("injection_at_750_1_vs_2_workers", 1, 2).diverging_from(750));
}

#[test]
fn injection_at_100_1_vs_3_workers() {
    expect_divergence(Case::new("injection_at_100_1_vs_3_workers", 1, 3).diverging_from(100));
}

#[test]
fn injection_at_900_3_vs_4_workers() {
    expect_divergence(Case::new("injection_at_900_3_vs_4_workers", 3, 4).diverging_from(900));
}

#[test]
fn injection_at_001_4_vs_4_workers() {
    expect_divergence(Case::new("injection_at_001_4_vs_4_workers", 4, 4).diverging_from(1));
}

#[test]
fn injection_at_950_1_vs_4_workers() {
    expect_divergence(Case::new("injection_at_950_1_vs_4_workers", 1, 4).diverging_from(950));
}

/// Like the first test, until the orchestrator arms the panic: then world B
/// panics in the second chunk of its repulsion pass at the stated tick, the
/// world fails with its context, and so does this test, alone.
#[test]
fn a_panic_in_a_system_fails_alone() {
    let case = Case::new("a_panic_in_a_system_fails_alone", 1, 4).panicking_at(300);
    let report = run_case(&case);
    report.write();
    if let Some(failure) = &report.failure {
        assert_eq!(report.history_b.len() as u32, failure.tick, "the history of world B stops at the last good tick");
        panic!(
            "world B failed at tick {} in {} chunk {} on worker {}: {} (world A at tick {}, matching the reference: {})",
            failure.tick,
            failure.node,
            failure.chunk,
            failure.worker,
            failure.message,
            report.history_a.len() - 1,
            report.a_matches_reference()
        );
    }
    assert!(report.divergence.is_none());
    assert!(report.a_matches_reference() && report.b_matches_reference());
}
