use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ludenea_project::Project;

#[derive(Parser)]
#[command(about = "Create and edit Ludenea games")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new game project in `dir`, named after the directory.
    New {
        dir: PathBuf,
        /// The engine repository the game depends on.
        #[arg(long)]
        engine_path: PathBuf,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::New { dir, engine_path } => match create(&dir, &engine_path) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::FAILURE
            }
        },
    }
}

fn create(dir: &std::path::Path, engine_path: &std::path::Path) -> Result<(), String> {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("the directory needs a name")?;
    let parent = match dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => std::path::Path::new("."),
    };
    let project = Project::create(parent, name, engine_path).map_err(|error| error.to_string())?;
    println!("created {}", project.root().display());
    Ok(())
}
