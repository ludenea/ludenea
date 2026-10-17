use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use ludenea_app::App;
use ludenea_control::{Bytes, Command, Host, InProcess, PROTOCOL_VERSION, Reply, Transport};
use xxhash_rust::xxh3::xxh3_64;

use crate::trace::{TRACE_VERSION, Trace};

pub type Outcome = Result<i32, String>;

/// A connected client with the scene loaded and the inputs and thread count applied.
struct Session {
    client: InProcess,
    scene_hash: u64,
    inputs_hash: u64,
}

impl Session {
    fn open(
        app: fn() -> App,
        scene: &Path,
        inputs: Option<&Path>,
        threads: Option<u32>,
    ) -> Result<Session, String> {
        let scene_bytes = read(scene)?;
        let ron = String::from_utf8(scene_bytes.clone())
            .map_err(|_| format!("{} is not valid UTF-8", scene.display()))?;
        let host = Host::new(app()).map_err(|error| error.to_string())?;
        let mut client =
            InProcess::connect(host, PROTOCOL_VERSION).map_err(|error| error.message)?;
        if let Some(threads) = threads {
            send(&mut client, Command::SetThreads { threads })?;
        }
        send(&mut client, Command::LoadScene { ron })?;
        let mut inputs_hash = 0;
        if let Some(path) = inputs {
            let log = read(path)?;
            inputs_hash = xxh3_64(&log);
            send(&mut client, Command::LoadInputs { log: Bytes(log) })?;
        }
        Ok(Session {
            client,
            scene_hash: xxh3_64(&scene_bytes),
            inputs_hash,
        })
    }

    fn hash(&mut self) -> Result<u64, String> {
        match send(&mut self.client, Command::GetHash)? {
            Reply::Hash { hash, .. } => Ok(hash.0),
            _ => Err("unexpected reply to GetHash".into()),
        }
    }

    /// Runs `ticks` ticks and returns the hash after each.
    fn step(&mut self, ticks: u32) -> Result<Vec<u64>, String> {
        let command = Command::Step {
            ticks,
            record_hashes: true,
        };
        match send(&mut self.client, command)? {
            Reply::Stepped { hashes, .. } => Ok(hashes.into_iter().map(|hash| hash.0).collect()),
            _ => Err("unexpected reply to Step".into()),
        }
    }

    fn snapshot(&mut self) -> Result<Vec<u8>, String> {
        match send(&mut self.client, Command::TakeSnapshot)? {
            Reply::Snapshot { snapshot, .. } => Ok(snapshot.0),
            _ => Err("unexpected reply to TakeSnapshot".into()),
        }
    }
}

fn send(client: &mut InProcess, command: Command) -> Result<Reply, String> {
    client.request(command).map_err(|error| error.message)
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

#[derive(Args)]
pub struct RunArgs {
    scene: PathBuf,
    #[arg(long)]
    inputs: Option<PathBuf>,
    #[arg(long)]
    ticks: u32,
    #[arg(long)]
    threads: Option<u32>,
    /// Print the final state hash.
    #[arg(long)]
    hash: bool,
    /// Write the per-tick hashes to this file.
    #[arg(long)]
    trace: Option<PathBuf>,
}

#[derive(Args)]
pub struct VerifyArgs {
    scene: PathBuf,
    #[arg(long)]
    inputs: PathBuf,
    #[arg(long)]
    golden: PathBuf,
    #[arg(long)]
    threads: Option<u32>,
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
    let RunArgs {
        scene,
        inputs,
        ticks,
        threads,
        hash: print_hash,
        trace,
    } = args;
    let ticks = *ticks;
    let mut session = Session::open(app, scene, inputs.as_deref(), *threads)?;
    let mut hashes = vec![session.hash()?];
    hashes.extend(session.step(ticks)?);
    let last = hashes[hashes.len() - 1];
    if let Some(path) = trace {
        let trace = Trace {
            version: TRACE_VERSION,
            scene_hash: session.scene_hash,
            inputs_hash: session.inputs_hash,
            hashes,
        };
        write(path, &trace.encode())?;
    }
    if *print_hash {
        say(out, format_args!("tick {ticks} hash {last:016x}"))?;
    }
    Ok(0)
}

pub fn verify(app: fn() -> App, args: &VerifyArgs, out: &mut dyn Write) -> Outcome {
    let VerifyArgs {
        scene,
        inputs,
        golden,
        threads,
        dump_dir,
    } = args;
    let threads = *threads;
    let golden = Trace::decode(&read(golden)?)?;
    if (xxh3_64(&read(scene)?), xxh3_64(&read(inputs)?)) != (golden.scene_hash, golden.inputs_hash)
    {
        say(
            out,
            "golden trace was recorded for a different scene or input log",
        )?;
        return Ok(2);
    }
    let mut session = Session::open(app, scene, Some(inputs), threads)?;
    // The snapshot after the previous tick stays available for the dump.
    let mut previous = session.snapshot()?;
    for (tick, expected) in golden.hashes.iter().enumerate() {
        let got = if tick == 0 {
            session.hash()?
        } else {
            session.step(1)?[0]
        };
        if got == *expected {
            if tick + 1 < golden.hashes.len() {
                previous = session.snapshot()?;
            }
            continue;
        }
        say(
            out,
            format_args!("mismatch at tick {tick}: expected {expected:016x}, got {got:016x}"),
        )?;
        let mut dumps = Vec::new();
        if tick > 0 {
            dumps.push((tick - 1, previous));
        }
        dumps.push((tick, session.snapshot()?));
        let mut paths = Vec::new();
        for (dumped_tick, bytes) in dumps {
            let path = dump_dir.join(format!("tick-{dumped_tick}.snap"));
            write(&path, &bytes)?;
            paths.push(path.display().to_string());
        }
        say(out, format_args!("wrote {}", paths.join(" and ")))?;
        return Ok(1);
    }
    say(
        out,
        format_args!("ok: {} hashes match", golden.hashes.len()),
    )?;
    Ok(0)
}

pub fn diff(app: fn() -> App, args: &DiffArgs, out: &mut dyn Write) -> Outcome {
    let (a, b) = (read(&args.snapshot_a)?, read(&args.snapshot_b)?);
    let host = Host::new(app()).map_err(|error| error.to_string())?;
    let mut client = InProcess::connect(host, PROTOCOL_VERSION).map_err(|error| error.message)?;
    let Reply::Differences { lines } = send(
        &mut client,
        Command::DiffSnapshots {
            a: Bytes(a),
            b: Bytes(b),
        },
    )?
    else {
        return Err("unexpected reply to DiffSnapshots".into());
    };
    if lines.is_empty() {
        say(out, "snapshots are identical")?;
        return Ok(0);
    }
    for line in lines {
        say(out, line)?;
    }
    Ok(1)
}
