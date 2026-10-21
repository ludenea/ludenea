use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ludenea_project::Project;

mod build;
mod play;
mod serve;

#[derive(Parser)]
#[command(about = "Create, edit, and build Ludenea games")]
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
        /// 127.0.0.1, `[::1]`, and the --host value are always accepted.
        #[arg(long = "allow-host", value_name = "NAME")]
        allow_host: Vec<String>,
        /// Another origin whose pages may open the project connection, such as a development
        /// server proxying it (`http://localhost:5173`; repeatable). By default only pages
        /// served from the address in the Host header may, so other sites cannot drive the
        /// project from a visitor's browser.
        #[arg(long = "allow-origin", value_name = "ORIGIN")]
        allow_origin: Vec<String>,
    },
    /// Build the project natively and play it in a window.
    Play {
        project: PathBuf,
        /// The scene to play, relative to the working directory; defaults to `default_scene` in
        /// `ludenea.toml`, which is relative to the project root.
        #[arg(long)]
        scene: Option<PathBuf>,
        /// Options of the game's own `play` (`--players`, `--record`, `--fullscreen`, ...).
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Build the project as a standalone web page: a static folder any web server can serve.
    Build {
        project: PathBuf,
        /// Build for the web (the only target for now).
        #[arg(long, required = true)]
        web: bool,
        /// The folder to write the page to; a previous web build there is replaced.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        // The game's exit code is the tool's; every failure of the tool itself is 2.
        Command::Play {
            project,
            scene,
            args,
        } => {
            return match play::run(&project, scene.as_deref(), &args) {
                Ok(code) => ExitCode::from(code),
                Err(message) => {
                    eprintln!("error: {message}");
                    ExitCode::from(2)
                }
            };
        }
        // Every failure is 2, as for `play`.
        Command::Build { project, web, out } => {
            debug_assert!(web, "clap requires --web");
            return match build::web(&project, &out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("error: {message}");
                    ExitCode::from(2)
                }
            };
        }
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
