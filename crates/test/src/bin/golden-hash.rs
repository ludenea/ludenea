//! Prints the golden hash after checking that every thread count agrees.

use std::process::ExitCode;

use ludenea_test::golden::{self, THREAD_COUNTS};

fn main() -> ExitCode {
    let reference = golden::hash(1);
    for &threads in THREAD_COUNTS {
        let hash = golden::hash(threads);
        if hash != reference {
            eprintln!(
                "error: {threads} threads give {hash:016x}, one thread gives {reference:016x}"
            );
            return ExitCode::FAILURE;
        }
    }
    println!("{reference:016x}");
    ExitCode::SUCCESS
}
