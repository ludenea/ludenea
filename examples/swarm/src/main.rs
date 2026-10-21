use std::process::ExitCode;

fn main() -> ExitCode {
    ludenea::cli::main(swarm::app)
}
