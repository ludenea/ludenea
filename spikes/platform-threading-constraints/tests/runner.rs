//! Runs the spike's self-check: the child-process runner must classify a clean
//! report, an abort, a hang and a panic correctly on this platform.

use std::process::Command;

#[test]
fn the_runner_classifies_the_diagnostic_probes() {
    let output = Command::new(env!("CARGO_BIN_EXE_platform-threading-constraints"))
        .arg("--self-check")
        .output()
        .expect("the spike binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("self-check: ok"),
        "self-check failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_probe_prints_one_report_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_platform-threading-constraints"))
        .args(["--probe", "noop"])
        .output()
        .expect("the spike binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert_eq!(stdout.trim(), "{\"outcome\":\"succeeded\",\"details\":{\"thread\":\"child main\"}}");
}
