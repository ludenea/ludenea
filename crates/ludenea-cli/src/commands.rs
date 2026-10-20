use std::fs;
use std::io::Write;
use std::num::{IntErrorKind, ParseIntError};
use std::path::{Path, PathBuf};

use clap::Args;
use ludenea_app::replay::{Replay, ReplayError, ReplayOutcome, play_replay, record_replay};
use ludenea_app::trace::{TRACE_VERSION, Trace, compare_traces, inputs_hash, scene_hash};
use ludenea_app::{App, EditLog, InputLog, NamedAssetTexts, Parallel, SimError, Simulation};
use ludenea_control::{Bytes, Command, Host, InProcess, PROTOCOL_VERSION, Reply, Transport};

use crate::assets::project_assets;

pub type Outcome = Result<i32, String>;

/// A run definition read from files, with the scene loaded into a simulation and the
/// hashes a trace records of it.
struct Run {
    sim: Simulation,
    scene: String,
    assets: NamedAssetTexts,
    inputs: InputLog,
    edits: EditLog,
    scene_hash: u64,
    inputs_hash: u64,
}

impl Run {
    fn read(
        app: fn() -> App,
        files: &RunFiles,
        inputs: Option<&Path>,
        threads: Option<usize>,
    ) -> Result<Run, String> {
        let scene_bytes = read(&files.scene)?;
        let scene = String::from_utf8(scene_bytes.clone())
            .map_err(|_| format!("{} is not valid UTF-8", files.scene.display()))?;
        let assets = project_assets(&files.scene, files.project.as_deref())?;
        let mut sim = simulation(app, threads)?;
        sim.load_scene_with(&scene, &assets)
            .map_err(|error| format!("cannot load {}: {error}", files.scene.display()))?;
        let scene_hash = scene_hash(&scene_bytes, &sim);
        let with_bytes = |path: &Path| read(path).map(|bytes| (bytes, path.to_path_buf()));
        let inputs = inputs.map(with_bytes).transpose()?;
        let edits = files.edits.as_deref().map(with_bytes).transpose()?;
        let inputs_hash = inputs_hash(
            inputs.as_ref().map(|(bytes, _)| &bytes[..]),
            edits.as_ref().map(|(bytes, _)| &bytes[..]),
        );
        let decode_error =
            |path: &Path, error: SimError| format!("cannot decode {}: {error}", path.display());
        let inputs = match inputs {
            Some((bytes, path)) => InputLog::decode(&bytes).map_err(|e| decode_error(&path, e))?,
            None => InputLog::new(1),
        };
        let edits = match edits {
            Some((bytes, path)) => EditLog::decode(&bytes).map_err(|e| decode_error(&path, e))?,
            None => EditLog::new(),
        };
        Ok(Run {
            sim,
            scene,
            assets,
            inputs,
            edits,
            scene_hash,
            inputs_hash,
        })
    }

    /// Runs `ticks` ticks from the scene load, recording them as a replay.
    fn record(&mut self, ticks: u64) -> Result<Replay, ReplayError> {
        record_replay(
            &mut self.sim,
            &self.scene,
            &self.assets,
            &self.inputs,
            &self.edits,
            ticks,
        )
    }
}

fn simulation(app: fn() -> App, threads: Option<usize>) -> Result<Simulation, String> {
    let mut sim = app().build().map_err(|error| error.to_string())?;
    if let Some(threads) = threads {
        sim.set_threads(threads)
            .map_err(|error| error.to_string())?;
    }
    Ok(sim)
}

/// The help of `--threads`, from the limit the pool enforces.
fn threads_help() -> String {
    format!(
        "Threads for the game, from 1 to {}; results never depend on the count",
        Parallel::MAX_THREADS
    )
}

/// Parses `--threads`, refusing a count no pool takes before any thread starts.
fn thread_count(value: &str) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let threads = value
        .parse()
        .map_err(|error: ParseIntError| match error.kind() {
            // Too large for a `usize`, so `Parallel::check` cannot name it: same refusal.
            IntErrorKind::PosOverflow => format!(
                "cannot start {value} threads: outside the supported range; use 1 to {}",
                Parallel::MAX_THREADS
            )
            .into(),
            _ => Box::new(error) as Box<dyn std::error::Error + Send + Sync>,
        })?;
    Parallel::check(threads)?;
    Ok(threads)
}

/// Plays `replay` against its hashes and tells whether they all match; on the first
/// mismatch reports it and writes the snapshots of that tick and the one before into
/// `dump_dir`, as for a panic of the game (see `failure`).
fn play(
    app: fn() -> App,
    replay: &Replay,
    threads: Option<usize>,
    dump_dir: &Path,
    out: &mut dyn Write,
) -> Result<bool, String> {
    let mut sim = simulation(app, threads)?;
    let report =
        play_replay(&mut sim, replay, |_, _| {}).map_err(|error| failure(error, dump_dir))?;
    if report.fingerprint_differs {
        say(out, "note: recorded by a build with other game types")?;
    }
    let ReplayOutcome::Diverged {
        tick,
        expected,
        got,
    } = report.outcome
    else {
        return Ok(true);
    };
    mismatch(out, tick, expected, got)?;
    let (previous, current) = report
        .diverged_snapshots
        .expect("a divergence has its snapshots");
    let dumps = previous
        .map(|bytes| (tick - 1, bytes))
        .into_iter()
        .chain([(tick, current)]);
    let mut paths = Vec::new();
    for (dumped_tick, bytes) in dumps {
        let path = dump_dir.join(format!("tick-{dumped_tick}.snap"));
        write(&path, &bytes)?;
        paths.push(path.display().to_string());
    }
    say(out, format_args!("wrote {}", paths.join(" and ")))?;
    Ok(false)
}

/// The message of a failed recording or replay. For a panic of the game while a tick ran,
/// also writes the snapshot of the state that tick ran from into `dump_dir`.
fn failure(error: ReplayError, dump_dir: &Path) -> String {
    let ReplayError::Panicked {
        panic,
        snapshot: Some(bytes),
    } = error
    else {
        return error.to_string();
    };
    let path = dump_dir.join(format!("tick-{}.snap", panic.tick));
    match write(&path, &bytes) {
        Ok(()) => format!("{panic}; wrote {}", path.display()),
        Err(error) => format!("{panic}; {error}"),
    }
}

fn mismatch(out: &mut dyn Write, tick: u64, expected: u64, got: u64) -> Result<(), String> {
    say(
        out,
        format_args!("mismatch at tick {tick}: expected {expected:016x}, got {got:016x}"),
    )
}

fn matched(out: &mut dyn Write, hashes: usize) -> Outcome {
    say(out, format_args!("ok: {hashes} hashes match"))?;
    Ok(0)
}

/// Sends `command`; the events it queues are dropped, since nothing here shows them.
fn send(client: &mut InProcess, command: Command) -> Result<Reply, String> {
    let reply = client.request(command).map_err(|error| error.message);
    client.drain_events();
    reply
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn say(out: &mut dyn Write, line: impl std::fmt::Display) -> Result<(), String> {
    writeln!(out, "{line}").map_err(|error| error.to_string())
}

/// The scene of a run, where its assets are, and its edits.
#[derive(Args)]
pub struct RunFiles {
    scene: PathBuf,
    /// The project folder whose prefabs and data assets the scene loads; defaults to the
    /// nearest folder above the scene holding `ludenea.toml`, else the scene's folder.
    #[arg(long)]
    project: Option<PathBuf>,
    /// The edit log of an editor session, replayed with the inputs.
    #[arg(long)]
    edits: Option<PathBuf>,
}

#[derive(Args)]
pub struct RunArgs {
    #[command(flatten)]
    files: RunFiles,
    #[arg(long)]
    inputs: Option<PathBuf>,
    #[arg(long)]
    ticks: u32,
    #[arg(long, value_parser = thread_count, help = threads_help())]
    threads: Option<usize>,
    /// Print the final state hash.
    #[arg(long)]
    hash: bool,
    /// Write the per-tick hashes to this file.
    #[arg(long)]
    trace: Option<PathBuf>,
    /// Write the run as a self-contained replay to this file.
    #[arg(long)]
    replay: Option<PathBuf>,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// A scene verified against `--golden`, or a `.replay` verified against its own hashes.
    #[command(flatten)]
    files: RunFiles,
    #[arg(long)]
    inputs: Option<PathBuf>,
    #[arg(long)]
    golden: Option<PathBuf>,
    #[arg(long, value_parser = thread_count, help = threads_help())]
    threads: Option<usize>,
    /// Where to write the snapshots around the first mismatch.
    #[arg(long, default_value = "dump")]
    dump_dir: PathBuf,
}

#[derive(Args)]
pub struct DiffArgs {
    snapshot_a: PathBuf,
    snapshot_b: PathBuf,
}

pub fn run(app: fn() -> App, args: &RunArgs, out: &mut dyn Write) -> Outcome {
    let mut run = Run::read(app, &args.files, args.inputs.as_deref(), args.threads)?;
    let replay = run
        .record(u64::from(args.ticks))
        .map_err(|error| error.to_string())?;
    if let Some(path) = &args.trace {
        let trace = Trace {
            version: TRACE_VERSION,
            scene_hash: run.scene_hash,
            inputs_hash: run.inputs_hash,
            hashes: replay.hashes.clone(),
        };
        write(path, &trace.encode())?;
    }
    if let Some(path) = &args.replay {
        write(path, &replay.encode())?;
    }
    if args.hash {
        let last = replay.hashes[replay.hashes.len() - 1];
        say(out, format_args!("tick {} hash {last:016x}", args.ticks))?;
    }
    Ok(0)
}

pub fn verify(app: fn() -> App, args: &VerifyArgs, out: &mut dyn Write) -> Outcome {
    let VerifyArgs {
        files,
        inputs,
        golden,
        threads,
        dump_dir,
    } = args;
    if files
        .scene
        .extension()
        .is_some_and(|extension| extension == "replay")
    {
        if inputs.is_some() || golden.is_some() || files.edits.is_some() || files.project.is_some()
        {
            return Err(
                "a replay holds its own inputs, edits, assets, and hashes; drop --inputs, --golden, --edits, and --project"
                    .into(),
            );
        }
        let replay = Replay::decode(&read(&files.scene)?).map_err(|error| error.to_string())?;
        return match play(app, &replay, *threads, dump_dir, out)? {
            true => matched(out, replay.hashes.len()),
            false => Ok(1),
        };
    }
    let (Some(inputs), Some(golden)) = (inputs, golden) else {
        return Err("verifying a scene needs --inputs and --golden".into());
    };
    let golden = Trace::decode(&read(golden)?)?;
    if golden.hashes.is_empty() {
        return Err("the golden trace has no hashes; record it again".into());
    }
    let mut run = Run::read(app, files, Some(inputs), *threads)?;
    if (run.scene_hash, run.inputs_hash) != (golden.scene_hash, golden.inputs_hash) {
        say(
            out,
            "golden trace was recorded for a different scene or input log",
        )?;
        return Ok(2);
    }
    let ticks = golden.hashes.len().saturating_sub(1) as u64;
    let mut replay = run
        .record(ticks)
        .map_err(|error| failure(error, dump_dir))?;
    let Some(tick) = compare_traces(&golden.hashes, &replay.hashes).first_divergent else {
        return matched(out, golden.hashes.len());
    };
    let got = replay.hashes[tick as usize];
    // Played again against the golden hashes, to stop at the first mismatch with dumps.
    replay.hashes = golden.hashes;
    if play(app, &replay, *threads, dump_dir, out)? {
        mismatch(out, tick, replay.hashes[tick as usize], got)?;
        say(
            out,
            "note: a second run matched the golden trace; the simulation is not deterministic",
        )?;
    }
    Ok(1)
}

pub fn diff(app: fn() -> App, args: &DiffArgs, out: &mut dyn Write) -> Outcome {
    let (a, b) = (read(&args.snapshot_a)?, read(&args.snapshot_b)?);
    let host = Host::new(app).map_err(|error| error.to_string())?;
    let mut client = InProcess::connect(host, PROTOCOL_VERSION).map_err(|error| error.message)?;
    let Reply::Differences { differences } = send(
        &mut client,
        Command::DiffSnapshots {
            a: Bytes(a),
            b: Some(Bytes(b)),
        },
    )?
    else {
        return Err("unexpected reply to DiffSnapshots".into());
    };
    if differences.is_empty() {
        say(out, "snapshots are identical")?;
        return Ok(0);
    }
    for difference in differences {
        say(out, difference)?;
    }
    Ok(1)
}

#[cfg(not(target_family = "wasm"))]
#[derive(Args)]
pub struct HostArgs {
    /// Speak the control protocol on stdin and stdout.
    #[arg(long, required = true)]
    stdio: bool,
}

#[cfg(not(target_family = "wasm"))]
pub fn host(app: fn() -> App, _args: &HostArgs) -> Outcome {
    let input = std::io::BufReader::new(std::io::stdin());
    // Unlocked: game code printing from the pool's threads must not wait for the whole session.
    ludenea_control::stdio::serve_stdio(app, input, std::io::stdout())
        .map_err(|error| error.to_string())?;
    Ok(0)
}
