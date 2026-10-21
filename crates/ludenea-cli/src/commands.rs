use std::fs;
use std::io::Write;
use std::num::{IntErrorKind, ParseIntError};
use std::path::{Path, PathBuf};

use clap::Args;
use ludenea_app::replay::{Replay, ReplayError, ReplayOutcome, play_replay, record_replay};
use ludenea_app::trace::{TRACE_VERSION, Trace, compare_traces, inputs_hash, scene_hash};
use ludenea_app::{App, EditLog, InputLog, NamedAssetTexts, Parallel, SimError, Simulation};
use ludenea_control::{Bytes, Command, Host, InProcess, PROTOCOL_VERSION, Reply, Transport};

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
    for warning in &textures.warnings {
        say(out, format_args!("warning: {warning}"))?;
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
