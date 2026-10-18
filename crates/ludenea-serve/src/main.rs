use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ludenea_project::Project;

mod serve;

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
    /// Serve the project and the editor to a browser.
    Serve {
        project: PathBuf,
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// The address to listen on; the default keeps the project private to this machine.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// The editor build to serve (`editor/apps/web/dist`), unless the editor is embedded.
        #[arg(long)]
        editor_dir: Option<PathBuf>,
        /// The engine repository new projects depend on.
        #[arg(long)]
        engine_path: Option<PathBuf>,
        /// Another host name the editor may be reached by, for remote use (repeatable).
        /// Requests naming any other host are refused, which stops DNS rebinding; localhost,
        /// 127.0.0.1, [::1], and the --host value are always accepted.
        #[arg(long = "allow-host", value_name = "NAME")]
        allow_host: Vec<String>,
        /// Another origin whose pages may open the project connection, such as a development
        /// server proxying it (`http://localhost:5173`; repeatable). By default only pages
        /// served from the address in the Host header may, so other sites cannot drive the
        /// project from a visitor's browser.
        #[arg(long = "allow-origin", value_name = "ORIGIN")]
        allow_origin: Vec<String>,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::New { dir, engine_path } => create(&dir, &engine_path),
        Command::Serve {
            project,
            port,
            host,
            editor_dir,
            engine_path,
            allow_host,
            allow_origin,
        } => tokio::runtime::Runtime::new()
            .map_err(|error| error.to_string())
            .and_then(|runtime| {
                runtime.block_on(serve::run(serve::Options {
                    project,
                    host,
                    port,
                    editor_dir,
                    engine_path,
                    allowed_hosts: allow_host,
                    allowed_origins: allow_origin,
                }))
            }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
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
