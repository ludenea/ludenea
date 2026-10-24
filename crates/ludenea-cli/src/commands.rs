use std::fs;
use std::io::Write;
use std::num::{IntErrorKind, ParseIntError};
use std::path::{Path, PathBuf};

use clap::Args;
use ludenea_app::replay::{
    Replay, ReplayError, ReplayOutcome, load_replay, play_replay, record_replay,
};
use ludenea_app::trace::{TRACE_VERSION, Trace, compare_traces, inputs_hash, scene_hash};
use ludenea_app::{
    App, EditLog, InputLog, NamedAssetTexts, Parallel, RawGamepad, RawInput, Rng, SimError,
    Simulation,
};
use ludenea_control::{Bytes, Command, Host, InProcess, PROTOCOL_VERSION, Reply, Transport};
use ludenea_net::{SyncTestError, sync_test};

use ludenea_app::project::project_assets;

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
fn check_replay(
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

fn is_replay(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "replay")
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
pub struct SyncTestArgs {
    /// A scene, run for `--ticks` ticks with `--inputs` or `--random-inputs`, or a `.replay`,
    /// which holds its own run.
    #[command(flatten)]
    files: RunFiles,
    #[arg(long)]
    inputs: Option<PathBuf>,
    /// Play seeded random inputs for one player, mapped by the game's own raw input mapping:
    /// keys held from a set of common codes, and one gamepad with random axes and buttons.
    #[arg(long, value_name = "SEED", conflicts_with = "inputs")]
    random_inputs: Option<u64>,
    /// The ticks to test: required for a scene, every recorded tick of a replay by default.
    #[arg(long)]
    ticks: Option<u64>,
    /// The ticks each rollback plays again, from 1 to the ticks tested.
    #[arg(long, default_value_t = 8)]
    window: u32,
    #[arg(long, value_parser = thread_count, help = threads_help())]
    threads: Option<usize>,
    /// Where to write the straight and the rolled back snapshots of the first mismatch.
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
    if is_replay(&files.scene) {
        if inputs.is_some() || golden.is_some() || files.edits.is_some() || files.project.is_some()
        {
            return Err(
                "a replay holds its own inputs, edits, assets, and hashes; drop --inputs, --golden, --edits, and --project"
                    .into(),
            );
        }
        let replay = Replay::decode(&read(&files.scene)?).map_err(|error| error.to_string())?;
        return match check_replay(app, &replay, *threads, dump_dir, out)? {
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
    if check_replay(app, &replay, *threads, dump_dir, out)? {
        mismatch(out, tick, replay.hashes[tick as usize], got)?;
        say(
            out,
            "note: a second run matched the golden trace; the simulation is not deterministic",
        )?;
    }
    Ok(1)
}

/// The key codes `--random-inputs` holds: the arrows, WASD, and the keys games commonly bind.
const RANDOM_KEYS: [&str; 14] = [
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "KeyW",
    "KeyA",
    "KeyS",
    "KeyD",
    "Space",
    "Enter",
    "Escape",
    "ShiftLeft",
    "ControlLeft",
    "Delete",
];
/// The buttons and axes of the gamepad `--random-inputs` plays, as the standard mapping has.
const RANDOM_BUTTONS: usize = 17;
const RANDOM_AXES: usize = 4;

/// Runs the scene or replay straight next to a copy, built from `app` too, that mispredicts
/// and rolls back every tick ([`sync_test`]). On the first mismatch, prints the report and
/// writes both snapshots of that tick into `--dump-dir`.
pub fn synctest(app: fn() -> App, args: &SyncTestArgs, out: &mut dyn Write) -> Outcome {
    let SyncTestArgs {
        files,
        inputs,
        random_inputs,
        ticks,
        window,
        threads,
        dump_dir,
    } = args;
    let mut other = simulation(app, *threads)?;
    let (mut sim, inputs, edits, ticks) = if is_replay(&files.scene) {
        if inputs.is_some()
            || random_inputs.is_some()
            || files.edits.is_some()
            || files.project.is_some()
        {
            return Err(
                "a replay holds its own inputs, edits, and assets; drop --inputs, --random-inputs, --edits, and --project"
                    .into(),
            );
        }
        let replay = Replay::decode(&read(&files.scene)?).map_err(|error| error.to_string())?;
        let recorded = (replay.hashes.len() as u64).saturating_sub(1);
        let ticks = match ticks {
            Some(ticks) if *ticks > recorded => {
                return Err(format!(
                    "cannot test {ticks} ticks: the replay records {recorded}"
                ));
            }
            Some(ticks) => *ticks,
            None => recorded,
        };
        let mut sim = simulation(app, *threads)?;
        let (inputs, edits) = load_replay(&mut sim, &replay).map_err(|error| error.to_string())?;
        load_replay(&mut other, &replay).map_err(|error| error.to_string())?;
        (sim, inputs, edits, ticks)
    } else {
        let Some(ticks) = *ticks else {
            return Err("testing a scene needs --ticks".into());
        };
        let run = Run::read(app, files, inputs.as_deref(), *threads)?;
        other
            .load_scene_with(&run.scene, &run.assets)
            .map_err(|error| format!("cannot load {}: {error}", files.scene.display()))?;
        let inputs = match random_inputs {
            Some(seed) => random_input_log(&run.sim, *seed, ticks)?,
            None => run.inputs,
        };
        (run.sim, inputs, run.edits, ticks)
    };
    for sim in [&mut sim, &mut other] {
        sim.set_players(usize::from(inputs.players));
    }
    match sync_test(&mut sim, &mut other, &inputs, &edits, ticks, *window) {
        Ok(report) => {
            say(
                out,
                format_args!(
                    "synctest passed: {} ticks, {} rollbacks",
                    report.ticks, report.rollbacks
                ),
            )?;
            Ok(0)
        }
        Err(error) => {
            let SyncTestError::Mismatch {
                tick, snapshots, ..
            } = &error
            else {
                return Err(error.to_string());
            };
            say(out, &error)?;
            let mut paths = Vec::new();
            for (side, snapshot) in ["straight", "rollback"].into_iter().zip(snapshots.iter()) {
                let path = dump_dir.join(format!("tick-{tick}-{side}.snap"));
                write(&path, &snapshot.encode())?;
                paths.push(path.display().to_string());
            }
            say(out, format_args!("wrote {}", paths.join(" and ")))?;
            Ok(1)
        }
    }
}

/// Seeded random devices drawn from `rng`: each key of [`RANDOM_KEYS`] held one draw in four,
/// and `gamepads` gamepads with random buttons and axes.
fn random_devices(
    rng: &mut Rng,
    gamepads: usize,
) -> (std::collections::BTreeSet<String>, Vec<RawGamepad>) {
    let keys = RANDOM_KEYS
        .iter()
        .filter(|_| rng.range_u32(0, 4) == 0)
        .map(|key| key.to_string())
        .collect();
    let gamepads = (0..gamepads)
        .map(|_| RawGamepad {
            buttons: (0..RANDOM_BUTTONS)
                .map(|_| rng.range_f32(0.0, 1.0))
                .collect(),
            axes: (0..RANDOM_AXES).map(|_| rng.range_f32(-1.0, 1.0)).collect(),
        })
        .collect();
    (keys, gamepads)
}

/// `ticks` ticks of seeded random raw input for one player, mapped by the game's mapping:
/// each key of [`RANDOM_KEYS`] held one tick in four, and one gamepad with random buttons and
/// axes.
fn random_input_log(sim: &Simulation, seed: u64, ticks: u64) -> Result<InputLog, String> {
    let mut rng = Rng::new(seed, 0);
    let mut log = InputLog::new(1);
    for tick in 0..ticks {
        let (keys, gamepads) = random_devices(&mut rng, 1);
        let raw = RawInput { keys, gamepads };
        let encoded = sim
            .map_raw_input(&[raw])
            .map_err(|error| format!("cannot map the random input of tick {tick}: {error}"))?;
        log.ticks.push(encoded);
    }
    Ok(log)
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

#[cfg(not(target_family = "wasm"))]
#[derive(Args)]
pub struct RenderArgs {
    /// A scene, run from its load with `--inputs` and `--edits`, or a `.replay`, which renders
    /// its own run; textures come from the project folder either way.
    #[command(flatten)]
    files: RunFiles,
    #[arg(long)]
    inputs: Option<PathBuf>,
    /// The tick to draw; 0 is the scene as loaded.
    #[arg(long)]
    tick: u64,
    /// The PNG to write.
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value_t = 1280, value_parser = clap::value_parser!(u32).range(1..))]
    width: u32,
    #[arg(long, default_value_t = 720, value_parser = clap::value_parser!(u32).range(1..))]
    height: u32,
    /// A debug draw layer to draw over the frame; repeat it for several.
    #[arg(long = "debug-layer")]
    debug_layers: Vec<String>,
}

/// Runs to `--tick`, draws the frames of that tick and the one before it at the later one
/// (`alpha` 1, as a game shows a tick it just ran) offscreen, and writes the pixels as a
/// PNG. Texture errors are warnings: what they leave out is drawn as placeholders. The
/// textures are read and the device opened before the run, so a project or an adapter the
/// render cannot use fails without waiting for it.
#[cfg(not(target_family = "wasm"))]
pub fn render(app: fn() -> App, args: &RenderArgs, out: &mut dyn Write) -> Outcome {
    use ludenea_app::replay::{load_replay, play_tick};
    use ludenea_render::gpu::{Overlay, Size};

    let RenderArgs {
        files,
        inputs,
        tick,
        out: path,
        width,
        height,
        debug_layers,
    } = args;
    let mut sim = simulation(app, None)?;
    let debug_layers = known_debug_layers(&sim, debug_layers)?;
    let is_replay = is_replay(&files.scene);
    if is_replay && (inputs.is_some() || files.edits.is_some()) {
        return Err("a replay holds its own inputs and edits; drop --inputs and --edits".into());
    }
    let textures = ludenea_app::project::project_textures(&files.scene, files.project.as_deref())?;
    for failure in &textures.failures {
        say(
            out,
            format_args!("warning: {failure}; the texture is left out"),
        )?;
    }
    // A replay is decoded before the adapter opens: a corrupt one is refused without a GPU.
    let decoded = if is_replay {
        Some(Replay::decode(&read(&files.scene)?).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let size = Size {
        width: *width,
        height: *height,
    };
    let mut renderer = renderer(&textures.textures, out)?;
    renderer
        .check_size(size)
        .map_err(|error| error.to_string())?;
    let replay = match decoded {
        Some(replay) => replay,
        // ponytail: a scene runs twice, recorded then played as a replay, so both kinds of
        // run render through one path; play the recording's edits directly if the cost shows.
        None => Run::read(app, files, inputs.as_deref(), None)?
            .record(*tick)
            .map_err(|error| error.to_string())?,
    };
    let last = (replay.hashes.len() as u64).saturating_sub(1);
    if *tick > last {
        return Err(format!(
            "cannot render tick {tick}: the replay ends at tick {last}"
        ));
    }
    let (recorded_inputs, recorded_edits) =
        load_replay(&mut sim, &replay).map_err(|error| error.to_string())?;
    let frame = |sim: &Simulation| {
        sim.render_frame(&debug_layers)
            .map_err(|error| format!("cannot extract the render frame: {error}"))
    };
    // Tick 0 has no tick before it: its frame is drawn alone.
    let mut previous = None;
    for at in 0..*tick {
        if at + 1 == *tick {
            previous = Some(frame(&sim)?);
        }
        play_tick(&mut sim, &recorded_inputs, &recorded_edits, at)
            .map_err(|error| error.to_string())?;
    }
    let current = frame(&sim)?;
    let played = sim.hash();
    say(out, format_args!("tick {tick} hash {played:016x}"))?;
    // Rendering goes on: the picture shows what this build plays, which is what to look at.
    if let Some(&recorded) = replay.hashes.get(*tick as usize)
        && played != recorded
    {
        say(
            out,
            format_args!(
                "warning: tick {tick} hash {played:016x} differs from the hash {recorded:016x} the replay recorded; run `verify` on the replay to find the first divergent tick"
            ),
        )?;
    }
    for warning in &current.warnings {
        say(out, format_args!("warning: {warning}"))?;
    }
    let overlay = Overlay {
        debug_layers,
        ..Overlay::default()
    };
    let report = renderer
        .set_frames(previous.as_ref(), &current, &overlay)
        .map_err(|error| error.to_string())?;
    let pixels = draw(&mut renderer, size, &report, out)?;
    write(path, &encode_png(size, &pixels)?)?;
    say(out, format_args!("wrote {}", path.display()))?;
    Ok(0)
}

/// `names` as a set, each one a debug draw layer of the game.
#[cfg(not(target_family = "wasm"))]
fn known_debug_layers(
    sim: &Simulation,
    names: &[String],
) -> Result<std::collections::BTreeSet<String>, String> {
    let known = sim.debug_layers();
    if let Some(unknown) = names.iter().find(|name| !known.contains(&name.as_str())) {
        let known = match known.is_empty() {
            true => "this game has none".to_string(),
            false => format!("its layers: {}", known.join(", ")),
        };
        return Err(format!("no debug draw layer `{unknown}`; {known}"));
    }
    Ok(names.iter().cloned().collect())
}

#[cfg(not(target_family = "wasm"))]
#[derive(Args)]
pub struct PlayArgs {
    /// The scene to play; prefabs, data assets and textures come from the project folder.
    #[command(flatten)]
    files: RunFiles,
    #[arg(
        long,
        help = format!(
            "Skip the lobby with this many local players, from 1 to {}: one holds the keyboard \
             and the first gamepad; with more, the first two share the keyboard's halves",
            ludenea_app::players::MAX_LOCAL_PLAYERS
        ),
        value_parser = clap::value_parser!(u8).range(1..=ludenea_app::players::MAX_LOCAL_PLAYERS as i64)
    )]
    players: Option<u8>,
    /// Close the window once the match reaches this tick.
    #[arg(long)]
    ticks: Option<u64>,
    /// Write the match as a replay to this file when the window closes, after a failure too.
    #[arg(long)]
    record: Option<PathBuf>,
    /// Open borderless fullscreen; F11 toggles it while playing.
    #[arg(long)]
    fullscreen: bool,
    /// Draw as fast as possible instead of in step with the display.
    #[arg(long)]
    no_vsync: bool,
    /// Keep the textures of the launch: without it, a changed PNG or meta is repacked while
    /// the game plays (a broken one keeps its previous version).
    #[arg(long)]
    no_watch: bool,
    /// Print the frame time percentiles when the window closes: the time each frame takes to
    /// make, without its wait for the screen.
    #[arg(long)]
    stats: bool,
    #[arg(long, value_parser = thread_count, help = threads_help())]
    threads: Option<usize>,
    /// A debug draw layer to draw over the game; repeat it for several.
    #[arg(long = "debug-layer")]
    debug_layers: Vec<String>,
    /// The window's width in logical pixels.
    #[arg(long, default_value_t = 1280, value_parser = clap::value_parser!(u32).range(1..))]
    width: u32,
    /// The window's height in logical pixels.
    #[arg(long, default_value_t = 720, value_parser = clap::value_parser!(u32).range(1..))]
    height: u32,
}

/// Loads the scene with its project's assets, textures (texture warnings as in `render`) and
/// edits, then plays it in a window, repacking the textures that change unless `--no-watch`.
/// Everything is read and the game loaded before the window opens, so bad files fail at
/// once.
#[cfg(not(target_family = "wasm"))]
pub fn play(app: fn() -> App, args: &PlayArgs, out: &mut dyn Write) -> Outcome {
    use ludenea_runtime::native::PlayOptions;

    let files = &args.files;
    if is_replay(&files.scene) {
        return Err(format!(
            "{} is a replay; play runs a scene (`verify` checks a replay, `render` draws one)",
            files.scene.display()
        ));
    }
    let game = game_files(
        &files.scene,
        files.project.as_deref(),
        files.edits.as_deref(),
    )?;
    let sim = simulation(app, None)?;
    let debug_layers = known_debug_layers(&sim, &args.debug_layers)?;
    let textures = ludenea_app::project::project_textures(&files.scene, files.project.as_deref())?;
    for failure in &textures.failures {
        say(
            out,
            format_args!("warning: {failure}; the texture is left out"),
        )?;
    }
    let title = files
        .scene
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let options = PlayOptions {
        files: game,
        textures: textures.textures,
        watch: (!args.no_watch).then(|| (files.scene.clone(), files.project.clone())),
        players: args.players,
        ticks: args.ticks,
        record: args.record.clone(),
        fullscreen: args.fullscreen,
        vsync: !args.no_vsync,
        stats: args.stats,
        threads: args.threads,
        debug_layers: debug_layers.into_iter().collect(),
        size: (args.width, args.height),
        title,
    };
    let outcome =
        ludenea_runtime::native::play(app, options, out).map_err(|error| error.to_string())?;
    if let Some(stats) = outcome.stats {
        say(out, format_args!("ticks {}, {stats}", outcome.ticks))?;
    }
    Ok(0)
}

/// The files a runtime plays: the scene's text, its project's prefab and data asset texts, and
/// the edit log at `edits`, if any.
#[cfg(not(target_family = "wasm"))]
fn game_files(
    scene: &Path,
    project: Option<&Path>,
    edits: Option<&Path>,
) -> Result<ludenea_runtime::GameFiles, String> {
    let text = String::from_utf8(read(scene)?)
        .map_err(|_| format!("{} is not valid UTF-8", scene.display()))?;
    let assets = project_assets(scene, project)?;
    Ok(ludenea_runtime::GameFiles {
        scene: text,
        assets: assets.texts.into_iter().collect(),
        edits: edits.map(read).transpose()?,
    })
}

/// A renderer on a headless device with `textures` packed into its atlas, warning about
/// each texture left out.
#[cfg(not(target_family = "wasm"))]
fn renderer(
    textures: &[ludenea_app::project::ProjectTexture],
    out: &mut dyn Write,
) -> Result<ludenea_render::gpu::Renderer, String> {
    use ludenea_render::atlas::{TextureSource, pack};
    use ludenea_render::gpu::{Renderer, headless};

    let (device, queue) = pollster::block_on(headless()).map_err(|error| error.to_string())?;
    // `render_rgba` draws in RGBA whatever the format; this one is for `draw`, unused here.
    let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm);
    let sources: Vec<TextureSource<'_>> = textures
        .iter()
        .map(|(id, path, png, meta)| TextureSource {
            id: *id,
            path: Some(path),
            png,
            meta,
        })
        .collect();
    let atlas = pack(&sources, renderer.max_page_side());
    for error in &atlas.errors {
        say(out, format_args!("warning: {error}"))?;
    }
    renderer
        .set_atlas(&atlas)
        .map_err(|error| error.to_string())?;
    Ok(renderer)
}

/// Draws the frames set last offscreen at the current tick (alpha 1), warning about each
/// frame `report` names as drawn as a placeholder, and reads the pixels back.
#[cfg(not(target_family = "wasm"))]
fn draw(
    renderer: &mut ludenea_render::gpu::Renderer,
    size: ludenea_render::gpu::Size,
    report: &ludenea_render::gpu::DrawReport,
    out: &mut dyn Write,
) -> Result<Vec<u8>, String> {
    let rgba = renderer
        .render_rgba(size, 1.0, None)
        .map_err(|error| error.to_string())?;
    for (entity, texture, frame) in &report.missing {
        say(
            out,
            format_args!(
                "warning: entity {entity}: frame {frame} of texture {texture} is not loaded, so a placeholder is drawn"
            ),
        )?;
    }
    Ok(rgba)
}

/// Tightly packed RGBA rows, top row first, as an 8-bit RGBA PNG.
#[cfg(not(target_family = "wasm"))]
fn encode_png(size: ludenea_render::gpu::Size, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let fail = |error: png::EncodingError| format!("cannot encode the PNG: {error}");
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, size.width, size.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(rgba).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(bytes)
}

/// How a runtime goes online: hosting or joining a room through a signaling listener, the STUN
/// servers, and the conditions this peer's own sends go through.
#[cfg(not(target_family = "wasm"))]
#[derive(Args)]
pub struct OnlineArgs {
    /// Host a room and print its code for the others to join.
    #[arg(long, group = "role")]
    host: bool,
    /// The peers of the hosted room, this one included, from 2 to 4 [default: 2].
    #[arg(long, conflicts_with = "join", value_parser = clap::value_parser!(u8).range(2..=4))]
    peers: Option<u8>,
    /// Host the room under this code instead of a random one.
    #[arg(long, value_name = "CODE", conflicts_with = "join", value_parser = room_code)]
    room: Option<ludenea_net::RoomCode>,
    /// Join the room of this code.
    #[arg(long, value_name = "CODE", group = "role", value_parser = room_code)]
    join: Option<ludenea_net::RoomCode>,
    /// The signaling listener's URL, such as `ludenea rooms` prints it.
    #[arg(long, value_name = "URL", default_value = ludenea_runtime::online::DEFAULT_SIGNALING)]
    signaling: String,
    /// A STUN server that tells this peer its public address; repeat it for several. The
    /// default is a public server of Google's.
    #[arg(long = "stun", value_name = "URL", default_value = ludenea_webrtc::DEFAULT_STUN, conflicts_with = "no_stun")]
    stun: Vec<String>,
    /// Use no STUN server, as on a local network.
    #[arg(long)]
    no_stun: bool,
    /// Delay every packet this peer sends by this many milliseconds.
    #[arg(long, default_value_t = 0)]
    latency_ms: u16,
    /// Delay every packet this peer sends by up to this many more milliseconds, drawn at random.
    #[arg(long, default_value_t = 0)]
    jitter_ms: u16,
    /// Lose this percentage of the packets this peer sends, drawn at random.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=100))]
    loss_percent: u16,
}

#[cfg(not(target_family = "wasm"))]
impl OnlineArgs {
    /// The options these arguments give, or `None` when they neither host nor join.
    fn options(&self) -> Option<ludenea_runtime::online::OnlineOptions> {
        use ludenea_runtime::online::{OnlineOptions, OnlineRole};

        let role = match (&self.join, self.host) {
            (Some(code), _) => OnlineRole::Join { code: code.clone() },
            (None, true) => OnlineRole::Host {
                peers: self.peers.unwrap_or(2),
                code: self.room.clone(),
            },
            (None, false) => return None,
        };
        Some(OnlineOptions {
            role,
            signaling: self.signaling.clone(),
            stun: if self.no_stun {
                Vec::new()
            } else {
                self.stun.clone()
            },
            conditions: ludenea_net::LinkConditions {
                latency_micros: u32::from(self.latency_ms) * 1_000,
                jitter_micros: u32::from(self.jitter_ms) * 1_000,
                loss_per_mille: self.loss_percent * 10,
                duplicate_per_mille: 0,
                reorder_per_mille: 0,
                reorder_extra_micros: 0,
                // `plan` draws it.
                seed: 0,
            },
        })
    }
}

#[cfg(not(target_family = "wasm"))]
fn room_code(text: &str) -> Result<ludenea_net::RoomCode, ludenea_net::RoomCodeError> {
    ludenea_net::RoomCode::parse(text)
}

#[cfg(not(target_family = "wasm"))]
#[derive(Args)]
#[command(group = clap::ArgGroup::new("role").required(true).multiple(false))]
pub struct PeerArgs {
    /// The scene to play; prefabs and data assets come from the project folder.
    scene: PathBuf,
    /// The project folder whose prefabs and data assets the scene loads; defaults to the
    /// nearest folder above the scene holding `ludenea.toml`, else the scene's folder.
    #[arg(long)]
    project: Option<PathBuf>,
    #[arg(long, value_parser = thread_count, help = threads_help())]
    threads: Option<usize>,
    #[arg(
        long,
        default_value_t = 1,
        help = format!(
            "The local players of this peer, from 1 to {}, with the devices `play --players` \
             gives them",
            ludenea_app::players::MAX_LOCAL_PLAYERS
        ),
        value_parser = clap::value_parser!(u8).range(1..=ludenea_app::players::MAX_LOCAL_PLAYERS as i64)
    )]
    players: u8,
    /// Play seeded random devices: keys held from a set of common codes, and one gamepad per
    /// local player with random axes and buttons. Without it, the players hold nothing.
    #[arg(long, value_name = "SEED")]
    random_inputs: Option<u64>,
    /// Leave the match once it reaches this tick.
    #[arg(long)]
    ticks: Option<u64>,
    /// Write the match as a replay to this file once it ended.
    #[arg(long)]
    record: Option<PathBuf>,
    /// Where to write this peer's snapshot of the divergent tick when the match desyncs.
    #[arg(long, default_value = "dump")]
    dump_dir: PathBuf,
    /// Give up when the room is not ready within this many seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 60)]
    room_timeout: u64,
    #[command(flatten)]
    online: OnlineArgs,
}

/// The time between two frames of the headless peer.
#[cfg(not(target_family = "wasm"))]
const PEER_STEP: std::time::Duration = std::time::Duration::from_millis(4);

#[cfg(not(target_family = "wasm"))]
#[expect(
    clippy::disallowed_methods,
    reason = "network pacing, never feeds the simulation"
)]
fn monotonic_now() -> std::time::Instant {
    std::time::Instant::now()
}

/// Plays a match online, headless and in real time: hosts or joins the room, readies it with
/// `--players` local players, then plays a frame every 4 ms until the match ended and stopped
/// telling the others why. Exits 0 when the match ended as left, 1 for any other ending, and
/// with an error for a failure of this peer: its game, its files, or its room.
#[cfg(not(target_family = "wasm"))]
pub fn peer(app: fn() -> App, args: &PeerArgs, game: &str, out: &mut dyn Write) -> Outcome {
    use ludenea_app::players::{LiveDevices, automatic_claims};
    use ludenea_control::MatchEndKind;
    use ludenea_runtime::Runner;
    use ludenea_runtime::online::{
        OnlineMatch, OnlinePhase, entropy, open_link, write_desync_snapshot,
    };

    let Some(options) = args.online.options() else {
        return Err("give --host or --join".into());
    };
    let hosting = args.online.host;
    let plan = options.plan(entropy()).map_err(|error| error.to_string())?;
    let files = game_files(&args.scene, args.project.as_deref(), None)?;
    let mut runner =
        Runner::load(app, &files, args.threads, Vec::new()).map_err(|error| error.to_string())?;
    let mut online =
        OnlineMatch::new(open_link(plan.link.clone()), &plan).map_err(|error| error.to_string())?;
    let players = usize::from(args.players);
    online
        .ready(automatic_claims(players))
        .map_err(|error| error.to_string())?;
    let code = online.code().clone();
    let mut rng = args.random_inputs.map(|seed| Rng::new(seed, 0));

    let start = monotonic_now();
    let mut last = start;
    let mut described = "the room has not answered yet".to_string();
    let mut started = false;
    let mut left = false;
    let mut end = None;
    let mut failure = None;
    let mut silent = std::collections::BTreeMap::new();
    loop {
        std::thread::sleep(PEER_STEP);
        let now = monotonic_now();
        let elapsed = now.duration_since(last).as_secs_f64();
        last = now;
        let micros = u64::try_from(now.duration_since(start).as_micros()).unwrap_or(u64::MAX);
        let phase = online.before_frame(&mut runner, micros);
        console(&mut runner);
        for message in online.messages() {
            say(out, format_args!("warning: {message}"))?;
        }
        match phase {
            OnlinePhase::Room(view) => {
                let description = view.describe();
                if description != described {
                    say(out, &description)?;
                    described = description;
                }
            }
            OnlinePhase::Assigned => {
                if hosting {
                    say(out, format_args!("room {code}"))?;
                    out.flush().map_err(|error| error.to_string())?;
                }
            }
            OnlinePhase::Failed(reason) => return Err(reason),
            OnlinePhase::Playing => {
                if !started {
                    started = true;
                    if let Some(layout) = online.layout() {
                        say(
                            out,
                            format_args!(
                                "match started: peer {} of {}, local players {:?}",
                                layout.local.0,
                                layout.links.len(),
                                layout.local_players()
                            ),
                        )?;
                    }
                }
            }
            OnlinePhase::Ended(ending) => {
                if end.is_none() {
                    let stats = runner.net_stats().map_err(|error| error.to_string())?;
                    let tick = ending.tick.unwrap_or(stats.tick);
                    say(
                        out,
                        format_args!("match ended at tick {tick}: {}", ending.message),
                    )?;
                    end = Some(ending);
                }
            }
        }
        if !started && end.is_none() && now.duration_since(start).as_secs() >= args.room_timeout {
            return Err(format!(
                "room {code} was not ready within {} s: {described}",
                args.room_timeout
            ));
        }
        if started || end.is_some() {
            let devices = match &mut rng {
                Some(rng) => {
                    let (keys, gamepads) = random_devices(rng, players);
                    LiveDevices {
                        keys,
                        gamepads: gamepads.into_iter().map(Some).collect(),
                    }
                }
                None => LiveDevices::default(),
            };
            if let Err(error) = runner.frame(elapsed, &devices) {
                // The match ended on this failure; the others are told while it lingers.
                failure.get_or_insert(error);
            }
            for (&peer, &for_ms) in runner.silent_peers() {
                if silent.insert(peer, for_ms) != Some(for_ms) {
                    say(
                        out,
                        format_args!("peer {peer} silent for {} s", for_ms as f64 / 1000.0),
                    )?;
                }
            }
            let back: Vec<u8> = silent
                .keys()
                .filter(|peer| !runner.silent_peers().contains_key(*peer))
                .copied()
                .collect();
            for peer in back {
                silent.remove(&peer);
                if end.is_none() {
                    say(out, format_args!("peer {peer} is back"))?;
                }
            }
            if let Some(ticks) = args.ticks
                && !left
                && runner.tick() >= ticks
                && end.is_none()
            {
                left = true;
                online.leave(&mut runner);
            }
        }
        online.after_frame(&mut runner, micros);
        if end.is_some() && !online.lingering(micros) {
            break;
        }
    }
    let Some(end) = end else {
        unreachable!("the loop ends only once the match ended");
    };
    for message in online.messages() {
        say(out, format_args!("warning: {message}"))?;
    }
    report_net_stats(&mut runner, &online, out)?;
    if let Some(path) = &args.record {
        let replay = runner.export_replay().map_err(|error| error.to_string())?;
        write(path, &replay)?;
        say(out, format_args!("wrote {}", path.display()))?;
    }
    if end.kind == MatchEndKind::Desync
        && let Some(layout) = online.layout()
    {
        let path = write_desync_snapshot(&mut runner, &end, layout.local.0, &args.dump_dir)
            .map_err(|error| error.to_string())?;
        say(
            out,
            format_args!(
                "wrote {}: compare it with the other peer's snapshot using `{game} diff`",
                path.display()
            ),
        )?;
    }
    console(&mut runner);
    if let Some(failure) = failure {
        return Err(failure.to_string());
    }
    let local = online.layout().map(|layout| layout.local.0);
    if end.kind == MatchEndKind::LocalFailure && end.peer.is_some() && end.peer == local {
        // This peer's game failed where no frame reported it, such as a host error.
        return Err(end.message);
    }
    Ok(if end.kind == MatchEndKind::Left { 0 } else { 1 })
}

/// Writes the host's console messages to the standard error, as the native window does: the
/// standard output keeps the peer's own lines.
#[cfg(not(target_family = "wasm"))]
fn console(runner: &mut ludenea_runtime::Runner) {
    use ludenea_control::ConsoleLevel;

    for (level, text) in runner.take_messages() {
        match level {
            ConsoleLevel::Info => eprintln!("{text}"),
            ConsoleLevel::Warning => eprintln!("warning: {text}"),
            ConsoleLevel::Error => eprintln!("error: {text}"),
        }
    }
}

/// One line per other peer of the match's statistics, then the link's lost sends and strays.
#[cfg(not(target_family = "wasm"))]
fn report_net_stats(
    runner: &mut ludenea_runtime::Runner,
    online: &ludenea_runtime::online::OnlineMatch,
    out: &mut dyn Write,
) -> Result<(), String> {
    let stats = runner.net_stats().map_err(|error| error.to_string())?;
    let milliseconds = |micros: u64| micros as f64 / 1000.0;
    for peer in &stats.peers {
        let dropped = if peer.dropped.is_empty() {
            "none".to_string()
        } else {
            peer.dropped
                .iter()
                .map(|dropped| format!("{} {}", dropped.count, dropped.reason))
                .collect::<Vec<_>>()
                .join(", ")
        };
        say(
            out,
            format_args!(
                "peer {}: ping {:.1} ms, jitter {:.1} ms, frame advantage {}, rollbacks {} \
                 (deepest {} ticks), prediction misses {}, stalls {}, packets dropped: {dropped}",
                peer.peer,
                milliseconds(peer.ping_micros),
                milliseconds(peer.jitter_micros),
                peer.frame_advantage,
                peer.rollbacks,
                peer.deepest_rollback,
                peer.prediction_misses,
                peer.stalls,
            ),
        )?;
    }
    say(
        out,
        format_args!(
            "link: {} sends lost, {} packets from outside the match",
            online.failed_sends(),
            online.strays()
        ),
    )
}
