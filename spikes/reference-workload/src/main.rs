//! The reference hashes of the reference workload: runs the sequential
//! reference implementation on the three sizes, with the wander system off
//! and on, and prints one JSON report. The research CI runs it on every cell
//! of the boundary; the hashes it prints there are the ones recorded in
//! `reference-hashes.md`.
//!
//! The large size runs 100 ticks instead of 1 000 so that a cell finishes in
//! seconds; the tick count is part of the recorded parameters.

use reference_workload::{Json, Parameters, Size, run_sequential, write_platform_and_build, write_run_core};

const SCHEMA_VERSION: u64 = 1;

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", "reference-workload");
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.key("runs");
    out.array_start();
    for size in Size::ALL {
        let tick_count = match size {
            Size::Large => 100,
            _ => reference_workload::DEFAULT_TICK_COUNT,
        };
        for wander in [false, true] {
            let run = run_sequential(&Parameters::new(size, tick_count, wander));
            out.object_start();
            out.string("mode", "sequential");
            out.u64("workers", 1);
            write_run_core(&mut out, &run);
            out.object_end();
        }
    }
    out.array_end();
    out.object_end();
    println!("{}", out.finish());
}
